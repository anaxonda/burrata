// Finite, app-specific UI control; no desktop focus or input events.
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { writeFile } from 'node:fs/promises';
const [automationFile, panel, outputFile] = process.argv.slice(2);
if (!automationFile || !['queue', 'info', 'closed'].includes(panel) || !outputFile) {
  console.error('usage: set-player-panel.mjs AUTOMATION_FILE queue|info|closed OUTPUT_JSON');
  process.exit(2);
}
const root = new URL('../../', import.meta.url).pathname;
const client = new Client({ name: 'burrata-player-panel-check', version: '1.0.0' });
const transport = new StdioClientTransport({ command: 'bash', args: ['scripts/dev/mcp-stdio.sh'], cwd: root,
  env: { ...process.env, BURRATA_AUTOMATION_FILE: automationFile }, stderr: 'pipe' });
transport.stderr?.on('data', () => {});
try {
  await client.connect(transport);
  const response = await client.callTool({ name: 'set_player_panel', arguments: { panel } }, { timeout: 10_000 });
  const report = response.structuredContent?.result ?? response.structuredContent;
  if (response.isError || report?.status !== 'complete' || report?.panel !== panel) throw new Error('panel_control_failed');
  const safe = { status: 'complete', panel };
  await writeFile(outputFile, JSON.stringify(safe, null, 2) + '\n');
  console.log(JSON.stringify(safe));
} catch {
  console.error('Player panel control did not complete. No desktop input was used.');
  process.exitCode = 1;
} finally { await client.close().catch(() => {}); }
