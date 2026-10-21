import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { createWebviewAudioSmokeGate, waitForFrontendReady, webviewMediaProbeConfirmsEngine } from './webview-audio-protocol.mjs';

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
const MCP_CALL_TIMEOUT_MS = 65_000;
const safeErrorCodes = new Set([
  'TIMEOUT', 'FRONTEND_NOT_READY', 'STALE_MEDIA_SAMPLE', 'PERMISSION_DENIED',
  'INVALID_ARGUMENT', 'PLAYER_NOT_RUNNING', 'REQUEST_TIMEOUT', 'NETWORK_UNAVAILABLE',
  'RESULT_TOO_LARGE', 'BURRATA_UNAVAILABLE',
]);
let currentPhase = 'connect';
let failedTool = null;
let failureCode = null;
function safeCode(value) {
  return typeof value === 'string' && safeErrorCodes.has(value) ? value : null;
}
async function call(name, args = {}, timeoutMs = MCP_CALL_TIMEOUT_MS) {
  failureCode = null;
  failedTool = name;
  let response;
  try {
    response = await client.callTool({ name, arguments: args }, { timeout: timeoutMs });
  } catch (error) {
    failureCode = error?.name === 'TimeoutError' || error?.name === 'AbortError'
      ? 'MCP_CLIENT_TIMEOUT' : 'MCP_TRANSPORT_ERROR';
    throw Object.assign(new Error('mcp_failure'), { category: 'mcp_failure', code: failureCode });
  }
  if (response.isError === true) {
    failureCode = safeCode(response.structuredContent?.error?.code)
      ?? safeCode(response.structuredContent?.code)
      ?? 'MCP_TOOL_ERROR';
    throw Object.assign(new Error('mcp_failure'), { category: 'mcp_failure', code: failureCode });
  }
  failedTool = null;
  return response.structuredContent?.result ?? response.structuredContent ?? null;
}

let started = false;
let gate = null;
async function inspectMediaSample() {
  const media = await call('inspect_webview_media');
  return { media, receivedAtMs: performance.now() };
}
function diagnostics() {
  return {
    ...(gate?.diagnostics(performance.now()) ?? {}),
    ...(currentPhase ? { failurePhase: currentPhase } : {}),
    ...(failedTool ? { failureTool: failedTool } : {}),
    ...(failureCode ? { failureCode } : {}),
  };
}
try {
  await client.connect(transport);
  currentPhase = 'list_tools';
  const listing = await client.listTools();
  const required = ['get_app_status', 'playback_state', 'set_volume', 'play_tracks',
    'pause_playback', 'inspect_webview_media'];
  if (!required.every((name) => listing.tools.some((tool) => tool.name === name))) {
    throw Object.assign(new Error('unsupported_smoke_tools'), { category: 'unsupported_smoke_tools' });
  }
  currentPhase = 'app_status';
  const status = await call('get_app_status');
  if (status?.running !== true || status?.permissions?.read !== true
      || status?.permissions?.control !== true) {
    throw Object.assign(new Error('smoke_permissions_unavailable'), { category: 'smoke_permissions_unavailable' });
  }
  currentPhase = 'initial_playback_state';
  const initial = await call('playback_state', { offset: 0, limit: 1 });
  currentPhase = 'initial_media_sample';
  const initialMedia = await waitForFrontendReady(
    () => call('inspect_webview_media', {}, 7_000),
    { timeoutMs: 30_000, pollIntervalMs: 250, sleep },
  );
  if (!webviewMediaProbeConfirmsEngine(initial, initialMedia)) {
    throw Object.assign(new Error('webview_engine_unavailable'), { category: 'webview_engine_unavailable' });
  }
  currentPhase = 'mute';
  const muted = await call('set_volume', { volume: 0 });
  if (muted?.error) {
    failureCode = safeCode(muted.error.code) ?? 'MCP_TOOL_ERROR';
    failedTool = 'set_volume';
    throw Object.assign(new Error('mute_failed'), { category: 'mcp_failure' });
  }
  currentPhase = 'start_playback';
  await call('play_tracks', { trackIds: [trackId] });
  started = true;

  gate = createWebviewAudioSmokeGate(trackId, performance.now());
  currentPhase = 'startup_samples';
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
  currentPhase = 'steady_sample';
  const [playback, { media, receivedAtMs }] = await Promise.all([
    call('playback_state', { offset: 0, limit: 1 }),
    inspectMediaSample(),
  ]);
  gate.observe(playback, media, receivedAtMs);
  currentPhase = null;
  process.stdout.write(`${JSON.stringify({ status: 'sampled', ...diagnostics() })}\n`);
} catch (error) {
  process.stdout.write(`${JSON.stringify({ status: 'failed', failureCategory: error?.category ?? 'mcp_failure', ...diagnostics() })}\n`);
  process.exitCode = 1;
} finally {
  if (started) {
    currentPhase = 'cleanup_pause';
    await call('pause_playback').catch(() => {});
  }
  await client.close().catch(() => {});
}
