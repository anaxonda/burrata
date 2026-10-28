import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { posix, win32 } from 'node:path';
import { basename } from 'node:path';
import { isIP } from 'node:net';
import { McpServer, fromJsonSchema } from '@modelcontextprotocol/server';
import { serveStdio } from '@modelcontextprotocol/server/stdio';

const VERSION = '0.1.2';
const DEFAULT_SCHEMA = { type: 'object', properties: {}, additionalProperties: false };
export const REGISTRY_TIMEOUT_MS = 5_000;
export const CALL_TIMEOUT_MS = 65_000;
export const PUBLIC_DURATION_REPAIR_TIMEOUT_MS = 75_000;
export const MAX_RESPONSE_BYTES = 10 * 1024 * 1024;
export const MAX_STRUCTURED_RESULT_BYTES = 4 * 1024 * 1024;
export const MAX_RESULT_TEXT_CHARS = 16_000;

export function defaultDiscoveryPath(env = process.env, platform = process.platform, home = homedir()) {
  if (env.BURRATA_AUTOMATION_FILE) return env.BURRATA_AUTOMATION_FILE;
  const path = platform === 'win32' ? win32 : posix;
  const appData = platform === 'win32'
    ? (env.APPDATA || path.join(home, 'AppData', 'Roaming'))
    : platform === 'darwin'
      ? path.join(home, 'Library', 'Application Support')
      : (env.XDG_DATA_HOME || path.join(home, '.local', 'share'));
  return path.join(appData, 'org.burrata.discovery', 'automation.json');
}

function validateDiscovery(value) {
  if (!value || typeof value !== 'object' || typeof value.url !== 'string' || typeof value.token !== 'string' || value.token.length < 32 || !/^[A-Za-z0-9._~+/-]+=*$/.test(value.token)) {
    throw new Error('PLAYER_NOT_RUNNING');
  }
  let endpoint;
  try { endpoint = new URL(value.url); } catch { throw new Error('PLAYER_NOT_RUNNING'); }
  const host = endpoint.hostname.replace(/^\[|\]$/g, '');
  const loopback = host === 'localhost' || host === '::1' || (isIP(host) === 4 && host.startsWith('127.'));
  if (endpoint.protocol !== 'http:' || !loopback || endpoint.username || endpoint.password || !endpoint.port || !['', '/', '/automation', '/automation/'].includes(endpoint.pathname) || endpoint.search || endpoint.hash) {
    throw new Error('PLAYER_NOT_RUNNING');
  }
  return { baseUrl: endpoint.origin, token: value.token };
}

export async function readDiscovery(file = defaultDiscoveryPath()) {
  let value;
  try { value = JSON.parse(await readFile(file, 'utf8')); } catch { throw new Error('PLAYER_NOT_RUNNING'); }
  return validateDiscovery(value);
}

function appErrorResult(code) {
  const safeCode = typeof code === 'string' && /^[A-Z0-9_]{1,80}$/.test(code) ? code : 'BURRATA_UNAVAILABLE';
  const messages = {
    PLAYER_NOT_RUNNING: 'Burrata is not running. Start Burrata and try again.',
    REQUEST_TIMEOUT: 'Burrata did not respond before the request timeout. Check the operation status before repeating it.',
    NETWORK_UNAVAILABLE: 'Burrata automation could not reach the local app.',
    RESULT_TOO_LARGE: 'The result exceeded the bridge size limit. Check operation status before repeating the request.',
  };
  const safeMessage = messages[safeCode] ?? 'Burrata automation request failed.';
  return {
    content: [{ type: 'text', text: `${safeCode}: ${safeMessage}` }],
    structuredContent: { code: safeCode, message: safeMessage },
    isError: true,
  };
}

function redactToken(value, token) {
  if (typeof value === 'string') return value.split(token).join('[redacted]');
  if (Array.isArray(value)) return value.map((item) => redactToken(item, token));
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, redactToken(item, token)]));
  return value;
}

export function requestTimeoutFor(endpoint, toolName) {
  if (endpoint === '/automation/tools') return REGISTRY_TIMEOUT_MS;
  if (endpoint === '/automation/call' && toolName === 'create_verification_playlist') return 110_000;
  if (endpoint === '/automation/call' && ['preview_collection_import','retry_import_failures'].includes(toolName)) return 120_000;
  if (endpoint === '/automation/call' && toolName === 'repair_spotify_durations_public') {
    return PUBLIC_DURATION_REPAIR_TIMEOUT_MS;
  }
  return CALL_TIMEOUT_MS;
}

export function requestErrorCode(error) {
  const chain = [];
  for (let current = error; current && !chain.includes(current); current = current.cause) chain.push(current);
  if (chain.some((item) => item.code === 'RESPONSE_TOO_LARGE')) return 'RESULT_TOO_LARGE';
  if (chain.some((item) => item.name === 'TimeoutError' || item.name === 'AbortError' || item.code === 'ETIMEDOUT' || (typeof item.code === 'string' && item.code.includes('TIMEOUT')))) return 'REQUEST_TIMEOUT';
  if (chain.some((item) => item.code === 'ECONNREFUSED')) return 'PLAYER_NOT_RUNNING';
  return 'NETWORK_UNAVAILABLE';
}

