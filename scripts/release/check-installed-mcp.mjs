import { assessFrontendLayout, remainingCheckTimeout } from './native-audio-protocol.mjs';
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';

const [command, discoveryPath, ...commandArgs] = process.argv.slice(2);
if (!command || !discoveryPath) {
  console.error('usage: check-installed-mcp.mjs MCP_COMMAND AUTOMATION_FILE');
  process.exit(2);
}

const client = new Client({ name: 'burrata-installed-smoke', version: '1.0.0' });
const transport = new StdioClientTransport({
  command,
  args: commandArgs,
  cwd: process.cwd(),
  env: { ...process.env, BURRATA_AUTOMATION_FILE: discoveryPath },
  stderr: 'pipe',
});
transport.stderr?.on('data', () => {});
const deadline = Date.now() + 25_000;
const timeout = setTimeout(() => {
  void client.close().finally(() => {
    console.error('installed MCP status check exceeded 25 seconds');
    process.exit(1);
  });
}, 25_000);
timeout.unref();

try {
  await client.connect(transport);
  const listing = await client.listTools();
  if (!['get_app_status', 'inspect_playback_layout'].every(name => listing.tools.some(tool => tool.name === name))) {
    throw new Error('installed MCP registry is missing status or actual frontend inspection');
  }
  let appStatus;
  while (Date.now() < deadline) {
    const response = await client.callTool({ name: 'get_app_status', arguments: {} }, {timeout:remainingCheckTimeout(deadline)});
    appStatus = response.structuredContent?.result;
    if (response.isError !== true && appStatus?.running === true
        && typeof appStatus.playback?.paused === 'boolean'
        && Array.isArray(appStatus.playback.queue)) break;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  if (appStatus?.running !== true || typeof appStatus.playback?.paused !== 'boolean'
      || !Array.isArray(appStatus.playback.queue)) {
    throw new Error('installed MCP did not report a running backend');
  }
  let frontend;
  while (Date.now() < deadline) {
    try {
      frontend = assessFrontendLayout(await client.callTool({name:'inspect_playback_layout',arguments:{}}, {timeout:remainingCheckTimeout(deadline)}));
      break;
    } catch { await new Promise(resolve => setTimeout(resolve, 250)); }
  }
  if (!frontend) throw new Error('installed MCP did not receive an actual frontend layout response');
  process.stdout.write(`${JSON.stringify({ initialized: true, toolCount: listing.tools.length, statusRunning: appStatus.running, ...frontend })}\n`);
} finally {
  clearTimeout(timeout);
  await client.close().catch(() => {});
}
