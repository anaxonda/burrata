import { appendFile } from 'node:fs/promises';
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { hasPlaybackAdvanced } from './playback_progress.mjs';

const [mode, discoveryFile, trackId, durationText, operationId, eventsPath] = process.argv.slice(2);
if (!['prepare', 'measure'].includes(mode) || !discoveryFile || !trackId
    || (mode === 'measure' && (!durationText || !operationId || !eventsPath))) {
  console.error('usage: isolated_memory_workload.mjs prepare AUTOMATION_FILE TRACK_ID | measure AUTOMATION_FILE TRACK_ID SECONDS OPERATION_ID EVENTS_NDJSON');
  process.exit(2);
}

const projectRoot = new URL('../../', import.meta.url).pathname;
const client = new Client({ name: 'burrata-isolated-memory-run', version: '1.0.0' });
const transport = new StdioClientTransport({
  command: 'bash',
  args: [`${projectRoot}scripts/dev/mcp-stdio.sh`],
  cwd: projectRoot,
  env: { ...process.env, BURRATA_AUTOMATION_FILE: discoveryFile },
  stderr: 'pipe',
});
transport.stderr?.on('data', () => {});

async function call(name, args = {}) {
  const response = await client.callTool({ name, arguments: args });
  if (response.isError) throw new Error(`${name} returned an MCP error`);
  return response.structuredContent?.result ?? response.structuredContent ?? null;
}

async function playbackState() {
  const value = await call('playback_state', { offset: 0, limit: 10 });
  if (!value || typeof value.status !== 'string') throw new Error('playback_state omitted status');
  return value;
}

try {
  await client.connect(transport);
  if (mode === 'prepare') {
    await call('set_volume', { volume: 0 });
    await call('play_tracks', { trackIds: Array(6).fill(trackId) });
    const deadline = Date.now() + 90_000;
    let previous = null;
    let advanced = false;
    let state;
    while (Date.now() < deadline) {
      state = await playbackState();
      if (['error', 'stopped', 'idle'].includes(state.status)) {
        throw new Error(`fixture failed before playback: ${state.status}`);
      }
      if (state.status === 'playing') {
        if (hasPlaybackAdvanced(previous, state)) { advanced = true; break; }
        previous = state;
      }
      await new Promise(resolve => setTimeout(resolve, 1000));
    }
    if (!advanced) throw new Error('fixture playhead did not advance; the output path is not verified');
    const page = await call('search_library', { query: 'Synthetic Library', offset: 19900, limit: 100 });
    if (page?.total !== 20_000 || !Array.isArray(page.items) || page.items.length !== 100) {
      throw new Error('seeded synthetic library did not return its expected final search page');
    }
    console.log(JSON.stringify({ mode, volume: 0, playbackStatus: state.status,
      positionSeconds: Number(state.position ?? 0), queueTotal: state.queueTotal,
      syntheticLibraryRows: page.total, finalPageRows: page.items.length }));
    process.exitCode = 0;
  } else {
    const durationSeconds = Number(durationText);
    if (!Number.isFinite(durationSeconds) || durationSeconds < 60) throw new Error('invalid measurement duration');
    const startedAt = Date.now();
    const pageCount = 20_000 / 100;
    let pagesRead = 0;
    let stateReads = 0;
    let priorIndex = null;
    let previous = null;
    let transitions = 0;
    while ((Date.now() - startedAt) / 1000 < durationSeconds) {
      const elapsed = (Date.now() - startedAt) / 1000;
      const offset = (pagesRead * 100) % 20_000;
      const page = await call('search_library', { query: 'Synthetic Library', offset, limit: 100 });
      if (page?.total !== 20_000 || !Array.isArray(page.items) || page.items.length !== 100) {
        throw new Error(`synthetic library page mismatch at offset ${offset}`);
      }
      pagesRead++;
      const state = await playbackState();
      stateReads++;
      if (state.status !== 'playing') throw new Error(`playback stopped during measurement: ${state.status}`);
      if (state.engine !== 'native' || state.trackId !== trackId || state.queueTotal !== 6) {
        throw new Error('the isolated native playback fixture changed during measurement');
      }
      if (previous && !hasPlaybackAdvanced(previous, state)) {
        throw new Error('the fixture playhead did not advance between measurement samples');
      }
      previous = state;
      const index = Number.isInteger(state.currentIndex) ? state.currentIndex : null;
      if (priorIndex !== null && index !== null && index !== priorIndex) transitions++;
      priorIndex = index;
      await appendFile(eventsPath, `${JSON.stringify({ operationId, atSeconds: Number(elapsed.toFixed(1)),
        action: 'search_page_and_playback_state', offset, returnedRows: page.items.length,
        playbackStatus: state.status, currentIndex: index,
        positionSeconds: Number(Number(state.position ?? 0).toFixed(2)) })}\n`);
      await new Promise(resolve => setTimeout(resolve, Math.min(15_000, Math.max(0, durationSeconds * 1000 - (Date.now() - startedAt)))));
    }
    await call('pause_playback');
    console.log(JSON.stringify({ mode, operationId, durationSeconds, searchPages: pagesRead,
      playbackStateReads: stateReads, queueTransitions: transitions }));
  }
} catch (error) {
  console.error(`${error.name}: ${error.message}`);
  process.exitCode = 1;
} finally {
  await client.close().catch(() => {});
}