export async function apiRequest(discovery, endpoint, options = {}, dependencies = {}) {
  const fetcher = dependencies.fetchImpl ?? fetch;
  const timeoutMs = dependencies.timeoutMs ?? requestTimeoutFor(endpoint, dependencies.toolName);
  const response = await fetcher(`${discovery.baseUrl}${endpoint}`, {
    ...options,
    redirect: 'error',
    signal: AbortSignal.timeout(timeoutMs),
    headers: {
      authorization: `Bearer ${discovery.token}`,
      ...(options.body ? { 'content-type': 'application/json' } : {}),
      ...options.headers,
    },
  });
  const contentLength = Number(response.headers.get('content-length'));
  if (Number.isFinite(contentLength) && contentLength > MAX_RESPONSE_BYTES) {
    throw Object.assign(new Error('Response too large'), { code: 'RESPONSE_TOO_LARGE' });
  }
  const raw = await response.text();
  if (Buffer.byteLength(raw, 'utf8') > MAX_RESPONSE_BYTES) {
    throw Object.assign(new Error('Response too large'), { code: 'RESPONSE_TOO_LARGE' });
  }
  let payload;
  try { payload = JSON.parse(raw); } catch { payload = null; }
  return { response, payload };
}

export async function fetchToolRegistry(discoveryPath = defaultDiscoveryPath()) {
  const discovery = await readDiscovery(discoveryPath);
  const { response, payload } = await apiRequest(discovery, '/automation/tools');
  if (!response.ok || !payload || !Array.isArray(payload.tools)) {
    const code = payload?.code === 'PLAYER_NOT_RUNNING' || payload?.error?.code === 'PLAYER_NOT_RUNNING'
      ? 'PLAYER_NOT_RUNNING'
      : requestErrorCode({ code: response.status === 401 ? 'UNAUTHORIZED' : 'NETWORK' });
    throw new Error(code);
  }
  const names = new Set();
  for (const tool of payload.tools) {
    if (!tool || typeof tool.name !== 'string' || !/^[a-zA-Z0-9_.-]{1,128}$/.test(tool.name) || names.has(tool.name)) {
      throw new Error('Invalid Burrata automation tool registry');
    }
    if (typeof tool.description !== 'string' || !tool.description || (tool.inputSchema != null && (typeof tool.inputSchema !== 'object' || Array.isArray(tool.inputSchema)))) {
      throw new Error('Invalid Burrata automation tool registry');
    }
    names.add(tool.name);
  }
  return payload.tools;
}

async function callAppTool(name, args, discoveryPath) {
  let discovery;
  try { discovery = await readDiscovery(discoveryPath); }
  catch { return appErrorResult('PLAYER_NOT_RUNNING'); }
  try {
    const { response, payload } = await apiRequest(discovery, '/automation/call', {
      method: 'POST', body: JSON.stringify({ name, arguments: args ?? {} }),
    }, { toolName: name });
    if (!response.ok || !payload || typeof payload !== 'object') {
      const errorCode = payload?.code ?? payload?.error?.code ?? payload?.structuredContent?.error?.code;
      const code = errorCode === 'PLAYER_NOT_RUNNING' ? errorCode : 'NETWORK_UNAVAILABLE';
      return appErrorResult(code);
    }
    if (!Array.isArray(payload.content)) return appErrorResult('BURRATA_UNAVAILABLE', 'Burrata returned an invalid tool result.');
    const structuredContent = payload.structuredContent === undefined ? undefined : redactToken(payload.structuredContent, discovery.token);
    if (structuredContent !== undefined && Buffer.byteLength(JSON.stringify(structuredContent), 'utf8') > MAX_STRUCTURED_RESULT_BYTES) {
      return appErrorResult('RESULT_TOO_LARGE');
    }
    const content = redactToken(payload.content, discovery.token).slice(0, 20).map((block) => {
      if (block?.type !== 'text' || typeof block.text !== 'string' || block.text.length <= MAX_RESULT_TEXT_CHARS) return block;
      return { ...block, text: `${block.text.slice(0, MAX_RESULT_TEXT_CHARS)}\n[Text shortened; structured content contains the complete result.]` };
    });
    return {
      content,
      ...(structuredContent !== undefined ? { structuredContent } : {}),
      ...(payload.isError === true ? { isError: true } : {}),
    };
  } catch (error) {
    return appErrorResult(requestErrorCode(error));
  }
}

export function createServer(tools, discoveryPath = defaultDiscoveryPath()) {
  const server = new McpServer({ name: 'burrata', version: VERSION });
  for (const tool of tools) {
    server.registerTool(tool.name, {
      ...(typeof tool.title === 'string' ? { title: tool.title } : {}),
      description: tool.description,
      inputSchema: fromJsonSchema(tool.inputSchema ?? DEFAULT_SCHEMA),
      ...(tool.annotations ? { annotations: tool.annotations } : tool.scope === 'read' ? { annotations: { readOnlyHint: true } } : {}),
    }, (args) => callAppTool(tool.name, args, discoveryPath));
  }
  return server;
}

export async function main() {
  const discoveryPath = defaultDiscoveryPath();
  let tools = [];
  try { tools = await fetchToolRegistry(discoveryPath); }
  catch (error) {
    const message = error instanceof Error && error.message === 'PLAYER_NOT_RUNNING'
      ? 'Burrata is not running; the MCP tool list is empty.'
      : 'Could not load Burrata MCP tools; the MCP tool list is empty.';
    console.error(message);
  }
  const handle = serveStdio(() => createServer(tools, discoveryPath));
  process.on('SIGINT', () => { void handle.close(); });
  process.on('SIGTERM', () => { void handle.close(); });
}

if (process.argv[1] && basename(process.argv[1]) === 'server.js') {
  void main().catch(() => {
    console.error('Burrata MCP server could not start.');
    process.exitCode = 1;
  });
}
