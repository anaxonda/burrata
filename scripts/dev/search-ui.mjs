// Operate only the explicitly selected Burrata app; never uses desktop focus/input.
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { writeFile } from 'node:fs/promises';
const [automationFile, sourceId, kind, query, outputFile] = process.argv.slice(2);
if (!automationFile || !sourceId || !kind || !query || !outputFile) {
  console.error('usage: search-ui.mjs AUTOMATION_FILE SOURCE KIND QUERY OUTPUT_JSON');
  process.exit(2);
}
const root = new URL('../../', import.meta.url).pathname;
const client = new Client({ name: 'burrata-search-ui-check', version: '1.0.0' });
const transport = new StdioClientTransport({ command: 'bash', args: ['scripts/dev/mcp-stdio.sh'], cwd: root,
  env: { ...process.env, BURRATA_AUTOMATION_FILE: automationFile }, stderr: 'pipe' });
transport.stderr?.on('data', () => {});
try {
  await client.connect(transport);
  const response = await client.callTool({ name: 'search_ui', arguments: { query, sourceId, kind } }, { timeout: 35_000 });
  const report = response.structuredContent?.result ?? response.structuredContent;
  if (response.isError || !['complete', 'failed', 'cancelled'].includes(report?.status)) throw new Error('search_ui_unavailable');
  await writeFile(outputFile, JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report));
  if (report.status !== 'complete') process.exitCode = 1;
} catch {
  console.error('App-specific search control did not complete. No desktop input was used.');
  process.exitCode = 1;
} finally { await client.close().catch(() => {}); }
