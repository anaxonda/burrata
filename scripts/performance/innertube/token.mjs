import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { access } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { failure } from './budget.mjs';

const VIDEO_ID = /^[A-Za-z0-9_-]{11}$/;

export function createTokenManager({ modulePath = fileURLToPath(new URL('../../../src-tauri/resources/youtube-support/bgutil/server/build/session_manager.js', import.meta.url)), spawnChild = spawn, now = Date.now } = {}) {
  let child, reader, pending, lastCpuMs = 0;
  let playerCache = null;
  const gvsCache = new Map();
  const stop = () => {
    playerCache = null; gvsCache.clear(); lastCpuMs = 0;
    if (child) {
      const stopping = child;
      stopping.kill('SIGTERM');
      const timer = setTimeout(() => { try { stopping.kill('SIGKILL'); } catch {} }, 250);
      timer.unref();
      stopping.once('close', () => clearTimeout(timer));
    }
    child = null;
    reader?.close();
    pending?.reject(failure('TOKEN_PROVIDER_FAILED', 'token'));
    pending = null;
  };
  async function mint(contentBinding, context, { signal, refresh = false } = {}) {
    if (!child) {
      try { await access(modulePath); } catch { throw failure('TOKEN_PROVIDER_UNAVAILABLE', 'token'); }
      child = spawnChild(process.execPath, [fileURLToPath(new URL('./token_child.mjs', import.meta.url))], {
        // The diagnostic supervisor owns one process group containing worker
        // and token helper, so hard-deadline cleanup cannot orphan the helper.
        stdio: ['pipe', 'pipe', 'ignore'], detached: false,
        env: { ...process.env, BURRATA_BENCH_BGUTIL_MODULE: modulePath },
      });
      reader = createInterface({ input: child.stdout });
      reader.on('line', line => {
        const request = pending; pending = null;
        try { request?.resolve(JSON.parse(line)); } catch { request?.reject(failure('TOKEN_PROVIDER_FAILED', 'token')); }
      });
      const startedChild = child;
      child.once('error', () => { if (child === startedChild) stop(); });
      child.once('exit', () => { if (child === startedChild) stop(); });
    }
    signal?.throwIfAborted();
    const abort = () => stop();
    signal?.addEventListener('abort', abort, { once: true });
    try {
      const result = await new Promise((resolve, reject) => {
        pending = { resolve, reject };
        child.stdin.write(JSON.stringify({ context, contentBinding, bypassCache: refresh }) + '\n');
      });
      if (!result.ok || result.contentBinding !== contentBinding || !Number.isFinite(result.expiresAt) || result.expiresAt <= now() + 30_000) throw failure('TOKEN_PROVIDER_FAILED', 'token');
      const token = { value: result.poToken, expiresAt: result.expiresAt,
        cpuMs: result.tokenProcessCpuMs - lastCpuMs, maxRssBytes: result.tokenProcessMaxRssBytes };
      lastCpuMs = result.tokenProcessCpuMs;
      return token;
    } finally { signal?.removeEventListener('abort', abort); }
  }
  return {
    stop,
    applyCached(client, { videoId } = {}) {
      const context = client.session.context;
      const visitorData = context?.client?.visitorData;
      if (!visitorData || !VIDEO_ID.test(videoId ?? '')) throw failure('TOKEN_BINDING_MISSING', 'token');
      const contextKey = [context.client.clientName, context.client.clientVersion,
        visitorData, context.client.remoteHost ?? ''].join('\u0000');
      const valid = token => token && token.expiresAt > now() + 30_000;
      const player = playerCache?.contextKey === contextKey && valid(playerCache.token) ? playerCache.token : null;
      const gvsEntry = gvsCache.get(videoId);
      const gvs = gvsEntry?.contextKey === contextKey && valid(gvsEntry.token) ? gvsEntry.token : null;
      if (!player || !gvs || !client.session.player) throw failure('TOKEN_BINDING_MISSING', 'token');
      client.session.po_token = player.value;
      client.session.player.po_token = gvs.value;
      return true;
    },
    async ensure(client, { videoId, signal, refresh = false } = {}) {
      const context = client.session.context;
      const visitorData = context?.client?.visitorData;
      if (!visitorData) throw failure('TOKEN_BINDING_MISSING', 'token');
      if (!VIDEO_ID.test(videoId ?? '')) throw failure('TOKEN_BINDING_MISSING', 'token');
      // Player endpoint tokens bind to the anonymous session visitor; the GVS
      // stream token binds to this video. Keep the full Innertube context in
      // both mint calls so BgUtils can attest the same client/session.
      const contextKey = [context.client.clientName, context.client.clientVersion,
        visitorData, context.client.remoteHost ?? ''].join('\u0000');
      const valid = token => token && token.expiresAt > now() + 30_000;
      let playerHit = !refresh && playerCache?.contextKey === contextKey && valid(playerCache.token);
      if (!playerHit) {
        const token = await mint(visitorData, context, { signal, refresh });
        playerCache = { contextKey, token };
      }
      let gvs = gvsCache.get(videoId);
      let gvsHit = !refresh && gvs?.contextKey === contextKey && valid(gvs.token);
      if (!gvsHit) {
        const token = await mint(videoId, context, { signal, refresh });
        gvs = { contextKey, token };
        if (gvsCache.size >= 8 && !gvsCache.has(videoId)) gvsCache.delete(gvsCache.keys().next().value);
        gvsCache.set(videoId, gvs);
      }
      client.session.po_token = playerCache.token.value;
      if (client.session.player) client.session.player.po_token = gvs.token.value;
      const tokenHelperCpuMs = (playerHit ? 0 : playerCache.token.cpuMs) + (gvsHit ? 0 : gvs.token.cpuMs);
      return { tokenCacheHit: playerHit && gvsHit, tokenExpiresInMs: Math.min(playerCache.token.expiresAt, gvs.token.expiresAt) - now(),
        tokenHelperCpuMs, tokenHelperMaxRssBytes: Math.max(playerCache.token.maxRssBytes, gvs.token.maxRssBytes),
        tokenBindings: { player: 'visitorData', gvs: 'videoId' } };
    },
  };
}
