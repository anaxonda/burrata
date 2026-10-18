import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer as createHttpServer } from 'node:http';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, posix, resolve, win32 } from 'node:path';
import { once } from 'node:events';
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { apiRequest, CALL_TIMEOUT_MS, defaultDiscoveryPath, MAX_RESULT_TEXT_CHARS, requestErrorCode, requestTimeoutFor, PUBLIC_DURATION_REPAIR_TIMEOUT_MS } from './server.js';

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
  // The spawned production server must not inherit Node's test-worker protocol.
  const { NODE_TEST_CONTEXT: _testContext, ...serverEnvironment } = process.env;
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [resolve('mcp/server.js')],
    cwd: process.cwd(),
    env: { ...serverEnvironment, BURRATA_AUTOMATION_FILE: discoveryPath },
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

test('identify/cancel MCP tools preserve no-argument compatibility and forward one bounded caller UUID', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'burrata-identify-mcp-'));
  const discoveryPath = join(directory, 'automation.json');
  const requests = [];
  const app = createHttpServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : undefined;
    requests.push({ path: request.url, body });
    response.setHeader('content-type', 'application/json');
    if (request.url === '/automation/tools' && request.method === 'GET') {
      response.end(JSON.stringify({ tools: [
        { name: 'identify_playing_track', description: 'Identify current audio.', scope: 'control', inputSchema: {
          type: 'object', properties: { captureId: { type: 'string', pattern: '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' } }, required: [], additionalProperties: false,
        } },
        { name: 'cancel_playing_track_identification', description: 'Cancel by captureId.', scope: 'control', inputSchema: {
          type: 'object', properties: { captureId: { type: 'string', pattern: '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' } }, required: ['captureId'], additionalProperties: false,
        } },
      ] }));
      return;
    }
    if (request.url === '/automation/call' && request.method === 'POST') {
      response.end(JSON.stringify({ content: [{ type: 'text', text: 'ok' }], structuredContent: { accepted: true } }));
      return;
    }
    response.writeHead(404).end(JSON.stringify({ code: 'NOT_FOUND' }));
  });
  app.listen(0, '127.0.0.1');
  await once(app, 'listening');
  const { port } = app.address();
  await writeFile(discoveryPath, JSON.stringify({ url: `http://127.0.0.1:${port}/automation`, token }));
  const { NODE_TEST_CONTEXT: _testContext, ...serverEnvironment } = process.env;
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [resolve('mcp/server.js')],
    cwd: process.cwd(),
    env: { ...serverEnvironment, BURRATA_AUTOMATION_FILE: discoveryPath },
    stderr: 'pipe',
  });
  const client = new Client({ name: 'burrata-identify-cancel-test', version: '1.0.0' });
  try {
    await client.connect(transport);
    const uuid = '123e4567-e89b-12d3-a456-426614174000';
    const listing = await client.listTools();
    assert.deepEqual(listing.tools.map((tool) => tool.name), ['identify_playing_track', 'cancel_playing_track_identification']);
    assert.deepEqual(listing.tools[0].inputSchema.required, []);
    assert.deepEqual(listing.tools[1].inputSchema.required, ['captureId']);
    await client.callTool({ name: 'identify_playing_track', arguments: {} });
    await client.callTool({ name: 'identify_playing_track', arguments: { captureId: uuid } });
    await client.callTool({ name: 'cancel_playing_track_identification', arguments: { captureId: uuid } });
    const beforeInvalid = requests.length;
    const invalid = await client.callTool({ name: 'cancel_playing_track_identification', arguments: { captureId: 'not-a-uuid' } });
    assert.equal(invalid.isError, true);
    assert.equal(requests.length, beforeInvalid, 'invalid captureId must be rejected before app dispatch');
    assert.deepEqual(requests.filter((request) => request.path === '/automation/call').map((request) => request.body), [
      { name: 'identify_playing_track', arguments: {} },
      { name: 'identify_playing_track', arguments: { captureId: uuid } },
      { name: 'cancel_playing_track_identification', arguments: { captureId: uuid } },
    ]);
  } finally {
    await client.close().catch(() => {});
    await new Promise((resolveClose) => app.close(resolveClose));
    await rm(directory, { recursive: true, force: true });
  }
});

