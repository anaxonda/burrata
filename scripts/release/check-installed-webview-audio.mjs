import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { webviewMediaProbeConfirmsEngine } from './webview-audio-protocol.mjs';

const [command, discoveryPath, trackId] = process.argv.slice(2);
if (!command || !discoveryPath || !/^local:[a-f0-9]{64}$/.test(trackId ?? '')) {
  console.error('usage: check-installed-webview-audio.mjs MCP_COMMAND AUTOMATION_FILE LOCAL_FIXTURE_ID');
  process.exit(2);
}

const client = new Client({ name: 'burrata-installed-webview-audio-smoke', version: '1.0.0' });
const transport = new StdioClientTransport({
  command,
  args: [],
  cwd: process.cwd(),
  env: { ...process.env, BURRATA_AUTOMATION_FILE: discoveryPath },
  stderr: 'pipe',
});
transport.stderr?.on('data', () => {});

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function call(name, args = {}) {
  let response;
  try {
    response = await client.callTool({ name, arguments: args }, { timeout: 5000 });
  } catch {
    throw Object.assign(new Error('mcp_failure'), { category: 'mcp_failure' });
  }
  if (response.isError === true) {
    throw Object.assign(new Error('mcp_failure'), { category: 'mcp_failure' });
  }
  return response.structuredContent?.result ?? response.structuredContent ?? null;
}

function sampleReady(snapshot, playback) {
  const media = snapshot?.media;
  return playback?.status === 'playing'
    && playback?.trackId === trackId
    && snapshot?.engine === 'webview'
    && snapshot?.applicable === true
    && snapshot?.trackId === trackId
    && snapshot?.queueTrackId === trackId
    && typeof snapshot?.entryId === 'string'
    && snapshot.entryId.length > 0
    && snapshot?.queueTotal === 1
    && snapshot?.previewKind === 'local'
    && media?.present === true
    && media?.sourceStable === true
    && media?.paused === false
    && Number.isInteger(media?.readyState)
    && media.readyState >= 2
    && Number.isFinite(media?.duration)
    && Number.isFinite(media?.currentTime);
}

let started = false;
try {
  await client.connect(transport);
  const listing = await client.listTools();
  const required = ['get_app_status', 'playback_state', 'set_volume', 'play_tracks',
    'pause_playback', 'inspect_webview_media'];
  if (!required.every((name) => listing.tools.some((tool) => tool.name === name))) {
    throw Object.assign(new Error('unsupported_smoke_tools'), { category: 'unsupported_smoke_tools' });
  }
  const status = await call('get_app_status');
  if (status?.running !== true || status?.permissions?.read !== true
      || status?.permissions?.control !== true) {
    throw Object.assign(new Error('smoke_permissions_unavailable'), { category: 'smoke_permissions_unavailable' });
  }
  const initial = await call('playback_state', { offset: 0, limit: 1 });
  const initialMedia = await call('inspect_webview_media');
  if (!webviewMediaProbeConfirmsEngine(initial, initialMedia)) {
    throw Object.assign(new Error('webview_engine_unavailable'), { category: 'webview_engine_unavailable' });
  }
  const muted = await call('set_volume', { volume: 0 });
  if (muted?.error) throw Object.assign(new Error('mute_failed'), { category: 'mcp_failure' });
  await call('play_tracks', { trackIds: [trackId] });
  started = true;

  const deadline = Date.now() + 10_000;
  let first = null;
  while (Date.now() < deadline) {
    const [playback, media] = await Promise.all([
      call('playback_state', { offset: 0, limit: 1 }),
      call('inspect_webview_media'),
    ]);
    const errorCode = media?.media?.errorCode;
    if (errorCode === 4) throw Object.assign(new Error('unsupported_media'), { category: 'unsupported_media' });
    if (errorCode === 2) throw Object.assign(new Error('source_failure'), { category: 'source_failure' });
    if (errorCode === 3) throw Object.assign(new Error('decode_failure'), { category: 'decode_failure' });
    if (sampleReady(media, playback)) {
      first = { ...media, playbackStatus: playback.status };
      break;
    }
    if (playback?.trackId && playback.trackId !== trackId) {
      throw Object.assign(new Error('wrong_playback_track'), { category: 'wrong_playback_track' });
    }
    await sleep(150);
  }
  if (!first) throw Object.assign(new Error('audio_not_ready'), { category: 'audio_not_ready' });
  await sleep(900);
  const [playback, second] = await Promise.all([
    call('playback_state', { offset: 0, limit: 1 }),
    call('inspect_webview_media'),
  ]);
  const errorCode = second?.media?.errorCode;
  if (errorCode === 4) throw Object.assign(new Error('unsupported_media'), { category: 'unsupported_media' });
  if (errorCode === 2) throw Object.assign(new Error('source_failure'), { category: 'source_failure' });
  if (errorCode === 3) throw Object.assign(new Error('decode_failure'), { category: 'decode_failure' });
  if (!sampleReady(second, playback)) {
    throw Object.assign(new Error('audio_state_not_playing'), { category: 'audio_state_not_playing' });
  }
  process.stdout.write(`${JSON.stringify({ status: 'sampled', first,
    second: { ...second, playbackStatus: playback.status } })}\n`);
} catch (error) {
  process.stdout.write(`${JSON.stringify({ status: 'failed', failureCategory: error?.category ?? 'mcp_failure' })}\n`);
  process.exitCode = 1;
} finally {
  if (started) await call('pause_playback').catch(() => {});
  await client.close().catch(() => {});
}
