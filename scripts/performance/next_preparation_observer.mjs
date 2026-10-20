// Diagnostic-only MCP observer. Importing this file never connects to an app.
import { pathToFileURL, fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';
import { lstat, readFile, realpath, writeFile } from 'node:fs/promises';

const finite = value => typeof value === 'number' && Number.isFinite(value);
const number = value => finite(value) && value >= 0 ? value : null;
const id = value => typeof value === 'string' && /^[A-Za-z0-9._:-]{1,100}$/.test(value) ? value : null;
const videoId = value => typeof value === 'string' ? value.replace(/^youtube:/, '') : null;
const failure = category => Object.assign(new Error(category), { category });

export function safeSample(state, atUnixMs) {
  const diagnostics = state?.diagnostics ?? {};
  const firstAudio = diagnostics.operationTrace?.find(row => row.phase === 'first_audio');
  return {
    atUnixMs, engine: state?.engine === 'native' ? 'native' : 'other',
    status: ['playing', 'paused', 'buffering', 'resolving', 'failed', 'idle', 'ended'].includes(state?.status) ? state.status : 'unknown',
    trackId: id(state?.trackId), entryId: id(state?.entryId),
    generation: Number.isSafeInteger(state?.generation) ? state.generation : null,
    seekSerial: Number.isSafeInteger(state?.seekSerial) ? state.seekSerial : null,
    positionSeconds: number(state?.position), durationSeconds: number(state?.duration),
    operationId: id(diagnostics.operationId), operationFirstAudioMs: number(firstAudio?.atMs),
    recoveryCount: number(diagnostics.recoveryCount), bufferingCount: number(diagnostics.bufferingCount),
    sourceFailureObserved: Boolean(diagnostics.lastSourceFailure),
    skippedFailureObserved: Boolean(diagnostics.lastSkippedTrackFailure),
    failureCategory: ['cancelled', 'timeout', 'network', 'rate_limited', 'source_access', 'source_unavailable',
      'source_private', 'source_deleted', 'no_match', 'recording_identity_mismatch', 'unsupported_audio', 'output', 'storage', 'unknown']
      .includes(state?.failure?.category) ? state.failure.category : null,
    sourceProvider: state?.candidate?.provider === 'youtube' ? 'youtube' : state?.candidate ? 'other' : null,
    sourceVideoId: /^[A-Za-z0-9_-]{11}$/.test(videoId(state?.candidate?.id) ?? '') ? videoId(state.candidate.id) : null,
    selectedFormatId: id(String(state?.quality?.formatId ?? '')),
    decoderBytesRead: number(diagnostics.decoder?.bytesRead),
    // This is not wire-byte accounting; it can include reads of cached data.
  };
}

function requireQueue(state, spec, entries) {
  if (state?.engine !== 'native') throw failure('native_engine_required');
  if (state?.queueTotal !== 2 || !Array.isArray(state.queue)
      || state.queueOffset !== 0 || state.queueLimit !== 2 || state.queueHasMore !== false
      || state.queue.map(row => row.id).join('\n') !== [spec.localTrackId, spec.nextTrackId].join('\n')
      || !Array.isArray(state.queueEntryIds) || state.queueEntryIds.length !== 2
      || state.queueEntryIds[0] === state.queueEntryIds[1]
      || (entries && state.queueEntryIds.join('\n') !== entries.join('\n'))) {
    throw failure('queue_occurrence_changed');
  }
}

export async function observeTransition({ call, now = Date.now, sleep = ms => new Promise(resolve => setTimeout(resolve, ms)) }, spec) {
  const started = now();
  const deadline = started + 100_000;
  const result = { transitionOk: false, originalFailure: null, samples: [], setupSeekSeconds: 30,
    pollIntervalMs: 100, physicalAudibilityMeasured: false, networkWireBytes: null };
  let entries, localGeneration, localSeekSerial, lastLocal, firstNext, nextOwner, sustained;
  const state = async () => {
    if (now() >= deadline) throw failure('transition_deadline');
    const snapshot = await call('playback_state', { offset: 0, limit: 2 }, deadline);
    return snapshot;
  };
  try {
    if ((await state())?.engine !== 'native') throw failure('native_engine_required');
    await call('set_volume', { volume: 0 }, deadline);
    await call('play_tracks', { trackIds: [spec.localTrackId, spec.nextTrackId] }, deadline);
    let current;
    while (now() < deadline) {
      current = await state(); requireQueue(current, spec);
      if (current.trackId !== spec.localTrackId) throw failure('unexpected_initial_occurrence');
      if (['failed', 'idle', 'ended'].includes(current.status)) throw failure('control_startup_failed');
      if (current.status === 'playing' && current.position > 0.05) break;
      await sleep(100);
    }
    entries = [...current.queueEntryIds];
    const beforeSeek = current.seekSerial;
    await call('seek_playback', { position: 30 }, deadline);
    while (now() < deadline) {
      current = await state(); requireQueue(current, spec, entries);
      if (current.trackId !== spec.localTrackId) throw failure('unexpected_initial_occurrence');
      if (current.status === 'failed') throw failure('control_seek_failed');
      if (current.status === 'playing' && current.seekSerial > beforeSeek
          && current.position >= 30 && current.position <= 32) break;
      await sleep(100);
    }
    if (Math.abs(current.duration - 90) > 0.1) throw failure('control_duration_mismatch');
    localGeneration = current.generation; localSeekSerial = current.seekSerial;
    result.seekSettledAtUnixMs = now(); result.controlGeneration = localGeneration;
    while (now() < deadline) {
      current = await state(); requireQueue(current, spec, entries);
      const sample = safeSample(current, now()); result.samples.push(sample);
      if (result.samples.length > 1001) throw failure('observation_limit');
      if (['failed', 'idle', 'ended'].includes(current.status)
          || sample.sourceFailureObserved || sample.skippedFailureObserved) {
        result.originalPlaybackFailureCategory = sample.failureCategory;
        throw failure('original_playback_failure');
      }
      if (current.trackId === spec.localTrackId) {
        if (current.entryId !== entries[0] || current.generation !== localGeneration
            || current.seekSerial !== localSeekSerial) throw failure('control_owner_changed');
        if (firstNext) throw failure('transition_reversed');
        if (current.status !== 'playing') throw failure('control_interrupted');
        lastLocal = sample;
      } else if (current.trackId === spec.nextTrackId) {
        if (!lastLocal || lastLocal.positionSeconds < 88) throw failure('control_ended_early');
        if (current.entryId !== entries[1] || current.generation === localGeneration) throw failure('next_owner_invalid');
        if (sample.sourceProvider && (sample.sourceProvider !== 'youtube' || sample.sourceVideoId !== spec.nextVideoId)) {
          throw failure('source_identity_mismatch');
        }
        const owner = `${current.entryId}:${current.generation}:${current.seekSerial}`;
        if (nextOwner && nextOwner !== owner) throw failure('next_owner_changed');
        nextOwner = owner;
        if (sample.recoveryCount > 0) throw failure('original_recovery_observed');
        if (current.status === 'playing' && sample.sourceVideoId === spec.nextVideoId && current.position > 0) {
          if (Math.abs(current.duration - spec.nextDurationSeconds) > 1) throw failure('source_duration_mismatch');
          firstNext ??= sample;
          if (!sustained) sustained = sample;
          if (sample.positionSeconds + 0.05 < sustained.positionSeconds) throw failure('next_position_reversed');
          if (sample.atUnixMs - sustained.atUnixMs >= 5000
              && sample.positionSeconds - sustained.positionSeconds >= 4.9) {
            result.transitionOk = true; result.sourceVideoId = sample.sourceVideoId;
            result.selectedFormatId = sample.selectedFormatId;
            result.sustainedNextSeconds = sample.positionSeconds - sustained.positionSeconds;
            break;
          }
        } else if (firstNext) throw failure('next_playback_interrupted');
      } else throw failure('unexpected_track');
      await sleep(100);
    }
    if (!result.transitionOk) throw failure('transition_deadline');
  } catch (error) {
    result.originalFailure ??= error.category ?? 'mcp_error';
  }
  result.wallMs = Math.max(0, now() - started);
  result.lastControl = lastLocal ?? null; result.firstNextPlaying = firstNext ?? null;
  result.observedHandoffDelayMs = lastLocal && firstNext
    ? Math.round(firstNext.atUnixMs - (lastLocal.atUnixMs + (90 - lastLocal.positionSeconds) * 1000)) : null;
  // This is a polling estimate against the last control playhead, not a
  // measurement of the speaker waveform or a claim of sample-level gaplessness.
  return result;
}

export async function requireIsolatedManifest(path) {
  const canonical = await realpath(path);
  const profile = dirname(dirname(canonical));
  if (dirname(profile) !== '/tmp' || !/^burrata-launch\.[A-Za-z0-9_-]+$/.test(profile.split('/').at(-1))
      || canonical !== join(profile, 'fixture', 'next-preparation.json')) throw failure('isolated_profile_required');
  const marker = join(profile, '.burrata-isolated-profile');
  const info = await lstat(marker);
  if (!info.isFile() || info.isSymbolicLink() || await readFile(marker, 'utf8') !== 'burrata isolated memory run\n') {
    throw failure('isolated_profile_required');
  }
  const spec = JSON.parse(await readFile(canonical, 'utf8'));
  if (spec.automationFile !== join(profile, 'data/org.burrata.discovery/automation.json')
      || spec.localDurationSeconds !== 90 || !id(spec.localTrackId) || !id(spec.nextTrackId)
      || !/^[A-Za-z0-9_-]{11}$/.test(spec.nextVideoId) || !finite(spec.nextDurationSeconds)
      || spec.nextTrackId !== `youtube:${spec.nextVideoId}`
      || spec.nextDurationSeconds < 60 || spec.nextDurationSeconds > 600) throw failure('invalid_manifest');
  const endpoint = await lstat(spec.automationFile);
  if (!endpoint.isFile() || endpoint.isSymbolicLink() || await realpath(spec.automationFile) !== spec.automationFile) {
    throw failure('isolated_profile_required');
  }
  return spec;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [manifest, output] = process.argv.slice(2);
  let client;
  try {
    if (!manifest || !output) throw failure('invalid_arguments');
    const spec = await requireIsolatedManifest(resolve(manifest));
    if (resolve(output) !== join(dirname(await realpath(manifest)), 'next-preparation-observation.json')) throw failure('isolated_output_required');
    const { Client } = await import('@modelcontextprotocol/client');
    const { StdioClientTransport } = await import('@modelcontextprotocol/client/stdio');
    const root = fileURLToPath(new URL('../../', import.meta.url));
    client = new Client({ name: 'burrata-next-preparation-diagnostic', version: '1.0.0' });
    const transport = new StdioClientTransport({ command: 'bash', args: [join(root, 'scripts/dev/mcp-stdio.sh')],
      cwd: root, env: { ...process.env, BURRATA_AUTOMATION_FILE: spec.automationFile }, stderr: 'pipe' });
    transport.stderr?.on('data', () => {});
    await client.connect(transport);
    const result = await observeTransition({ call: async (name, args, deadline) => {
      const response = await client.callTool({ name, arguments: args }, { timeout: Math.max(1, Math.min(10_000, deadline - Date.now())) });
      if (response.isError) throw failure('mcp_tool_error');
      const value = response.structuredContent?.result ?? response.structuredContent;
      if (value?.ok === false) throw failure('mcp_tool_error');
      return value;
    } }, spec);
    await writeFile(output, JSON.stringify(result, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
    process.exitCode = result.transitionOk && !result.originalFailure ? 0 : 1;
  } catch (error) {
    console.error(error.category ?? 'observer_setup_failed'); process.exitCode = 1;
  } finally { await client?.close(); }
}
