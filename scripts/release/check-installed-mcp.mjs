import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';

const [command, discoveryPath] = process.argv.slice(2);
if (!command || !discoveryPath) {
  console.error('usage: check-installed-mcp.mjs MCP_COMMAND AUTOMATION_FILE');
  process.exit(2);
}

const client = new Client({ name: 'burrata-installed-smoke', version: '1.0.0' });
const transport = new StdioClientTransport({
  command,
  args: [],
  cwd: process.cwd(),
  env: { ...process.env, BURRATA_AUTOMATION_FILE: discoveryPath },
  stderr: 'pipe',
});
transport.stderr?.on('data', () => {});
const timeout = setTimeout(() => {
  void client.close().finally(() => {
    console.error('installed MCP status check exceeded 20 seconds');
    process.exit(1);
  });
}, 20_000);
timeout.unref();

try {
  await client.connect(transport);
  const listing = await client.listTools();
  if (!listing.tools.some((tool) => tool.name === 'get_app_status')) {
    throw new Error('installed MCP registry is missing get_app_status');
  }
  const deadline = Date.now() + 15_000;
  let appStatus;
  while (Date.now() < deadline) {
    const response = await client.callTool({ name: 'get_app_status', arguments: {} });
    appStatus = response.structuredContent?.result;
    if (response.isError !== true && appStatus?.running === true
        && typeof appStatus.playback?.paused === 'boolean'
        && Array.isArray(appStatus.playback.queue)) break;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  if (appStatus?.running !== true || typeof appStatus.playback?.paused !== 'boolean'
      || !Array.isArray(appStatus.playback.queue)) {
    throw new Error('installed MCP did not report a running app with a mounted frontend');
  }
  process.stdout.write(`${JSON.stringify({ initialized: true, toolCount: listing.tools.length, statusRunning: appStatus.running, frontendMounted: true })}\n`);
} finally {
  clearTimeout(timeout);
  await client.close().catch(() => {});
}
