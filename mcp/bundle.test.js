import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { buildMcpBundle } from '../scripts/build_mcp.mjs';

test('bundled MCP CJS bridge speaks the SDK stdio protocol and forwards a tool call', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'burrata-mcp-bundle-'));
  const discoveryPath = join(directory, 'automation.json');
  const preloadPath = join(directory, 'mock-fetch.cjs');
  const requestsPath = join(directory, 'requests.jsonl');
  const token = 'b'.repeat(48);
  await writeFile(discoveryPath, JSON.stringify({ url: 'http://127.0.0.1:1/automation', token }));
  await writeFile(preloadPath, `
    const fs = require('node:fs');
    const requestLog = ${JSON.stringify(requestsPath)};
    const token = ${JSON.stringify(`Bearer ${token}`)};
    global.fetch = async (url, options = {}) => {
      const headers = options.headers || {};
      fs.appendFileSync(requestLog, JSON.stringify({ url, authorization: headers.authorization, body: options.body || null }) + '\\n');
      let payload;
      let status = 200;
      if (headers.authorization !== token) {
        status = 401;
        payload = { code: 'UNAUTHORIZED' };
      } else if (String(url).endsWith('/automation/tools')) {
        payload = { tools: [{
          name: 'bundle_probe',
          description: 'Probe the bundled Burrata MCP bridge.',
          inputSchema: { type: 'object', properties: { value: { type: 'string' } }, required: ['value'], additionalProperties: false },
          scope: 'read',
        }] };
      } else if (String(url).endsWith('/automation/call')) {
        const body = JSON.parse(options.body);
        payload = { content: [{ type: 'text', text: 'received ' + body.arguments.value }], structuredContent: { value: body.arguments.value } };
      } else {
        status = 404;
        payload = { code: 'NOT_FOUND' };
      }
      return new Response(JSON.stringify(payload), { status, headers: { 'content-type': 'application/json' } });
    };
  `);

  let client;
  let transport;
  const stderrChunks = [];
  try {
    const bundlePath = await buildMcpBundle();
    assert.equal(bundlePath, resolve(fileURLToPath(new URL('../src-tauri/resources/mcp/server.cjs', import.meta.url))));
    transport = new StdioClientTransport({
      command: process.execPath,
      args: [bundlePath],
      cwd: process.cwd(),
      env: { ...process.env, BURRATA_AUTOMATION_FILE: discoveryPath, NODE_OPTIONS: `--require=${preloadPath}` },
      stderr: 'pipe',
    });
    transport.stderr?.on('data', (chunk) => stderrChunks.push(Buffer.from(chunk)));
    transport.onerror = (error) => stderrChunks.push(Buffer.from(String(error)));
    client = new Client({ name: 'burrata-mcp-bundle-test', version: '1.0.0' });
    try {
      await client.connect(transport);
    } catch (error) {
      throw new Error(`${error}\n${Buffer.concat(stderrChunks).toString()}`);
    }
    const listing = await client.listTools();
    assert.deepEqual(listing.tools.map((tool) => tool.name), ['bundle_probe']);
    const result = await client.callTool({ name: 'bundle_probe', arguments: { value: 'packaged' } });
    assert.equal(result.content[0].text, 'received packaged');
    assert.deepEqual(result.structuredContent, { value: 'packaged' });
    const requests = (await readFile(requestsPath, 'utf8')).trim().split('\n').map((line) => JSON.parse(line));
    assert.deepEqual(requests.map((request) => new URL(request.url).pathname), ['/automation/tools', '/automation/call']);
    assert.ok(requests.every((request) => request.authorization === `Bearer ${token}`));
    assert.deepEqual(JSON.parse(requests[1].body), { name: 'bundle_probe', arguments: { value: 'packaged' } });
  } finally {
    if (client) await client.close().catch(() => {});
    else await transport?.close().catch(() => {});
    await rm(directory, { recursive: true, force: true });
  }
});