test('WebView media inspection is exposed as a read-only no-argument proxy tool', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'burrata-media-inspection-mcp-'));
  const discoveryPath = join(directory, 'automation.json');
  const requests = [];
  const app = createHttpServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : undefined;
    requests.push({ path: request.url, body });
    response.setHeader('content-type', 'application/json');
    if (request.url === '/automation/tools' && request.method === 'GET') {
      response.end(JSON.stringify({ tools: [{
        name: 'inspect_webview_media', description: 'Read current WebView audio-element state.', scope: 'read',
        inputSchema: { type: 'object', properties: {}, required: [], additionalProperties: false },
      }] }));
      return;
    }
    if (request.url === '/automation/call' && request.method === 'POST') {
      response.end(JSON.stringify({ content: [{ type: 'text', text: 'WebView media sample' }], structuredContent: {
        engine: 'webview', applicable: true, trackId: 'track-1', sessionId: 'session-1', sourceGeneration: 7, seekSerial: 2,
        currentIndex: 0, queueTotal: 1, entryId: null, previewKind: 'audio', media: { present: true, currentTime: 10.25, duration: 131, paused: false, readyState: 4, networkState: 2, errorCode: null, sourcePresent: true, sourceStable: true },
      } }));
      return;
    }
    response.writeHead(404).end();
  });
  app.listen(0, '127.0.0.1');
  await once(app, 'listening');
  const { port } = app.address();
  await writeFile(discoveryPath, JSON.stringify({ url: `http://127.0.0.1:${port}/automation`, token }));
  const { NODE_TEST_CONTEXT: _testContext, ...serverEnvironment } = process.env;
  const transport = new StdioClientTransport({ command: process.execPath, args: [resolve('mcp/server.js')], cwd: process.cwd(), env: { ...serverEnvironment, BURRATA_AUTOMATION_FILE: discoveryPath }, stderr: 'pipe' });
  const client = new Client({ name: 'burrata-media-inspection-test', version: '1.0.0' });
  try {
    await client.connect(transport);
    const listing = await client.listTools();
    assert.equal(listing.tools[0].annotations.readOnlyHint, true);
    assert.deepEqual(listing.tools[0].inputSchema.required, []);
    assert.deepEqual(listing.tools[0].inputSchema.properties, {});
    const result = await client.callTool({ name: 'inspect_webview_media', arguments: {} });
    assert.equal(result.structuredContent.media.currentTime, 10.25);
    const invalid = await client.callTool({ name: 'inspect_webview_media', arguments: { command: 'play' } });
    assert.equal(invalid.isError, true);
    assert.equal(requests.filter((item) => item.path === '/automation/call').length, 1);
    assert.deepEqual(requests.find((item) => item.path === '/automation/call').body, { name: 'inspect_webview_media', arguments: {} });
  } finally {
    await client.close().catch(() => {});
    await new Promise((resolveClose) => app.close(resolveClose));
    await rm(directory, { recursive: true, force: true });
  }
});

test('automation discovery defaults to the Tauri app data directory', () => {
  assert.equal(defaultDiscoveryPath({}, 'linux', '/home/tester'), posix.join('/home/tester', '.local', 'share', 'org.burrata.discovery', 'automation.json'));
  assert.equal(defaultDiscoveryPath({ XDG_DATA_HOME: '/data/tester' }, 'linux', '/home/tester'), posix.join('/data/tester', 'org.burrata.discovery', 'automation.json'));
  assert.equal(defaultDiscoveryPath({}, 'darwin', '/Users/tester'), posix.join('/Users/tester', 'Library', 'Application Support', 'org.burrata.discovery', 'automation.json'));
  assert.equal(defaultDiscoveryPath({ APPDATA: 'C:\\Users\\tester\\AppData\\Roaming' }, 'win32', 'C:\\Users\\tester'), win32.join('C:\\Users\\tester\\AppData\\Roaming', 'org.burrata.discovery', 'automation.json'));
});

