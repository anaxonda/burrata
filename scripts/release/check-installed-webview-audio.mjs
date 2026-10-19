import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { createWebviewAudioSmokeGate, webviewMediaProbeConfirmsEngine } from './webview-audio-protocol.mjs';

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

let started = false;
let gate = null;
async function inspectMediaSample() {
  const media = await call('inspect_webview_media');
  return { media, receivedAtMs: performance.now() };
}
function diagnostics() {
  return gate?.diagnostics(performance.now()) ?? {};
}
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

  gate = createWebviewAudioSmokeGate(trackId, performance.now());
  while (performance.now() < gate.startupDeadlineMs) {
    const [playback, { media, receivedAtMs }] = await Promise.all([
      call('playback_state', { offset: 0, limit: 1 }),
      inspectMediaSample(),
    ]);
    if (gate.observe(playback, media, receivedAtMs) === 'steady') break;
    await sleep(150);
  }
  if (gate.phase !== 'steady') gate.expireStartup();
  await sleep(900);
  const [playback, { media, receivedAtMs }] = await Promise.all([
    call('playback_state', { offset: 0, limit: 1 }),
    inspectMediaSample(),
  ]);
  gate.observe(playback, media, receivedAtMs);
  process.stdout.write(`${JSON.stringify({ status: 'sampled', ...diagnostics() })}\n`);
} catch (error) {
  process.stdout.write(`${JSON.stringify({ status: 'failed', failureCategory: error?.category ?? 'mcp_failure', ...diagnostics() })}\n`);
  process.exitCode = 1;
} finally {
  if (started) await call('pause_playback').catch(() => {});
  await client.close().catch(() => {});
}
