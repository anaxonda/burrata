// Saved-track playback qualification through the actual app. Every start,
// including failures, stays in the denominator. Never logs signed media URLs.
import {Client} from '@modelcontextprotocol/client';
import {StdioClientTransport} from '@modelcontextprotocol/client/stdio';
import {randomUUID} from 'node:crypto';
import {writeFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';

export function summarize(results) {
  const timings = results.filter(r => Number.isFinite(r.serviceFirstAudioMs))
    .map(r => r.serviceFirstAudioMs).sort((a, b) => a - b);
  const percentile = fraction => timings.length ? timings[Math.ceil(fraction * timings.length) - 1] : null;
  const started = results.filter(r => r.started).length;
  const completed = results.filter(r => r.outcome === 'completed').length;
  return {attempts: results.length, started, completed, failedStarts: results.length - started,
    completionRate: results.length ? completed / results.length : null,
    startedCompletionRate: started ? completed / started : null,
    successfulStartLatencyMs: {p50: percentile(.5), p95: percentile(.95), p99: percentile(.99)},
    latencyScope: 'Service request to first mixer-consumed PCM, for successful starts; failed starts counted separately'};
}

async function main() {
  const [automationFile, output, ...trackIds] = process.argv.slice(2);
  if (!automationFile || !output || !trackIds.length) throw Error('usage: full_track_reliability.mjs AUTOMATION_FILE OUTPUT TRACK_ID [...]');
  const client = new Client({name: 'burrata-full-track-qualification', version: '1.0.0'});
  const transport = new StdioClientTransport({command: 'bash', args: ['scripts/dev/mcp-stdio.sh'],
    env: {...process.env, BURRATA_AUTOMATION_FILE: automationFile}, stderr: 'pipe'});
  transport.stderr?.on('data', () => {});
  const results = [];
  const save = () => writeFile(output, JSON.stringify({summary: summarize(results), results}, null, 2) + '\n');
  const call = async (name, args = {}) => {
    const response = await client.callTool({name, arguments: args}, {timeout: 65000});
    if (response.isError) throw Error(`App operation failed: ${name}`);
    return response.structuredContent?.result ?? response.structuredContent;
  };
  try {
    await client.connect(transport);
    for (const trackId of trackIds) {
      const result = {trackId, operationId: randomUUID(), started: false, outcome: 'pending', samples: []};
      results.push(result);
      await save();
      await call('play_tracks_queued', {trackIds: [trackId], operationId: result.operationId});
      let deadline = Date.now() + 2 * 60 * 60 * 1000;
      const startupDeadline = Date.now() + 65000;
      let selected = false;
      let lastPosition = 0;
      let progressedAt = Date.now();
      while (Date.now() < deadline) {
        const state = await call('playback_state', {limit: 1});
        if (!selected && state.trackId === trackId && state.queuedOperationId === result.operationId) {
          selected = true;
          if (Number.isFinite(state.duration) && state.duration > 0) deadline = Math.min(deadline, Date.now() + (state.duration + 180) * 1000);
        }
        if (selected && state.trackId !== trackId) { result.outcome = 'unexpected_transition'; break; }
        if (!selected && Date.now() > startupDeadline) { result.outcome = 'start_not_selected'; break; }
        if (selected) {
          const d = state.diagnostics ?? {};
          if (Number.isFinite(state.position) && state.position > lastPosition + .1) { lastPosition = state.position; progressedAt = Date.now(); }
          if (Number.isFinite(d.firstAudioMs)) { result.started = true; result.serviceFirstAudioMs ??= d.firstAudioMs; }
          result.samples.push({at: Date.now(), status: state.status, position: state.position,
            generation: state.generation, bufferingCount: d.bufferingCount, recoveryCount: d.recoveryCount});
          if (result.samples.length > 2000) result.samples.shift();
          result.lastRecovery = d.lastRecovery;
          result.failure = state.failure;
          if (state.status === 'ended') { result.outcome = result.started && state.naturalQueueEnd ? 'completed' : 'unverified_end'; break; }
          if (state.status === 'failed') { result.outcome = 'failed'; break; }
          if (state.status === 'paused') { result.outcome = 'interrupted'; break; }
          if (!result.started && Date.now() > startupDeadline) { result.outcome = 'startup_timeout'; break; }
          if (result.started && Date.now() - progressedAt > 90000) { result.outcome = 'no_output_progress'; break; }
        }
        await new Promise(resolve => setTimeout(resolve, 1000));
      }
      if (result.outcome === 'pending') result.outcome = 'observation_timeout';
      if (['observation_timeout', 'startup_timeout', 'start_not_selected', 'no_output_progress'].includes(result.outcome)) {
        await call('cancel_playback_operation', {operationId: result.operationId}).catch(() => {});
      }
      await save();
      if (['interrupted', 'unexpected_transition'].includes(result.outcome)) break;
    }
  } catch (error) {
    const active = results.at(-1);
    if (active?.outcome === 'pending') active.outcome = 'automation_error';
    throw error;
  } finally {
    await save();
    await client.close();
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
