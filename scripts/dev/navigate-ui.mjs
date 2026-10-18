// Finite Burrata route control through authenticated MCP; no desktop input.
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { writeFile } from 'node:fs/promises';

const [automationFile, target, outputFile] = process.argv.slice(2);
const views = ['library', 'favorites', 'history', 'downloads', 'settings'];
const artist = typeof target === 'string' ? target.match(/^spotifyArtist:([A-Za-z0-9]{22})$/)?.[1] : null;
if (!automationFile || (!views.includes(target) && !artist) || !outputFile) {
  console.error('usage: navigate-ui.mjs AUTOMATION_FILE library|favorites|history|downloads|settings|spotifyArtist:ID OUTPUT_JSON');
  process.exit(2);
}
const root = new URL('../../', import.meta.url).pathname;
const client = new Client({ name: 'burrata-route-check', version: '1.0.0' });
const transport = new StdioClientTransport({ command: 'bash', args: ['scripts/dev/mcp-stdio.sh'], cwd: root,
  env: { ...process.env, BURRATA_AUTOMATION_FILE: automationFile }, stderr: 'pipe' });
transport.stderr?.on('data', () => {});
try {
  await client.connect(transport);
  const response = await client.callTool({ name: 'navigate_ui', arguments: artist ? { spotifyArtistId: artist } : { view: target } }, { timeout: 20_000 });
  const report = response.structuredContent?.result ?? response.structuredContent;
  if (response.isError || report?.status !== 'complete' || report?.destination !== (artist || target)) throw new Error('route_control_failed');
  const safe = { status: 'complete', targetKind: artist ? 'spotify_artist' : 'view', destination: artist || target, providerDataReady: 'not_measured' };
  await writeFile(outputFile, JSON.stringify(safe, null, 2) + '\n');
  console.log(JSON.stringify(safe));
} catch {
  console.error('Burrata route control did not complete.');
  process.exitCode = 1;
} finally { await client.close().catch(() => {}); }
