import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Client } from '@modelcontextprotocol/client';
import { buildMcpBundle } from '../scripts/build_mcp.mjs';

function linkedTransports() {
  const make = () => ({ onclose: undefined, onerror: undefined, onmessage: undefined, peer: undefined,
    async start() {},
    async send(message) { queueMicrotask(() => this.peer.onmessage?.(message)); },
    async close() { this.onclose?.(); this.peer.onclose?.(); },
  });
  const left = make();
  const right = make();
  left.peer = right;
  right.peer = left;
  return [left, right];
}

test('packaged MCP CJS bridge registers and calls a tool through the SDK', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'burrata-mcp-bundle-'));
  const discoveryPath = join(directory, 'automation.json');
  const token = 'b'.repeat(48);
  const previousFetch = globalThis.fetch;
  let client;
  let server;
  let callRequest;
  try {
    const bundlePath = await buildMcpBundle();
    const expectedBundle = resolve(fileURLToPath(new URL('../src-tauri/resources/mcp/server.cjs', import.meta.url)));
    assert.equal(bundlePath, expectedBundle);
    const licenseNotice = await readFile(`${bundlePath}.LEGAL.txt`, 'utf8');
    assert.match(licenseNotice, /MIT Licensed/);
    const { createServer } = createRequire(import.meta.url)(bundlePath);
    assert.equal(typeof createServer, 'function');

    await writeFile(discoveryPath, JSON.stringify({ url: 'http://127.0.0.1:43210/automation', token }));
    globalThis.fetch = async (url, options = {}) => {
      assert.equal(options.headers.authorization, `Bearer ${token}`);
      callRequest = { url: String(url), body: JSON.parse(options.body) };
      return new Response(JSON.stringify({
        content: [{ type: 'text', text: 'received packaged' }],
        structuredContent: { value: 'packaged' },
      }), { status: 200, headers: { 'content-type': 'application/json' } });
    };

    server = createServer([{
      name: 'bundle_probe',
      description: 'Probe the bundled Burrata MCP bridge.',
      inputSchema: { type: 'object', properties: { value: { type: 'string' } }, required: ['value'], additionalProperties: false },
      scope: 'read',
    }], discoveryPath);
    const [clientTransport, serverTransport] = linkedTransports();
    await server.connect(serverTransport);
    client = new Client({ name: 'burrata-mcp-bundle-test', version: '1.0.0' });
    await client.connect(clientTransport);
    const listing = await client.listTools();
    assert.deepEqual(listing.tools.map((tool) => tool.name), ['bundle_probe']);
    const result = await client.callTool({ name: 'bundle_probe', arguments: { value: 'packaged' } });
    assert.equal(result.content[0].text, 'received packaged');
    assert.deepEqual(result.structuredContent, { value: 'packaged' });
    assert.deepEqual(callRequest, {
      url: 'http://127.0.0.1:43210/automation/call',
      body: { name: 'bundle_probe', arguments: { value: 'packaged' } },
    });
  } finally {
    globalThis.fetch = previousFetch;
    await client?.close().catch(() => {});
    await server?.close().catch(() => {});
    await rm(directory, { recursive: true, force: true });
  }
});
