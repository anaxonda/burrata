// Opt-in research build only. Runs in the selected Monochrome page, never reads
// retains request header/cookie values, and exports only a bounded sanitized observation.
(() => {
  const REDACTED = '[REDACTED]';
  const SLOT = '__burrataMonochromeResearchV1';
  const LIMIT = 256 * 1024;
  const secret = /token|authorization|cookie|password|secret|signature|(?:^|_)key(?:$|_)|^keys$|keyid|decryptionkey|keyvalue|^kid$|^iv$/i;
  const textFields = /^(title|name|artist|artists|album|version|isrc|provider|source|selected_source|kind|delivery|codec|container|mime_type|quality|quality_requested|scheme|algorithm|method|type|schema_version|status|format|expires_at|expiresAt|expiry|expiration|valid_until|validUntil|id|track_id|recording_id)$/i;
  const queryFields = new Set(['track', 'artist', 'album', 'isrc', 'duration', 'intent', 'quality']);
  const safeIvSources = new Set(['explicit', 'derived', 'default', 'manifest', 'segment', 'track_id', 'key_id', 'response', 'metadata', 'unknown']);
  function sanitize(value, field = '', depth = 0, keyMaterial = false, budget = { nodes: 0 }) {
    if (++budget.nodes > 2000) return '[NODE LIMIT]';
    if (depth > 10) return '[DEPTH LIMIT]';
    const sensitive = keyMaterial || secret.test(field);
    if (value === null) return null;
    if (Array.isArray(value)) return value.slice(0, 40).map(item => sanitize(item, field, depth + 1, sensitive, budget));
    if (typeof value === 'object') {
      const result = {};
      for (const [key, child] of Object.entries(value).slice(0, 80)) {
        const safeKey = /^[a-zA-Z_][a-zA-Z0-9_-]{0,63}$/.test(key) ? key : '[REDACTED FIELD]';
        // Keep encryption structure and algorithm names, but redact key leaves.
        result[safeKey] = sanitize(child, key, depth + 1, sensitive, budget);
      }
      return result;
    }
    if (/^encoding$/i.test(field) && typeof value === 'string') return /^(hex|base64|base64url|utf-8)$/i.test(value) ? value.toLowerCase() : REDACTED;
    if (/^iv_source$/i.test(field) && typeof value === 'string') return safeIvSources.has(value.toLowerCase()) ? value.toLowerCase() : REDACTED;
    if (sensitive) return REDACTED;
    if (typeof value === 'number') return Number.isFinite(value) ? value : null;
    if (typeof value === 'boolean') return value;
    if (typeof value !== 'string' || !textFields.test(field)) return REDACTED;
    if (/https?:|bearer\s|eyJ[a-zA-Z0-9_-]+\.|[a-f0-9]{32,}|[a-zA-Z0-9_-]{60,}/i.test(value)) return REDACTED;
    return value.slice(0, 256);
  }
  function route(value, base = 'https://monochrome.samidy.com/') {
    try {
      if (typeof value !== 'string' || !value.trim()) return null;
      const url = new URL(value, base);
      if (!['https:', 'http:'].includes(url.protocol)) return null;
      const path = url.pathname.split('/').map(part => /^[a-zA-Z0-9_-]{24,}$/.test(part) || /eyJ|%3[fF]|%3[dD]|%26/.test(part) ? '[REDACTED]' : part.replace(/[a-fA-F0-9]{32,}/g, '[REDACTED]')).join('/');
      return `${url.origin}${path.slice(0, 512)}`;
    } catch { return null; }
  }
  function headerValue(headers, wanted) {
    if (!headers) return { present: false, value: '' };
    try {
      if (typeof headers.get === 'function') {
        const value = headers.get(wanted);
        return { present: value !== null && value !== undefined, value: value == null ? '' : String(value) };
      }
      for (const [name, value] of Object.entries(headers)) {
        if (name.toLowerCase() === wanted.toLowerCase()) return { present: true, value: String(value) };
      }
    } catch {}
    return { present: false, value: '' };
  }
  function requestMetadata(urlValue, method = 'GET', headers = null, fallbackHeaders = null) {
    const query = {};
    try {
      const url = new URL(urlValue, 'https://monochrome.samidy.com/');
      for (const key of queryFields) {
        const value = url.searchParams.get(key);
        if (value === null || !value.trim() || value.length > 128 || /https?:|bearer\s|eyJ[a-zA-Z0-9_-]+\.|[a-f0-9]{32,}|[a-zA-Z0-9_-]{60,}|(?:token|authorization|cookie|secret|signature|password|api[_-]?key)/i.test(value)) continue;
        query[key] = value;
      }
    } catch {}
    const read = name => {
      const primary = headerValue(headers, name);
      return primary.present ? primary : headerValue(fallbackHeaders, name);
    };
    const authorization = read('authorization');
    const turnstile = read('x-turnstile-jwt');
    const contentType = read('content-type');
    const safeMethod = /^[A-Z]{1,16}$/i.test(String(method)) ? String(method).toUpperCase() : 'UNKNOWN';
    return { method: safeMethod, query, headers: {
      authorizationBearer: authorization.present && /^Bearer\s+\S+/i.test(authorization.value),
      turnstileJwtPresent: turnstile.present,
      contentTypePresent: contentType.present,
    } };
  }
  function isResolver(body) {
    if (!body || typeof body !== 'object') return false;
    if (Array.isArray(body.playback)) return true;
    const keys = Object.keys(body).join(' ');
    if (/^(?:.*\s)?(?:stream_url|streamUrl|decryption_key|decryptionKey|trackInfo|manifest)(?:\s|$)/.test(keys)) return true;
    return body.data && typeof body.data === 'object' && (Array.isArray(body.data.playback) || Object.keys(body.data).some(k => /manifest|stream_?url|decryption/i.test(k)));
  }
  // Expose pure helpers to the isolated test harness; no production page export.
  if (typeof window === 'undefined') {
    globalThis.BurrataMonochromeResearchTest = { sanitize, route, isResolver, requestMetadata };
    return;
  }
  if (location.origin !== 'https://monochrome.samidy.com' || window !== window.top || window[SLOT]) return;
  const rows = [];
  const state = { schemaVersion: 1, evidenceKind: 'page_observed_sanitized_network', startedAt: new Date().toISOString(), stopped: false, responses: rows };
  const pendingXhr = new Set();
  const originalFetch = window.fetch;
  const originalOpen = XMLHttpRequest.prototype.open;
  const originalSend = XMLHttpRequest.prototype.send;
  const originalSetRequestHeader = XMLHttpRequest.prototype.setRequestHeader;
  const xhrUrls = new WeakMap();
  const xhrRequests = new WeakMap();
  const append = (endpoint, status, body, request) => {
    if (state.stopped || !endpoint) return;
    const auth = /\/(?:auth|turnstile)(?:\/|$)/i.test(endpoint);
    if (!auth && !isResolver(body)) return;
    const row = { observedAt: new Date().toISOString(), endpoint, status, kind: auth ? 'authorization' : 'resolver', request, response: sanitize(body) };
    rows.push(row);
    if (rows.length > 20) rows.shift();
  };
  async function inspectFetch(response, requested, request) {
    if (state.stopped || !/json/i.test(response.headers.get('content-type') || '')) return;
    const length = Number(response.headers.get('content-length'));
    if (length > LIMIT) return;
    const reader = response.clone().body?.getReader();
    if (!reader) return;
    const chunks = []; let size = 0;
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > LIMIT || state.stopped) { void reader.cancel().catch(() => {}); return; }
        chunks.push(value);
      }
      const bytes = new Uint8Array(size); let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
      append(route(response.url || requested), response.status, JSON.parse(new TextDecoder().decode(bytes)), request);
    } catch { /* Observation must not affect playback. */ }
  }
  function observedFetch(...args) {
    const input = args[0];
    const init = args[1] || {};
    let request = {};
    try {
      const inputUrl = typeof input === 'string' ? input : input?.url;
      const inputHeaders = input?.headers;
      request = requestMetadata(inputUrl, init.method || input?.method || 'GET', init.headers, inputHeaders);
    } catch {}
    const promise = Reflect.apply(originalFetch, this, args);
    promise.then(response => { void inspectFetch(response, typeof args[0] === 'string' ? args[0] : args[0]?.url, request).catch(() => {}); }, () => {});
    return promise;
  }
  function observedOpen(method, url, ...args) {
    xhrUrls.set(this, route(url));
    xhrRequests.set(this, { method, url, authorizationBearer: false, turnstileJwtPresent: false, contentTypePresent: false });
    return Reflect.apply(originalOpen, this, [method, url, ...args]);
  }
  function observedSetRequestHeader(name, value) {
    const request = xhrRequests.get(this);
    if (request) {
      const key = String(name).toLowerCase();
      if (key === 'authorization') request.authorizationBearer = /^Bearer\s+\S+/i.test(String(value));
      if (key === 'x-turnstile-jwt') request.turnstileJwtPresent = true;
      if (key === 'content-type') request.contentTypePresent = true;
    }
    return Reflect.apply(originalSetRequestHeader, this, [name, value]);
  }
  function observedSend(...args) {
    const xhr = this;
    const sourceRequest = xhrRequests.get(xhr) || {};
    const request = requestMetadata(sourceRequest.url, sourceRequest.method,
      { authorization: sourceRequest.authorizationBearer ? 'Bearer present' : '', 'x-turnstile-jwt': sourceRequest.turnstileJwtPresent ? 'present' : '', 'content-type': sourceRequest.contentTypePresent ? 'present' : '' });
    const onLoad = () => {
      pendingXhr.delete(record);
      try {
        if (!/json/i.test(xhr.getResponseHeader('content-type') || '')) return;
        const body = xhr.responseType === 'json' ? xhr.response : (!xhr.responseType || xhr.responseType === 'text') && xhr.responseText.length <= LIMIT ? JSON.parse(xhr.responseText) : null;
        if (body) append(xhrUrls.get(xhr), xhr.status, body, request);
      } catch { /* Preserve page behavior. */ }
    };
    const record = [xhr, onLoad]; pendingXhr.add(record);
    xhr.addEventListener('load', onLoad, { once: true });
    return Reflect.apply(originalSend, xhr, args);
  }
  window.fetch = observedFetch;
  XMLHttpRequest.prototype.open = observedOpen;
  XMLHttpRequest.prototype.send = observedSend;
  if (originalSetRequestHeader) XMLHttpRequest.prototype.setRequestHeader = observedSetRequestHeader;
  function stop() {
    state.stopped = true;
    if (window.fetch === observedFetch) window.fetch = originalFetch;
    if (XMLHttpRequest.prototype.open === observedOpen) XMLHttpRequest.prototype.open = originalOpen;
    if (XMLHttpRequest.prototype.send === observedSend) XMLHttpRequest.prototype.send = originalSend;
    if (originalSetRequestHeader && XMLHttpRequest.prototype.setRequestHeader === observedSetRequestHeader) XMLHttpRequest.prototype.setRequestHeader = originalSetRequestHeader;
    for (const [xhr, listener] of pendingXhr) xhr.removeEventListener('load', listener);
    pendingXhr.clear();
  }
  window[SLOT] = {
    stop,
    read() {
      // Historical endpoints identify the deployed API without replaying a track.
      const endpoints = [...new Set(performance.getEntriesByType('resource').filter(x => ['fetch', 'xmlhttprequest', 'script'].includes(x.initiatorType)).map(x => route(x.name)).filter(Boolean))].slice(-100);
      return { ...state, responses: rows.slice(), pageOrigin: location.origin, serviceWorker: route(navigator.serviceWorker?.controller?.scriptURL), endpoints };
    },
  };
  setTimeout(stop, 5 * 60 * 1000);
})();
