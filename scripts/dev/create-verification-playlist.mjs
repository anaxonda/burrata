import { writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve, dirname } from 'node:path';
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';

const [automationFile, provider, output, ...extra] = process.argv.slice(2);
if (!automationFile || !output || extra.length || !['spotify', 'youtube'].includes(provider)) {
  throw new Error('Usage: node scripts/dev/create-verification-playlist.mjs AUTOMATION_FILE spotify|youtube OUTPUT_JSON');
}
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const client = new Client({ name: 'burrata-verification-playlist', version: '1.0.0' });
const transport = new StdioClientTransport({ command: process.execPath, args: [resolve(root, 'mcp/server.js')], cwd: root,
  env: { ...process.env, BURRATA_AUTOMATION_FILE: resolve(automationFile) }, stderr: 'pipe' });
const deadline = setTimeout(() => {
  process.stderr.write('Creation deadline exceeded; inspect the provider and attempt marker. DO NOT RETRY creation.\n');
  process.exit(1);
}, 125000);
try {
  await client.connect(transport);
  const registry = await client.listTools();
  if (!registry.tools.some(tool => tool.name === 'create_verification_playlist')) throw new Error('The running app does not expose create_verification_playlist');
  const response = await client.callTool({ name: 'create_verification_playlist', arguments: { provider } }, undefined, { timeout: 115000 });
  if (response.isError) {
    const text = (response.content || []).filter(item => item.type === 'text').map(item => item.text).join('\n').slice(0, 4096);
    // Native errors contain only finite diagnostics; never save raw provider responses here.
    const diagnostic = text.match(/Diagnostic: (\{[^\n]{1,512}\})/)?.[1] || null;
    await writeFile(resolve(output), JSON.stringify({ provider, status: 'UNKNOWN_OUTCOME', diagnostic }, null, 2) + '\n', { mode: 0o600 });
    throw new Error(`Creation failed or has an unknown outcome. Inspect the provider and attempt marker; DO NOT RETRY creation.${diagnostic ? ` Diagnostic: ${diagnostic}` : ''}`);
  }
  const result = response.structuredContent?.result;
  if (!result || result.provider !== provider || result.name !== 'Burrata verification' || result.visibility !== 'private' || result.total !== 0 || result.verified !== true
      || typeof result.playlistId !== 'string' || !/^[A-Za-z0-9_-]{1,256}$/.test(result.playlistId)) throw new Error('Creation receipt could not be verified; DO NOT RETRY creation.');
  const url = provider === 'spotify' ? `https://open.spotify.com/playlist/${result.playlistId}` : `https://www.youtube.com/playlist?list=${result.playlistId}`;
  const receipt = { provider, playlistId: result.playlistId, name: result.name, visibility: 'private', total: 0, verified: true, url };
  await writeFile(resolve(output), JSON.stringify(receipt, null, 2) + '\n', { mode: 0o600 });
  process.stdout.write(JSON.stringify(receipt) + '\n');
} finally {
  clearTimeout(deadline);
  await client.close();
}
