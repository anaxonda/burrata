// App-specific read check; never prints opaque cursors or result metadata.
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { writeFile } from 'node:fs/promises';
const [automationFile, output] = process.argv.slice(2);
if (!automationFile || !output) throw new Error('automation file and output required');
const root = new URL('../../', import.meta.url).pathname;
const client = new Client({ name: 'discovery-cursor-check', version: '1.0.0' });
const transport = new StdioClientTransport({ command: 'bash', args: ['scripts/dev/mcp-stdio.sh'],
  cwd: root, env: { ...process.env, BURRATA_AUTOMATION_FILE: automationFile }, stderr: 'pipe' });
transport.stderr?.on('data', () => {});
try {
  await client.connect(transport);
  const registry = await client.listTools();
  if (!registry.tools.some(tool => tool.name === 'discover_music')) throw new Error('discovery tool unavailable');
  const args = { query: "poso s'agapo", sourceIds: ['youtube'], kinds: ['track', 'playlist', 'channel'], limit: 1 };
  const call = async (arguments_) => {
    const response = await client.callTool({ name: 'discover_music', arguments: arguments_ }, { timeout: 60_000 });
    if (response.isError) throw new Error('discovery request failed');
    const result = response.structuredContent?.result ?? response.structuredContent;
    if (!Array.isArray(result?.pages)) throw new Error('missing pages');
    return result.pages.find(page => page.sourceId === 'youtube');
  };
  const first = await call(args);
  if (!first?.nextCursor || first.nextCursor.length <= 100) throw new Error('long buffered cursor not exercised');
  const next = await call({ ...args, cursor: first.nextCursor });
  if (!next || next.results.length !== 1 || next.complete !== first.complete ||
      JSON.stringify(next.warnings) !== JSON.stringify(first.warnings)) throw new Error('page status mismatch');
  const report = { ok: true, cursorLength: first.nextCursor.length,
    firstCount: first.results.length, nextCount: next.results.length,
    partial: !first.complete, warningsPreserved: true, desktopInput: false };
  await writeFile(output, JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report));
} catch {
  console.error('Discovery cursor verification did not complete; no desktop input was used.');
  process.exitCode = 1;
} finally { await client.close().catch(() => {}); }
