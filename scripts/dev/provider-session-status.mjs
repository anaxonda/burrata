// Read only the sanitized managed-session status exposed by Burrata's MCP API.
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
const [automationFile] = process.argv.slice(2);
if (!automationFile) {
  console.error('usage: provider-session-status.mjs AUTOMATION_FILE');
  process.exit(2);
}
const root = new URL('../../', import.meta.url).pathname;
const client = new Client({ name: 'burrata-provider-session-check', version: '1.0.0' });
const transport = new StdioClientTransport({ command: 'bash', args: [`${root}scripts/dev/mcp-stdio.sh`], cwd: root,
  env: { ...process.env, BURRATA_AUTOMATION_FILE: automationFile }, stderr: 'pipe' });
transport.stderr?.on('data', () => {});
try {
  await client.connect(transport);
  const response = await client.callTool({ name: 'get_app_status', arguments: {} }, { timeout: 20_000 });
  const status = response.structuredContent?.result ?? response.structuredContent;
  if (response.isError || !status?.managedProviderSession) throw new Error('status_unavailable');
  console.log(JSON.stringify(status.managedProviderSession));
} catch {
  console.error('Managed provider status is unavailable.');
  process.exitCode = 1;
} finally {
  await client.close().catch(() => {});
}