test('timeouts and unavailable app errors stay distinct without waiting for the production deadline', async () => {
  assert.equal(requestTimeoutFor('/automation/tools'), 5_000);
  assert.equal(requestTimeoutFor('/automation/call'), CALL_TIMEOUT_MS);
  assert.equal(requestTimeoutFor('/automation/call', 'create_verification_playlist'), 110_000);
  assert.equal(requestTimeoutFor('/automation/call', 'repair_spotify_durations_public'), PUBLIC_DURATION_REPAIR_TIMEOUT_MS);
  assert.equal(PUBLIC_DURATION_REPAIR_TIMEOUT_MS, 75_000);
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

test('reviewed playlist MCP tools forward explicit tokens and reject raw candidate payloads', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'burrata-writeback-mcp-'));
  const discoveryPath = join(directory, 'automation.json');
  const requests = [];
  const app = createHttpServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : undefined;
    requests.push({ path: request.url, body });
    response.setHeader('content-type', 'application/json');
    if (request.url === '/automation/tools' && request.method === 'GET') {
      response.end(JSON.stringify({ tools: [
        { name: 'search_remote_playlist_writeback_candidates', description: 'Search candidates.', scope: 'read', inputSchema: {
          type: 'object', properties: { bindingId: { type:'string',minLength:1,maxLength:256 },localTrackId:{type:'string',minLength:1,maxLength:300} },required:['bindingId','localTrackId'],additionalProperties:false,
        } },
        { name:'get_playlist_sync_preview',description:'Preview reviewed matches.',scope:'read',inputSchema:{
          type:'object',properties:{bindingId:{type:'string',minLength:1,maxLength:256},reviewedMatches:{type:'array',maxItems:1000,items:{type:'object',properties:{localTrackId:{type:'string',minLength:1,maxLength:300},candidateToken:{type:'string',pattern:'^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'}},required:['localTrackId','candidateToken'],additionalProperties:false}}},required:['bindingId'],additionalProperties:false,
        } },
      ] }));
      return;
    }
    if (request.url === '/automation/call' && request.method === 'POST') {
      response.end(JSON.stringify({ content: [{ type: 'text', text: 'ok' }], structuredContent: { accepted: true } }));
      return;
    }
    response.writeHead(404).end(JSON.stringify({ code: 'NOT_FOUND' }));
  });
  app.listen(0, '127.0.0.1');
  await once(app, 'listening');
  const { port } = app.address();
  await writeFile(discoveryPath, JSON.stringify({ url: `http://127.0.0.1:${port}/automation`, token }));
  const { NODE_TEST_CONTEXT: _testContext, ...serverEnvironment } = process.env;
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [resolve('mcp/server.js')],
    cwd: process.cwd(),
    env: { ...serverEnvironment, BURRATA_AUTOMATION_FILE: discoveryPath },
    stderr: 'pipe',
  });
  const client = new Client({ name: 'burrata-writeback-test', version: '1.0.0' });
  try {
    await client.connect(transport);
    const uuid = '123e4567-e89b-12d3-a456-426614174000';
    const listing=await client.listTools();
    assert.deepEqual(listing.tools.map(tool=>tool.name),['search_remote_playlist_writeback_candidates','get_playlist_sync_preview']);
    const candidateArgs={bindingId:'binding',localTrackId:'local'};
    const reviewedArgs={bindingId:'binding',reviewedMatches:[{localTrackId:'local',candidateToken:uuid}]};
    await client.callTool({name:'search_remote_playlist_writeback_candidates',arguments:candidateArgs});
    await client.callTool({name:'get_playlist_sync_preview',arguments:{bindingId:'binding'}});
    await client.callTool({name:'get_playlist_sync_preview',arguments:reviewedArgs});
    const beforeInvalid=requests.length;
    for(const args of [{bindingId:'binding',reviewedMatches:[{localTrackId:'local',candidateToken:'bad'}]},{bindingId:'binding',reviewedMatches:[{localTrackId:'local',candidateToken:uuid,uri:'spotify:track:injected'}]},{bindingId:'binding',enableWrites:true}]) {
      const result=await client.callTool({name:'get_playlist_sync_preview',arguments:args});assert.equal(result.isError,true);
    }
    assert.equal(requests.length,beforeInvalid,'invalid review payloads must not reach app');
    assert.deepEqual(requests.filter(request=>request.path==='/automation/call').map(request=>request.body),[
      {name:'search_remote_playlist_writeback_candidates',arguments:candidateArgs},
      {name:'get_playlist_sync_preview',arguments:{bindingId:'binding'}},
      {name:'get_playlist_sync_preview',arguments:reviewedArgs},
    ]);

  } finally {
    await client.close().catch(() => {});
    await new Promise((resolveClose) => app.close(resolveClose));
    await rm(directory, { recursive: true, force: true });
  }
});

