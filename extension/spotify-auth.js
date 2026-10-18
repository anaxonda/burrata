(() => {
  const GIST_URL = 'https://api.github.com/gists/22ed9c6ba463899e933427f7de1f0eef';
  const MAX_REGISTRY_BYTES = 128 * 1024;

  function decodeBase32(secret) {
    const normalized = String(secret || '').trim().toUpperCase().replace(/=+$/, '');
    if (normalized.length < 8 || normalized.length > 128) throw new Error('Spotify TOTP secret is malformed.');
    const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
    const output = [];
    let buffer = 0;
    let bits = 0;
    for (const char of normalized) {
      const value = alphabet.indexOf(char);
      // Meld's own decoder ignores characters outside RFC 4648 Base32.
      if (value < 0) continue;
      buffer = (buffer << 5) | value;
      bits += 5;
      if (bits >= 8) {
        bits -= 8;
        output.push((buffer >> bits) & 0xff);
      }
    }
    if (output.length < 5) throw new Error('Spotify TOTP secret is too short.');
    return new Uint8Array(output);
  }

  function parseRegistry(payload) {
    if (!payload || typeof payload !== 'object' || !payload.files || typeof payload.files !== 'object') {
      throw new Error('Spotify TOTP registry has an unsupported shape.');
    }
    const strings = Object.values(payload.files).map(file => file && typeof file.content === 'string' ? file.content : '').filter(Boolean);
    for (const content of strings) {
      if (new TextEncoder().encode(content).byteLength > MAX_REGISTRY_BYTES) continue;
      let entries;
      try { entries = JSON.parse(content.replace(/^\uFEFF/, '').trim()); } catch { continue; }
      if (!Array.isArray(entries)) continue;
      const valid = entries.filter(entry => entry && typeof entry.s === 'string' && Number.isInteger(entry.v) && entry.v > 0 && entry.v <= 4096)
        .filter(entry => {
          try { decodeBase32(entry.s); return true; } catch { return false; }
        });
      if (valid.length) return valid.sort((a, b) => b.v - a.v)[0];
    }
    throw new Error('Spotify TOTP registry contained no valid secret versions.');
  }

  async function totp(secret, serverTimeSeconds) {
    if (!Number.isFinite(serverTimeSeconds) || serverTimeSeconds <= 0) throw new Error('Spotify server time was invalid.');
    const counter = BigInt(Math.floor(serverTimeSeconds / 30));
    const message = new Uint8Array(8);
    let value = counter;
    for (let index = 7; index >= 0; index--) {
      message[index] = Number(value & 0xffn);
      value >>= 8n;
    }
    const key = await crypto.subtle.importKey('raw', decodeBase32(secret), { name: 'HMAC', hash: 'SHA-1' }, false, ['sign']);
    const hash = new Uint8Array(await crypto.subtle.sign('HMAC', key, message));
    const offset = hash[hash.length - 1] & 0x0f;
    const code = ((hash[offset] & 0x7f) << 24) | ((hash[offset + 1] & 0xff) << 16) | ((hash[offset + 2] & 0xff) << 8) | (hash[offset + 3] & 0xff);
    return String(code % 1_000_000).padStart(6, '0');
  }

  async function loadRegistry(storage, fetcher = fetch, now = Date.now()) {
    const cached = await storage.get('spotifyTotpNuance');
    if (cached?.spotifyTotpNuance && now - cached.spotifyTotpNuance.fetchedAt < 24 * 60 * 60 * 1000) {
      try { decodeBase32(cached.spotifyTotpNuance.secret); if (Number.isInteger(cached.spotifyTotpNuance.version)) return cached.spotifyTotpNuance; } catch { /* discard malformed cache */ }
    }
    try {
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 15000);
      let response;
      try {
        response = await fetcher(GIST_URL, { headers: { Accept: 'application/vnd.github+json' }, cache: 'no-store', signal: controller.signal });
      } catch (error) {
        if (controller.signal.aborted) throw new Error('Spotify TOTP registry request timed out.');
        throw error;
      } finally {
        clearTimeout(timeout);
      }
      if (!response.ok) throw new Error(`Spotify TOTP registry returned ${response.status}.`);
      const payload = await response.json();
      const latest = parseRegistry(payload);
      const record = { secret: latest.s, version: latest.v, fetchedAt: now };
      await storage.set({ spotifyTotpNuance: record });
      return record;
    } catch (error) {
      if (cached?.spotifyTotpNuance) {
        try { decodeBase32(cached.spotifyTotpNuance.secret); if (Number.isInteger(cached.spotifyTotpNuance.version)) return cached.spotifyTotpNuance; } catch { /* invalid cache is never used */ }
      }
      throw error;
    }
  }

  globalThis.BurrataSpotifyAuth = { GIST_URL, decodeBase32, parseRegistry, totp, loadRegistry };
})();
