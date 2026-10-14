import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer as createHttpServer } from 'node:http';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { once } from 'node:events';
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { apiRequest, CALL_TIMEOUT_MS, defaultDiscoveryPath, MAX_RESULT_TEXT_CHARS, requestErrorCode, requestTimeoutFor } from './server.js';

const token = 'a'.repeat(48);
const schema = {
  type: 'object',
  properties: { query: { type: 'string', minLength: 1 } },
  required: ['query'],
  additionalProperties: false,
};

test('stdio proxy registers app tools and forwards authenticated calls using the MCP SDK client', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'burrata-mcp-'));
  const discoveryPath = join(directory, 'automation.json');
  const requests = [];
  let mode = 'ok';
  const app = createHttpServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : undefined;
    requests.push({ path: request.url, authorization: request.headers.authorization, body });
    response.setHeader('content-type', 'application/json');
    if (request.headers.authorization !== `Bearer ${token}`) {
      response.writeHead(401).end(JSON.stringify({ code: 'UNAUTHORIZED', message: 'Invalid bearer token.' }));
      return;
    }
    if (request.url === '/automation/tools' && request.method === 'GET') {
      response.end(JSON.stringify({ tools: [{ name: 'library_search', description: 'Search the local music library.', inputSchema: schema, scope: 'read' }] }));
      return;
    }
    if (request.url === '/automation/call' && request.method === 'POST') {
      if (mode === 'offline') {
        response.writeHead(503).end(JSON.stringify({ code: 'PLAYER_NOT_RUNNING', message: 'Start Burrata.' }));
        return;
      }
      if (body.arguments.query === 'leaky') {
        response.end(JSON.stringify({ content: [{ type: 'text', text: `Accidental token echo: ${token}` }], structuredContent: { echoed: token } }));
        return;
      }
      if (body.arguments.query === 'verbose') {
        response.end(JSON.stringify({ content: [{ type: 'text', text: 'x'.repeat(MAX_RESULT_TEXT_CHARS + 100) }], structuredContent: { query: 'verbose' } }));
        return;
      }
      response.end(JSON.stringify({
        content: [{ type: 'text', text: `Found results for ${body.arguments.query}` }],
        structuredContent: { query: body.arguments.query, count: 3 },
      }));
      return;
    }
    response.writeHead(404).end(JSON.stringify({ code: 'NOT_FOUND' }));
  });

  app.listen(0, '127.0.0.1');
  await once(app, 'listening');
  const { port } = app.address();
  await writeFile(discoveryPath, JSON.stringify({ url: `http://127.0.0.1:${port}/automation`, token }));
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [resolve('mcp/server.js')],
    cwd: process.cwd(),
    env: { ...process.env, BURRATA_AUTOMATION_FILE: discoveryPath },
    stderr: 'pipe',
  });
  const stderrChunks = [];
  transport.stderr?.on('data', (chunk) => stderrChunks.push(Buffer.from(chunk)));
  const client = new Client({ name: 'burrata-mcp-test', version: '1.0.0' });

  try {
    await client.connect(transport);
    const listing = await client.listTools();
    assert.equal(listing.tools.length, 1);
    assert.equal(listing.tools[0].name, 'library_search');
    assert.deepEqual(listing.tools[0].inputSchema.required, ['query']);
    assert.equal(listing.tools[0].annotations.readOnlyHint, true);

    const requestCount = requests.length;
    const invalid = await client.callTool({ name: 'library_search', arguments: {} });
    assert.equal(invalid.isError, true);
    assert.equal(requests.length, requestCount);

    const result = await client.callTool({ name: 'library_search', arguments: { query: 'ambient' } });
    assert.equal(result.isError, undefined);
    assert.equal(result.structuredContent.query, 'ambient');
    assert.equal(result.structuredContent.count, 3);
    assert.equal(result.content[0].text, 'Found results for ambient');
    assert.ok(requests.every((request) => request.authorization === `Bearer ${token}`));
    assert.deepEqual(requests[1].body, { name: 'library_search', arguments: { query: 'ambient' } });

    const redacted = await client.callTool({ name: 'library_search', arguments: { query: 'leaky' } });
    assert.ok(!JSON.stringify(redacted).includes(token));

    const verbose = await client.callTool({ name: 'library_search', arguments: { query: 'verbose' } });
    assert.ok(verbose.content[0].text.length <= MAX_RESULT_TEXT_CHARS + 100);
    assert.ok(verbose.content[0].text.endsWith('[Text shortened; structured content contains the complete result.]'));

    mode = 'offline';
    const offline = await client.callTool({ name: 'library_search', arguments: { query: 'offline' } });
    assert.equal(offline.isError, true);
    assert.equal(offline.structuredContent.code, 'PLAYER_NOT_RUNNING');

    await writeFile(discoveryPath, JSON.stringify({ url: `http://127.0.0.1:${port}/automation`, token: 'b'.repeat(48) }));
    const unauthorized = await client.callTool({ name: 'library_search', arguments: { query: 'unauthorized' } });
    assert.equal(unauthorized.isError, true);
    assert.equal(unauthorized.structuredContent.code, 'NETWORK_UNAVAILABLE');
    assert.ok(!JSON.stringify(unauthorized).includes(token));
    assert.ok(!Buffer.concat(stderrChunks).toString().includes(token));
  } finally {
    await client.close().catch(() => {});
    await new Promise((resolveClose) => app.close(resolveClose));
    await rm(directory, { recursive: true, force: true });
  }
});

test('automation discovery defaults to the Tauri app data directory', () => {
  assert.equal(defaultDiscoveryPath({}, 'linux', '/home/tester'), '/home/tester/.local/share/org.burrata.discovery/automation.json');
  assert.equal(defaultDiscoveryPath({ XDG_DATA_HOME: '/data/tester' }, 'linux', '/home/tester'), '/data/tester/org.burrata.discovery/automation.json');
  assert.equal(defaultDiscoveryPath({ APPDATA: 'C:\\Users\\tester\\AppData\\Roaming' }, 'win32', 'C:\\Users\\tester'), join('C:\\Users\\tester\\AppData\\Roaming', 'org.burrata.discovery', 'automation.json'));
});

test('timeouts and unavailable app errors stay distinct without waiting for the production deadline', async () => {
  assert.equal(requestTimeoutFor('/automation/tools'), 5_000);
  assert.equal(requestTimeoutFor('/automation/call'), CALL_TIMEOUT_MS);
  assert.equal(CALL_TIMEOUT_MS, 65_000);
  assert.equal(requestErrorCode(Object.assign(new TypeError('fetch failed'), { cause: Object.assign(new Error('refused'), { code: 'ECONNREFUSED' }) })), 'PLAYER_NOT_RUNNING');
  assert.equal(requestErrorCode(Object.assign(new TypeError('fetch failed'), { cause: Object.assign(new Error('reset'), { code: 'ECONNRESET' }) })), 'NETWORK_UNAVAILABLE');

  const discovery = { baseUrl: 'http://127.0.0.1:1234', token };
  const fetchImpl = (_url, { signal }) => new Promise((_, reject) => {
    signal.addEventListener('abort', () => reject(signal.reason), { once: true });
  });
  await assert.rejects(
    apiRequest(discovery, '/automation/call', {}, { timeoutMs: 2, fetchImpl }),
    (error) => requestErrorCode(error) === 'REQUEST_TIMEOUT',
  );
});
