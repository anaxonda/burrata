#!/usr/bin/env node
import { build } from 'esbuild';
import { mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { pathToFileURL } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const output = resolve(root, 'src-tauri/resources/mcp/server.cjs');

export async function buildMcpBundle() {
  await mkdir(dirname(output), { recursive: true });
  await build({
    entryPoints: [resolve(root, 'mcp/bundle-entry.js')],
    outfile: output,
    bundle: true,
    platform: 'node',
    format: 'cjs',
    target: 'node24',
    packages: 'bundle',
    sourcemap: false,
    legalComments: 'linked',
  });
  return output;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  console.log(`Bundled Burrata MCP bridge: ${await buildMcpBundle()}`);
}