test('verification intent tools enforce approval schemas through the authenticated dynamic stdio proxy', async (t) => {
  const directory=await mkdtemp(join(tmpdir(),'burrata-intent-mcp-')),discoveryPath=join(directory,'automation.json'),requests=[];
  // Native registry contract: local preview/control; remotely mutating apply/manage.
  const provider={type:'string',enum:['spotify','youtube']};
  const intentId={type:'string',pattern:'^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$',minLength:36,maxLength:36};
  const hash={type:'string',pattern:'^[0-9a-f]{64}$',minLength:64,maxLength:64};
  const tools=[
    {name:'preview_verification_playlist_intent',description:'Prepare an exact local intent without remote creation.',scope:'control',inputSchema:{type:'object',properties:{provider},required:['provider'],additionalProperties:false}},
    {name:'apply_verification_playlist_intent',description:'Create one separately explicitly approved exact intent.',scope:'manage',inputSchema:{type:'object',properties:{provider,intentId,reviewToken:hash,contractHash:hash},required:['provider','intentId','reviewToken','contractHash'],additionalProperties:false}},
  ];
  const app=createHttpServer(async(request,response)=>{
    const chunks=[];for await(const chunk of request)chunks.push(chunk);
    const body=chunks.length?JSON.parse(Buffer.concat(chunks).toString()):undefined;
    requests.push({path:request.url,authorization:request.headers.authorization,body});response.setHeader('content-type','application/json');
    if(request.headers.authorization!==`Bearer ${token}`){response.writeHead(401).end(JSON.stringify({code:'UNAUTHORIZED'}));return;}
    if(request.method==='GET'&&request.url==='/automation/tools'){response.end(JSON.stringify({tools}));return;}
    if(request.method==='POST'&&request.url==='/automation/call'){response.end(JSON.stringify({content:[{type:'text',text:'mock receipt'}],structuredContent:{accepted:true}}));return;}
    response.writeHead(404).end();
  });
  app.listen(0,'127.0.0.1');await once(app,'listening');
  await writeFile(discoveryPath,JSON.stringify({url:`http://127.0.0.1:${app.address().port}/automation`,token}));
  const {NODE_TEST_CONTEXT:_testContext,...serverEnvironment}=process.env;
  const transport=new StdioClientTransport({command:process.execPath,args:[resolve('mcp/server.js')],cwd:process.cwd(),env:{...serverEnvironment,BURRATA_AUTOMATION_FILE:discoveryPath},stderr:'pipe'});
  transport.stderr?.on('data',()=>{});
  const client=new Client({name:'verification-intent-proxy-test',version:'1'});
  const approval={provider:'spotify',intentId:'123e4567-e89b-42d3-a456-426614174000',reviewToken:'c'.repeat(64),contractHash:'d'.repeat(64)};
  try {
    await client.connect(transport);
    await t.test('registry retains strict schemas and mutating scopes do not become read-only hints',async()=>{
      const listing=await client.listTools();assert.deepEqual(listing.tools.map(tool=>tool.name),tools.map(tool=>tool.name));
      for(let i=0;i<tools.length;i++){assert.deepEqual(listing.tools[i].inputSchema,tools[i].inputSchema);assert.notEqual(listing.tools[i].annotations?.readOnlyHint,true);}
      assert.equal(tools[0].scope,'control');assert.equal(tools[1].scope,'manage');
    });
    await t.test('both providers forward preview and all exact approval fields unchanged',async()=>{
      for(const source of ['spotify','youtube'])for(const [name,args] of [['preview_verification_playlist_intent',{provider:source}],['apply_verification_playlist_intent',{...approval,provider:source}]]){
        assert.notEqual((await client.callTool({name,arguments:args})).isError,true);assert.deepEqual(requests.at(-1).body,{name,arguments:args});
      }
      assert.equal(requests.filter(request=>request.path==='/automation/call').length,4);assert.ok(requests.every(request=>request.authorization===`Bearer ${token}`));
    });
    await t.test('preview rejects missing providers, wrong providers and arbitrary title/URL before dispatch',async()=>{
      const before=requests.length;
      for(const args of [{},{provider:'Spotify'},{provider:'soundcloud'},{provider:'spotify',title:'arbitrary'},{provider:'youtube',url:'https://example.invalid'},{provider:'youtube',reviewToken:approval.reviewToken}])assert.equal((await client.callTool({name:'preview_verification_playlist_intent',arguments:args})).isError,true);
      assert.equal(requests.length,before);
    });
    await t.test('apply rejects missing approval artifacts, malformed bindings and arbitrary extras before dispatch',async()=>{
      const before=requests.length,invalid=[];
      for(const key of ['provider','intentId','reviewToken','contractHash']){const missing={...approval};delete missing[key];invalid.push(missing);}
      invalid.push({...approval,provider:'Spotify'},{...approval,intentId:'123e4567-e89b-12d3-a456-426614174000'},{...approval,intentId:approval.intentId.toUpperCase()},{...approval,reviewToken:'c'.repeat(63)},{...approval,reviewToken:'C'.repeat(64)},{...approval,contractHash:'g'.repeat(64)},{...approval,contractHash:'d'.repeat(65)},{...approval,title:'arbitrary'},{...approval,playlistId:'injected'},{...approval,approved:true});
      for(const args of invalid)assert.equal((await client.callTool({name:'apply_verification_playlist_intent',arguments:args})).isError,true);
      assert.equal(requests.length,before,'invalid approval arguments must not dispatch to app');
    });
  }finally{await client.close().catch(()=>{});await new Promise(resolveClose=>app.close(resolveClose));await rm(directory,{recursive:true,force:true});}
});
