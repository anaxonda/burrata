// Read-only geometry verification for an explicitly selected Burrata instance.
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { writeFile } from 'node:fs/promises';
const [automationFile, outputFile] = process.argv.slice(2);
if (!automationFile || !outputFile) {
  console.error('usage: inspect-playback-layout.mjs AUTOMATION_FILE OUTPUT_JSON');
  process.exit(2);
}
const root = new URL('../../', import.meta.url).pathname;
const client = new Client({ name: 'burrata-player-layout-check', version: '1.0.0' });
const transport = new StdioClientTransport({ command: 'bash', args: [`${root}scripts/dev/mcp-stdio.sh`], cwd: root,
  env: { ...process.env, BURRATA_AUTOMATION_FILE: automationFile }, stderr: 'pipe' });
transport.stderr?.on('data', () => {});
try {
  await client.connect(transport);
  const response = await client.callTool({ name: 'inspect_playback_layout', arguments: {} }, { timeout: 10_000 });
  const layout = response.structuredContent?.result ?? response.structuredContent;
  if (response.isError || layout?.schemaVersion !== 1 || !Array.isArray(layout?.findings)) {
    throw new Error('layout_unavailable');
  }
  await writeFile(outputFile, `${JSON.stringify(layout, null, 2)}\n`);
  console.log(JSON.stringify({ viewport: layout.viewport, findings: layout.findings, footerHeight: layout.footerHeight }));
} catch {
  console.error('Player layout inspection is unavailable. Check that the current desktop build is connected.');
  process.exitCode = 1;
} finally {
  await client.close().catch(() => {});
}
