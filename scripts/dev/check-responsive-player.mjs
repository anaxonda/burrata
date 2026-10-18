// Resize an explicitly selected desktop instance, inspect geometry, then restore it.
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { writeFile } from 'node:fs/promises';
const [automationFile, outputFile] = process.argv.slice(2);
if (!automationFile || !outputFile) throw new Error('usage: check-responsive-player.mjs AUTOMATION_FILE OUTPUT_JSON');
const root = new URL('../../', import.meta.url).pathname;
const client = new Client({ name: 'burrata-responsive-player-check', version: '1.0.0' });
const transport = new StdioClientTransport({ command: 'bash', args: ['scripts/dev/mcp-stdio.sh'], cwd: root,
  env: { ...process.env, BURRATA_AUTOMATION_FILE: automationFile }, stderr: 'pipe' });
transport.stderr?.on('data', () => {});
async function call(name, args) {
  const response = await client.callTool({ name, arguments: args }, { timeout: 10_000 });
  if (response.isError) throw new Error('desktop_tool_failed');
  return response.structuredContent?.result ?? response.structuredContent;
}
let original, restored = false;
const reports = [];
try {
  await client.connect(transport);
  for (const [width, height] of [[500, 800], [360, 640], [1000, 650]]) {
    const resize = await call('resize_app_window', { width, height, restoreMaximized: false });
    original ??= resize.before;
    const layout = await call('inspect_playback_layout', {});
    const requestedSizeApplied = Math.abs(layout.viewport.width - width) <= 1 && Math.abs(layout.viewport.height - height) <= 1;
    reports.push({ requested: { width, height }, actual: resize.after, requestedSizeApplied, layout });
  }
} finally {
  try {
    if (original) {
      const result = await call('resize_app_window', { width: Math.round(original.innerSize.width), height: Math.round(original.innerSize.height), restoreMaximized: original.maximized });
      restored = result.after.maximized === original.maximized && (original.maximized || Math.abs(result.after.innerSize.width - original.innerSize.width) <= 1 && Math.abs(result.after.innerSize.height - original.innerSize.height) <= 1);
    }
    await writeFile(outputFile, JSON.stringify({ schemaVersion: 1, original, restored, reports }, null, 2) + '\n');
    console.log(JSON.stringify({ restored, checks: reports.map(({ requested, layout }) => ({ requested, viewport: layout.viewport, findings: layout.findings })) }));
  } finally { await client.close().catch(() => {}); }
}
if (!restored || reports.length !== 3 || reports.some(({layout, requestedSizeApplied}) => !requestedSizeApplied || layout.findings.length)) process.exitCode = 1;
