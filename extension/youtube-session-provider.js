// Direct HTTP YouTube InnerTube prototype. Credentials and normalized session
// context are supplied by an explicit browser handoff, never scraped from a
// local Chrome profile by this module. Playlist edits require a reviewed,
// approved preview and exact fresh occurrence-ID snapshot.
(function () {
  'use strict';

  const first = (...values) => values.find(value => value !== undefined && value !== null && value !== '');
  // InnerTube uses a symbolic name in context.client.clientName and its
  // numeric counterpart in X-YouTube-Client-Name.
  const clientIds = Object.freeze({ WEB: '1', WEB_REMIX: '67' });
  const clientIdentity = value => {
    const raw = String(value || 'WEB');
    if (Object.hasOwn(clientIds, raw)) return { name: raw, id: clientIds[raw] };
    const name = Object.keys(clientIds).find(key => clientIds[key] === raw);
    return name ? { name, id: raw } : null;
  };
  const parseSessionCookies = header => {
    if (typeof header !== 'string' || header.length > 16_384) return null;
    const cookies = Object.create(null);
    const allowed = ['SAPISID', '__Secure-1PAPISID', '__Secure-3PAPISID', 'LOGIN_INFO', 'SID', 'HSID', 'SSID', 'APISID', '__Secure-1PSID', '__Secure-3PSID', 'SIDCC', '__Secure-1PSIDTS', '__Secure-3PSIDTS', '__Secure-1PSIDCC', '__Secure-3PSIDCC'];
    for (const [index, part] of header.split(';').entries()) {
      const pair = index === 0 ? part : part.replace(/^[ \t]+/, '');
      const equals = pair.indexOf('=');
      if (equals < 1) return null;
      const name = pair.slice(0, equals), value = pair.slice(equals + 1);
      if (!allowed.includes(name)
          || Object.hasOwn(cookies, name)
          || !/^[^\s;\u0000-\u001f\u007f]{8,4096}$/.test(value)) return null;
      cookies[name] = value;
    }
    return ['SAPISID', '__Secure-1PAPISID', '__Secure-3PAPISID'].some(name => Object.hasOwn(cookies, name)) ? cookies : null;
  };
  const text = value => value?.runs?.map(run => run.text).join('') || value?.simpleText || '';
  const channelIdFromResolution = value => {
    const id = value?.endpoint?.browseEndpoint?.browseId || value?.browseEndpoint?.browseId;
    return typeof id === 'string' && /^UC[A-Za-z0-9_-]{20,30}$/.test(id) ? id : null;
  };
  const safeChannelResolutionUrl = value => {
    if (typeof value !== 'string' || value.length > 2048 || /[\u0000-\u0020\u007f\\]/u.test(value)) return null;
    let parsed;
    try { parsed = new URL(value, 'https://www.youtube.com'); } catch { return null; }
    if (parsed.protocol !== 'https:' || !['youtube.com', 'www.youtube.com', 'music.youtube.com'].includes(parsed.hostname.toLowerCase()) || parsed.username || parsed.password || parsed.port || parsed.hash) return null;
    let rawPath;
    if (value.startsWith('/')) rawPath = value.split(/[?#]/, 1)[0];
    else {
      const absolute = /^https:\/\/[^/?#]+(\/[^?#]*)?(?:[?#]|$)/i.exec(value);
      if (!absolute) return null;
      rawPath = absolute[1] || '/';
    }
    const segments = rawPath.slice(1).split('/');
    if (segments.at(-1) === '') segments.pop();
    if (![1, 2].includes(segments.length) || segments.some(segment => !segment)) return null;
    let decoded;
    try { decoded = segments.map(segment => decodeURIComponent(segment)); } catch { return null; }
    if (decoded.some(segment => !segment || segment === '.' || segment === '..' || /[%/\\\u0000-\u001f\u007f]/u.test(segment))) return null;
    const handleChars = /^[\p{L}\p{N}\p{M}._-]{1,100}$/u;
    if (segments.length === 1) {
      if (!decoded[0].startsWith('@') || !handleChars.test(decoded[0].slice(1))) return null;
      return `https://www.youtube.com/@${encodeURIComponent(decoded[0].slice(1))}`;
    }
    if (!['channel', 'c', 'user'].includes(decoded[0]) || !handleChars.test(decoded[1])) return null;
    return `https://www.youtube.com/${decoded[0]}/${encodeURIComponent(decoded[1])}`;
  };
  const validHandle = value => {
    const points = Array.from(String(value || ''));
    return points.length > 0 && points.length <= 100 && points.every(point => /^[\p{L}\p{N}\p{M}._-]$/u.test(point));
  };
  const image = value => {
    const rows = value?.thumbnails || value?.musicThumbnailRenderer?.thumbnail?.thumbnails || value?.thumbnail?.thumbnails || value?.thumbnailViewModel?.image?.sources;
    return Array.isArray(rows) ? rows.at(-1)?.url || null : null;
  };
  const duration = value => {
    const parts = String(value || '').split(':');
    if (parts.length < 2 || parts.length > 3 || parts.some(part => !/^\d+$/.test(part))) return null;
    return parts.reduce((total, part) => total * 60 + Number(part), 0) || null;
  };
  const viewText = value => typeof value === 'string' ? value : first(text(value), value?.content, value?.dynamicTextViewModel?.text?.content, value?.text?.content, value?.accessibility?.accessibilityData?.label) || '';
  // Public WEB lockups expose duration in thumbnail bottom-overlay badges,
  // not the content metadata rows (which contain views/publication age).
  const lockupBadgeDuration = row => {
    const overlays = row?.contentImage?.thumbnailViewModel?.overlays;
    const values = new Set();
    for (const overlay of Array.isArray(overlays) ? overlays : []) {
      for (const badge of Array.isArray(overlay?.thumbnailBottomOverlayViewModel?.badges) ? overlay.thumbnailBottomOverlayViewModel.badges : []) {
        const value = badge?.thumbnailBadgeViewModel;
        if (value?.badgeStyle !== 'THUMBNAIL_OVERLAY_BADGE_STYLE_DEFAULT') continue;
        const label = viewText(value.text);
        if (/^(?:\d+:)?\d{1,2}:[0-5]\d$/.test(label)) values.add(duration(label));
      }
    }
    return values.size === 1 ? [...values][0] : null;
  };
  const parseInitialData = html => {
    for (const match of String(html).matchAll(/ytInitialData\s*=\s*/g)) {
      let start = match.index + match[0].length;
      while (/\s/.test(html[start] || '')) start++;
      if (html[start] !== '{') continue;
      let depth = 0, inString = false, escaped = false;
      for (let i = start; i < html.length; i++) {
        const ch = html[i];
        if (inString) { if (escaped) escaped = false; else if (ch === '\\') escaped = true; else if (ch === '"') inString = false; continue; }
        if (ch === '"') inString = true;
        else if (ch === '{') depth++;
        else if (ch === '}' && --depth === 0) { try { return JSON.parse(html.slice(start, i + 1)); } catch { break; } }
      }
    }
    return null;
  };
  const parseYtcfg = html => {
    const source = String(html);
    for (const marker of ['ytcfg.set(', 'ytcfg.set (']) {
      let from = 0;
      while ((from = source.indexOf(marker, from)) >= 0) {
        let start = from + marker.length;
        while (/\s/.test(source[start] || '')) start++;
        if (source[start] !== '{') { from = start; continue; }
        let depth = 0, inString = false, escaped = false;
        for (let i = start; i < source.length; i++) {
          const ch = source[i];
          if (inString) { if (escaped) escaped = false; else if (ch === '\\') escaped = true; else if (ch === '"') inString = false; continue; }
          if (ch === '"') inString = true;
          else if (ch === '{') depth++;
          else if (ch === '}' && --depth === 0) {
            try { const value = JSON.parse(source.slice(start, i + 1)); if (value && typeof value === 'object' && !Array.isArray(value)) return value; } catch { /* keep looking for a JSON config block */ }
            break;
          }
        }
        from = start;
      }
    }
    return null;
  };
  const nextToken = root => {
    let token = null;
    const walk = (value, continuation = false) => {
      if (!value || typeof value !== 'object' || token) return;
      if (value.nextContinuationData?.continuation) { token = value.nextContinuationData.continuation; return; }
      if (continuation && value.continuationCommand?.token) { token = value.continuationCommand.token; return; }
      if (value.continuationItemRenderer?.continuationEndpoint?.continuationCommand?.token) { token = value.continuationItemRenderer.continuationEndpoint.continuationCommand.token; return; }
      if (value.continuationItemViewModel?.continuationCommand?.innertubeCommand?.continuationCommand?.token) { token = value.continuationItemViewModel.continuationCommand.innertubeCommand.continuationCommand.token; return; }
      for (const [key, child] of Object.entries(value)) walk(child, continuation || /continuation/i.test(key));
    };
    walk(root);
    return token;
  };
  const decode = root => {
    const rows = [];
    const walk = value => {
      if (!value || typeof value !== 'object') return;
      if (value.lockupViewModel) {
        const x = value.lockupViewModel, metadata = x.metadata?.lockupMetadataViewModel || x.lockupMetadataViewModel || {};
        const command = x.rendererContext?.commandContext?.onTap?.innertubeCommand || {};
        const endpoint = command.browseEndpoint || command.watchEndpoint || {};
        const id = first(x.contentId, endpoint.browseId, endpoint.playlistId);
        const type = String(x.contentType || '').toLowerCase();
        const title = viewText(metadata.title) || viewText(x.title);
        const metadataRows = metadata.metadata?.contentMetadataViewModel?.metadataRows || metadata.metadataRows || [];
        const subtitle = metadataRows.map(row => (row.metadataParts || []).map(part => viewText(part.text || part)).filter(Boolean).join('')).filter(Boolean).join(' • ');
        if (id && title && (type.includes('playlist') || String(id).startsWith('PL') || String(id).startsWith('VL'))) {
          const playlistId = String(id).replace(/^VL/, '');
          rows.push({ id: playlistId, kind: 'playlist', title, subtitle, url: `https://www.youtube.com/playlist?list=${playlistId}`, artworkUrl: image(x.contentImage || x.thumbnail), data: { provider: 'youtube', playlistId, total: null } });
        } else if (id && title && (type.includes('channel') || String(id).startsWith('UC'))) {
          rows.push({ id, kind: 'channel', title, subtitle, url: `https://www.youtube.com/channel/${id}`, artworkUrl: image(x.contentImage || x.thumbnail), data: { provider: 'youtube', channelId: id } });
        }
      }
      if (value.videoRenderer) {
        const x = value.videoRenderer, id = x.videoId;
        if (id) { const title = text(x.title), artist = text(x.ownerText || x.shortBylineText), durationSeconds = duration(x.lengthText?.simpleText), url = `https://www.youtube.com/watch?v=${id}`, artworkUrl = image(x.thumbnail); rows.push({ id, kind: 'track', title, subtitle: artist, url, artworkUrl, data: { id, provider: 'youtube', trackId: id, title, artist, album: '', durationSeconds, url, artworkUrl, available: true } }); }
      }
      if (value.channelRenderer) {
        const x = value.channelRenderer, id = x.channelId;
        if (id) rows.push({ id, kind: 'channel', title: text(x.title), subtitle: text(x.subscriberCountText), url: `https://www.youtube.com/channel/${id}`, artworkUrl: image(x.thumbnail), data: { provider: 'youtube', channelId: id } });
      }
      if (value.playlistRenderer || value.gridPlaylistRenderer) {
        const x = value.playlistRenderer || value.gridPlaylistRenderer, id = x.playlistId || x.navigationEndpoint?.watchEndpoint?.playlistId;
        if (id) rows.push({ id, kind: 'playlist', title: text(x.title) || 'Untitled playlist', subtitle: text(x.shortBylineText), url: `https://www.youtube.com/playlist?list=${id}`, artworkUrl: image(x.thumbnails?.[0]) || image(x.thumbnail), data: { provider: 'youtube', playlistId: id, total: Number(x.videoCount) || null } });
      }
      if (value.musicResponsiveListItemRenderer) {
        const x = value.musicResponsiveListItemRenderer, id = x.playlistItemData?.videoId || x.navigationEndpoint?.watchEndpoint?.videoId || x.flexColumns?.[0]?.musicResponsiveListItemFlexColumnRenderer?.text?.runs?.[0]?.navigationEndpoint?.watchEndpoint?.videoId;
        const columns = (x.flexColumns || []).map(column => text(column.musicResponsiveListItemFlexColumnRenderer?.text));
        if (id) { const title = columns[0] || '', artist = columns.slice(1).join(' • '), durationSeconds = duration(text(x.fixedColumns?.[0]?.musicResponsiveListItemFlexColumnRenderer?.text)), url = `https://www.youtube.com/watch?v=${id}`, artworkUrl = image(x.thumbnail); rows.push({ id, kind: 'track', title, subtitle: artist, url, artworkUrl, data: { id, provider: 'youtube', trackId: id, title, artist, album: '', durationSeconds, url, artworkUrl, available: true } }); }
      }
      if (value.musicTwoRowItemRenderer) {
        const x = value.musicTwoRowItemRenderer, endpoint = x.navigationEndpoint || x.title?.runs?.[0]?.navigationEndpoint, id = endpoint?.browseEndpoint?.browseId || endpoint?.watchEndpoint?.playlistId;
        if (id && String(id).startsWith('UC')) rows.push({ id, kind: 'channel', title: text(x.title), subtitle: text(x.subtitle), url: `https://www.youtube.com/channel/${id}`, artworkUrl: image(x.thumbnailRenderer?.musicThumbnailRenderer?.thumbnail), data: { provider: 'youtube', channelId: id } });
        else if (id && /^(?:VL|PL)/.test(String(id))) { const playlistId = String(id).replace(/^VL/, ''); rows.push({ id: playlistId, kind: 'playlist', title: text(x.title), subtitle: text(x.subtitle), url: `https://www.youtube.com/playlist?list=${playlistId}`, artworkUrl: image(x.thumbnailRenderer?.musicThumbnailRenderer?.thumbnail), data: { provider: 'youtube', playlistId, total: null } }); }
      }
      for (const child of Object.values(value)) walk(child);
    };
    walk(root);
    const seen = new Set();
    return rows.filter(row => !seen.has(`${row.kind}:${row.id}`) && (seen.add(`${row.kind}:${row.id}`), true));
  };
  // yt-dlp 897341241f2bfb2439fe0367f4c7c9ccd29a3828, youtube/_tab.py
  // _reload_with_unavailable_videos documents VL browse + wgYCCAA= to include
  // unavailable occurrences. Retain that request contract for reads/preflight.
  const recommendedPlaylistVideo = value => value?.playlistVideoRenderer?.style === 'PLAYLIST_VIDEO_RENDERER_STYLE_RECOMMENDED_VIDEO';
  // NavigationEndpoint unwraps these explicit command forms; never search arbitrary JSON.
  const playlistContinuationToken = value => {
    const isItem = Boolean(value?.continuationItemRenderer || value?.continuationItemViewModel);
    if (!isItem) return null;
    const endpoint = value?.continuationItemRenderer?.continuationEndpoint || value?.continuationItemViewModel?.continuationCommand;
    const tokens = new Set(); let invalid = false, visited = 0;
    const read = (command, depth = 0) => {
      if (!command || typeof command !== 'object') return;
      if (depth > 8 || ++visited > 128) { invalid = true; return; }
      if (command.continuationCommand) {
        const continuation = command.continuationCommand;
        if (continuation.request && continuation.request !== 'CONTINUATION_REQUEST_TYPE_BROWSE') { invalid = true; return; }
        if (typeof continuation.token !== 'string' || !continuation.token || continuation.token.length > 16384) invalid = true;
        else tokens.add(continuation.token);
      }
      for (const key of ['innertubeCommand', 'command', 'performOnceCommand']) if (command[key]) read(command[key], depth + 1);
      for (const key of ['commandExecutorCommand', 'serialCommand', 'parallelCommand']) {
        const commands = command[key]?.commands;
        if (Array.isArray(commands)) for (const child of commands) read(child, depth + 1);
      }
    };
    read(endpoint);
    return invalid || tokens.size !== 1 ? {} : [...tokens][0];
  };
  // Own-library cursors must belong to a recognized list, never a sidebar or
  // arbitrary endpoint. Share the bounded NavigationEndpoint command decoder.
  const libraryContinuation = (root, previousToken, inspect) => {
    const tokens = new Set(), seen = new WeakSet();
    const shape = { version: 1, recognizedContainers: 0, directCursorItems: 0, wrappedCursorItems: 0, invalidCursorItems: 0, repeatedCursorItems: 0, distinctTokens: 0, unrecognizedContinuationNodes: 0, visitedNodes: 0, truncated: false, ambiguous: false, terminalObserved: false };
    const inspectedItems = new WeakSet();
    let visited = 0;
    const item = (value, depth) => {
      if (!value || typeof value !== 'object') return;
      if (value.nextContinuationData) {
        inspectedItems.add(value);
        const token = value.nextContinuationData.continuation;
        if (typeof token !== 'string' || !token || token.length > 16384) { shape.invalidCursorItems++; return; }
        shape.directCursorItems++;
        if (tokens.has(token) || token === previousToken) shape.repeatedCursorItems++;
        tokens.add(token);
        return;
      }
      if (value.continuationItemRenderer || value.continuationItemViewModel) {
        inspectedItems.add(value);
        const token = playlistContinuationToken(value);
        const endpoint = value.continuationItemRenderer?.continuationEndpoint || value.continuationItemViewModel?.continuationCommand;
        if (typeof token !== 'string') { shape.invalidCursorItems++; return; }
        if (endpoint?.continuationCommand) shape.directCursorItems++; else shape.wrappedCursorItems++;
        if (tokens.has(token) || token === previousToken) shape.repeatedCursorItems++;
        tokens.add(token);
        return;
      }
      // Only documented content wrappers are navigable membership containers.
      for (const key of ['itemSectionRenderer', 'sectionListRenderer', 'richGridRenderer', 'gridRenderer', 'gridContinuation', 'richGridContinuation', 'sectionListContinuation', 'itemSectionContinuation']) {
        const container = value[key];
        if (container && typeof container === 'object') {
          shape.recognizedContainers++;
          walk(container.contents || container.items || [], depth + 1);
          if (Array.isArray(container.continuations)) walk(container.continuations, depth + 1);
        }
      }
    };
    const walk = (values, depth = 0) => {
      if (!values || typeof values !== 'object') return;
      if (depth > 32 || ++visited > 30000) { shape.truncated = true; return; }
      if (seen.has(values)) { shape.truncated = true; return; }
      seen.add(values);
      if (Array.isArray(values)) for (const value of values) walk(value, depth + 1);
      else item(values, depth);
    };
    walk(root);
    if (root?.continuationContents) walk(root.continuationContents);
    // Append actions are traversed only through their explicit command arrays;
    // named targets need a source-grounded binding before being accepted.
    const commands = (value, depth = 0) => {
      if (!value || typeof value !== 'object') return;
      if (depth > 8 || ++visited > 30000) { shape.truncated = true; return; }
      const append = value.appendContinuationItemsAction;
      if (append && !Object.hasOwn(append, 'target') && !Object.hasOwn(append, 'targetId') && Array.isArray(append.continuationItems)) {
        shape.recognizedContainers++;
        walk(append.continuationItems);
      }
      for (const command of Array.isArray(value.commandExecutorCommand?.commands) ? value.commandExecutorCommand.commands : []) commands(command, depth + 1);
    };
    for (const key of ['onResponseReceivedActions', 'onResponseReceivedEndpoints', 'onResponseReceivedCommands']) {
      for (const command of Array.isArray(root?.[key]) ? root[key] : []) commands(command);
    }
    // Diagnostic-only broad structure count detects continuation nodes outside
    // recognized containers without exposing any raw keys, values or tokens.
    if (inspect) {
      const diagnosticSeen = new WeakSet();
      const count = (value, depth = 0) => {
        if (!value || typeof value !== 'object') return;
        if (depth > 64 || shape.visitedNodes >= 30000 || diagnosticSeen.has(value)) { shape.truncated = true; return; }
        diagnosticSeen.add(value); shape.visitedNodes++;
        if ((value.continuationItemRenderer || value.continuationItemViewModel || value.nextContinuationData) && !inspectedItems.has(value)) shape.unrecognizedContinuationNodes = Math.min(10000, shape.unrecognizedContinuationNodes + 1);
        for (const child of Object.values(value)) count(child, depth + 1);
      };
      count(root);
    }
    for (const key of ['recognizedContainers', 'directCursorItems', 'wrappedCursorItems', 'invalidCursorItems', 'repeatedCursorItems']) shape[key] = Math.min(10000, shape[key]);
    shape.distinctTokens = Math.min(10000, tokens.size);
    shape.ambiguous = shape.truncated || shape.invalidCursorItems > 0 || shape.repeatedCursorItems > 0 || tokens.size > 1;
    shape.terminalObserved = shape.recognizedContainers > 0 && !shape.ambiguous && tokens.size === 0 && (!inspect || shape.unrecognizedContinuationNodes === 0);
    return { token: !shape.ambiguous && tokens.size === 1 ? [...tokens][0] : null, shape };
  };
  const playlistSections = (root, playlistId) => {
    const sections = [], outerContinuations = [];
    let ambiguousOuter = false, invalidAppendWrapper = false;
    const recognized = new Set(['playlistVideoListRenderer', 'musicPlaylistShelfRenderer', 'musicPlaylistShelfContinuation', 'playlistVideoListContinuation']);
    const continuationToken = playlistContinuationToken;
    const matchingList = child => !playlistId || !child.playlistId || child.playlistId === playlistId || child.playlistId === `VL${playlistId}`;
    const actualItem = item => !recommendedPlaylistVideo(item) && (item?.playlistVideoRenderer || item?.musicResponsiveListItemRenderer || String(item?.lockupViewModel?.contentType || '').toLowerCase().includes('video'));
    const recommendationOnly = items => Array.isArray(items) && items.some(recommendedPlaylistVideo) && !items.some(actualItem);
    const hasRecommendation = value => value && typeof value === 'object' && (recommendedPlaylistVideo(value) || Object.values(value).some(hasRecommendation));
    const walk = value => {
      if (!value || typeof value !== 'object' || recommendedPlaylistVideo(value)) return false;
      let found = false;
      for (const [key, child] of Object.entries(value)) {
        if (['onResponseReceivedActions', 'onResponseReceivedEndpoints', 'onResponseReceivedCommands'].includes(key)) continue;
        if (recognized.has(key) && child && typeof child === 'object') {
          if (matchingList(child) && !recommendationOnly(child.contents)) { sections.push(child); found = true; }
          continue; // A named item section is one occurrence source, not a recursive duplicate.
        }
        if (key === 'sectionListRenderer' && child && typeof child === 'object') {
          if (walk(child)) {
            found = true;
            // Older responses put the playlist cursor immediately beside its
            // item list. Never recurse into unrelated shelves to find it.
            for (const item of Array.isArray(child.contents) ? child.contents : []) {
              if (!continuationToken(item)) continue;
              if (hasRecommendation(child)) ambiguousOuter = true;
              else outerContinuations.push(item);
            }
          }
        } else if (walk(child)) found = true;
      }
      return found;
    };
    walk(root);
    // Public WEB playlist HTML (2026-10-16) also uses indexed video lockups
    // directly inside the selected browse tab's first item section. Bind every
    // row to this requested playlist; sibling sections/cursors are recommendations.
    if (!sections.length && playlistId) {
      const tabs = root?.contents?.twoColumnBrowseResultsRenderer?.tabs;
      const selected = Array.isArray(tabs) ? tabs.filter(tab => tab?.tabRenderer?.selected === true) : [];
      const firstSection = selected.length === 1 ? selected[0].tabRenderer.content?.sectionListRenderer?.contents?.[0]?.itemSectionRenderer : null;
      const items = firstSection?.contents;
      if (Array.isArray(items) && items.length && items.every((item, index) => {
        const row = item?.lockupViewModel;
        const endpoint = row?.rendererContext?.commandContext?.onTap?.innertubeCommand?.watchEndpoint;
        return row?.contentType === 'LOCKUP_CONTENT_TYPE_VIDEO'
          && typeof row.contentId === 'string' && /^[A-Za-z0-9_-]{11}$/.test(row.contentId)
          && endpoint?.videoId === row.contentId && endpoint.playlistId === playlistId
          && endpoint.index === index;
      })) sections.push({ contents: items });
    }
    // Source: YouTube.js 18.1 AppendContinuationItemsAction.continuationItems.
    // Only playlist-shaped rows/cursors qualify; generic search/video appends
    // and recommendation-only appends are not playlist membership evidence.
    const appends = [];
    const readCommand = (action, depth = 0) => {
      if (!action || typeof action !== 'object' || depth > 4) return;
      if (action.appendContinuationItemsAction) appends.push(action.appendContinuationItemsAction);
      // YouTube.js CommandExecutorCommand parses this explicit command array.
      for (const command of Array.isArray(action.commandExecutorCommand?.commands) ? action.commandExecutorCommand.commands : []) readCommand(command, depth + 1);
    };
    for (const key of ['onResponseReceivedActions', 'onResponseReceivedEndpoints', 'onResponseReceivedCommands']) {
      for (const action of Array.isArray(root?.[key]) ? root[key] : []) readCommand(action);
    }
    for (const append of appends) {
      const rawItems = append?.continuationItems;
      if (!Array.isArray(rawItems)) continue;
      const hasTarget = Object.prototype.hasOwnProperty.call(append, 'target') || Object.prototype.hasOwnProperty.call(append, 'targetId');
      const targets = ['target', 'targetId'].filter(key => Object.prototype.hasOwnProperty.call(append, key)).map(key => append[key]);
      const knownTarget = targets.length > 0 && targets.every(target => target === 'playlist-video-list' || target === playlistId || target === `VL${playlistId}`);
      // Explicit routing wins over row shape: other surfaces can use the same renderer.
      if (hasTarget && !knownTarget) continue;
      // YouTube.js ItemSection parses only data.contents. The observed requested-
      // playlist append wraps its rows here; headers/commands are not membership.
      const flattenItems = (values, depth = 0) => {
        if (depth > 8) { invalidAppendWrapper = true; return []; }
        return values.flatMap(item => {
          if (recommendedPlaylistVideo(item)) return [];
          const contents = knownTarget && item?.itemSectionRenderer?.contents;
          return Array.isArray(contents) ? flattenItems(contents, depth + 1) : [item];
        });
      };
      const items = flattenItems(rawItems);
      if (recommendationOnly(items)) continue;
      const hasRows = items.some(item => !recommendedPlaylistVideo(item) && (item?.playlistVideoRenderer
        || item?.musicResponsiveListItemRenderer?.playlistItemData?.videoId
        || item?.musicResponsiveListItemRenderer?.playlistItemData?.setVideoId
        || (knownTarget && String(item?.lockupViewModel?.contentType || '').toLowerCase().includes('video'))));
      const cursorOnly = items.length > 0 && items.every(item => continuationToken(item));
      if (hasRows || (knownTarget && (cursorOnly || rawItems.length === 0))) sections.push({ contents: items });
    }
    const tokens = new Set();
    let invalidToken = false;
    const addToken = token => {
      if (!token) return;
      if (typeof token !== 'string' || token.length > 16_384) invalidToken = true;
      else tokens.add(token);
    };
    for (const section of sections) {
      for (const item of Array.isArray(section.contents) ? section.contents : []) { const token = continuationToken(item); if (token) addToken(token); }
      for (const item of Array.isArray(section.continuations) ? section.continuations : []) {
        const token = item?.nextContinuationData?.continuation;
        if (token) addToken(token);
      }
    }
    // An explicit item-list continuation wins over outer recommendation cursors.
    const ambiguousFallback = !tokens.size && ambiguousOuter;
    if (!tokens.size) for (const item of outerContinuations) { const token = continuationToken(item); if (token) addToken(token); }
    return { sections, found: sections.length > 0, cursor: tokens.size === 1 && !invalidToken && !ambiguousFallback && !invalidAppendWrapper ? [...tokens][0] : null, ambiguous: tokens.size > 1 || ambiguousFallback || invalidToken || invalidAppendWrapper };
  };
  const playlistEntries = (body, playlistId) => {
    const entries = [];
    const walk = value => {
      if (!value || typeof value !== 'object' || recommendedPlaylistVideo(value)) return;
      if (value.lockupViewModel) {
        const x = value.lockupViewModel, metadata = x.metadata?.lockupMetadataViewModel || x.lockupMetadataViewModel || {};
        const command = x.rendererContext?.commandContext?.onTap?.innertubeCommand || {}, endpoint = command.watchEndpoint || {};
        const type = String(x.contentType || '').toLowerCase(), trackId = first(x.contentId, endpoint.videoId), entryId = first(endpoint.playlistSetVideoId, endpoint.playlistItemId) || null;
        const title = viewText(metadata.title) || viewText(x.title), rows = metadata.metadata?.contentMetadataViewModel?.metadataRows || metadata.metadataRows || [];
        const parts = rows.flatMap(row => (row.metadataParts || []).map(part => viewText(part.text || part)).filter(Boolean));
        const durationText = parts.find(part => /^\d{1,2}:\d{2}(?::\d{2})?$/.test(part)) || '';
        const artist = (rows[0]?.metadataParts || []).map(part => viewText(part.text || part)).filter(part => part && part !== durationText).join(' • ');
        if (type.includes('video') && (trackId || entryId)) entries.push({ entryId, trackId: trackId || null, title: title || 'Unavailable video', artist, album: '', durationSeconds: duration(durationText) || lockupBadgeDuration(x), url: trackId ? `https://www.youtube.com/watch?v=${trackId}` : null, artworkUrl: image(x.contentImage), available: !!trackId && !type.includes('unavailable') });
      }
      if (value.playlistVideoRenderer) {
        const x = value.playlistVideoRenderer, trackId = x.videoId || null, entryId = x.setVideoId || x.playlistVideoId || x.playlistItemData?.playlistSetVideoId || null;
        // Missing legacy flags preserve the occurrence but do not confirm playability.
        const available = typeof trackId === 'string' && /^[A-Za-z0-9_-]{11}$/.test(trackId) && x.isPlayable === true && !x.unplayableText;
        if (trackId || entryId || x.isPlayable === false || x.unplayableText) entries.push({ entryId, trackId, title: text(x.title) || text(x.unplayableText) || 'Unavailable video', artist: text(x.shortBylineText), album: '', durationSeconds: duration(x.lengthText?.simpleText), url: trackId ? `https://www.youtube.com/watch?v=${trackId}` : null, artworkUrl: image(x.thumbnail), available });
      }
      if (value.musicResponsiveListItemRenderer) {
        const x = value.musicResponsiveListItemRenderer, trackId = x.playlistItemData?.videoId || x.navigationEndpoint?.watchEndpoint?.videoId;
        const columns = (x.flexColumns || []).map(column => text(column.musicResponsiveListItemFlexColumnRenderer?.text));
        if (trackId) entries.push({ entryId: x.playlistItemData?.setVideoId || null, trackId, title: columns[0] || '', artist: columns.slice(1).join(' • '), album: '', durationSeconds: duration(text(x.fixedColumns?.[0]?.musicResponsiveListItemFlexColumnRenderer?.text)), url: `https://www.youtube.com/watch?v=${trackId}`, artworkUrl: image(x.thumbnail), available: true });
      }
      for (const child of Object.values(value)) walk(child);
    };
    for (const section of playlistSections(body, playlistId).sections) walk(section.contents || section);
    return entries;
  };
  const playlistPageContinuation = (root, playlistId) => playlistSections(root, playlistId);
  // Finite response structure only. Never copy keys, values, routing IDs or tokens.
  const playlistResponseShape = (body, playlistId, accountId, hl, isContinuation, pageInfo) => {
    // Fixed allowlist covers legacy and newer playlist/header/sidebar nodes and
    // endpoint containers. Counts never include provider keys or values.
    const keys = ['playlistVideoListRenderer', 'playlistVideoListContinuation', 'musicPlaylistShelfRenderer', 'musicPlaylistShelfContinuation', 'sectionListRenderer', 'sectionListContinuation', 'itemSectionRenderer', 'itemSectionContinuation', 'richGridRenderer', 'richGridContinuation', 'playlistVideoRenderer', 'musicResponsiveListItemRenderer', 'lockupViewModel', 'continuationItemRenderer', 'continuationItemViewModel', 'appendContinuationItemsAction', 'reloadContinuationItemsCommand', 'commandExecutorCommand', 'openPopupAction', 'playlistHeaderRenderer', 'musicEditablePlaylistDetailHeaderRenderer', 'musicPlaylistEditHeaderRenderer', 'musicDetailHeaderRenderer', 'musicResponsiveHeaderRenderer', 'playlistSidebarRenderer', 'playlistSidebarPrimaryInfoRenderer', 'playlistSidebarSecondaryInfoRenderer', 'videoOwner', 'videoOwnerRenderer', 'playlistMetadataRenderer', 'pageHeaderRenderer', 'pageHeaderViewModel', 'playlistHeaderViewModel', 'ownerEndpoint', 'ownerText', 'navigationEndpoint', 'browseEndpoint', 'watchEndpoint', 'canonicalBaseUrl', 'canonicalUrl', 'urlCanonical', 'microformatDataRenderer', 'privacy', 'privacyStatus', 'privacyForm', 'dropdownFormFieldRenderer', 'dropdownRenderer', 'privacyDropdownItemRenderer', 'icon', 'editEndpoint', 'playlistEditEndpoint', 'playlistEditorEndpoint', 'changePlaylistPrivacyEndpoint'];
    const buckets = ['contents', 'continuationContents', 'onResponseReceivedActions', 'onResponseReceivedEndpoints', 'onResponseReceivedCommands', 'sidebar', 'header', 'other'];
    const counts = Object.fromEntries(buckets.map(bucket => [bucket, Object.fromEntries(keys.map(key => [key, 0]))]));
    const targets = { absent: 0, playlistList: 0, requestedPlaylist: 0, other: 0, malformed: 0 };
    let visited = 0, truncated = false;
    const walk = (value, bucket, depth = 0) => {
      if (!value || typeof value !== 'object') return;
      if (depth > 64 || ++visited > 30000) { truncated = true; return; }
      for (const [key, child] of Object.entries(value)) {
        if (keys.includes(key)) counts[bucket][key] = Math.min(10000, counts[bucket][key] + 1);
        if (key === 'appendContinuationItemsAction' && child && typeof child === 'object') {
          const supplied = ['target', 'targetId'].filter(name => Object.prototype.hasOwnProperty.call(child, name));
          const category = !supplied.length ? 'absent' : supplied.some(name => typeof child[name] !== 'string' || !child[name]) ? 'malformed' : supplied.every(name => child[name] === 'playlist-video-list') ? 'playlistList' : supplied.every(name => child[name] === playlistId || child[name] === `VL${playlistId}`) ? 'requestedPlaylist' : 'other';
          targets[category] = Math.min(10000, targets[category] + 1);
        }
        walk(child, bucket, depth + 1);
      }
    };
    for (const [key, value] of Object.entries(body || {})) walk(value, buckets.includes(key) ? key : 'other');
    const emptyMembership = () => ({ ordinaryRows: 0, recommendedRows: 0, unavailableRows: 0, rowsWithOccurrenceId: 0 });
    const membership = { allResponse: emptyMembership(), recognizedSections: emptyMembership() };
    const emptyPlayability = () => ({ explicitTrue: 0, explicitFalse: 0, missing: 0, invalid: 0, withWatchEndpoint: 0, knownPlaceholder: 0 });
    const playability = { allResponse: emptyPlayability(), recognizedSections: emptyPlayability() };
    const cursorLocations = { namedListDirect: 0, namedListItemSection: 0, sectionListDirect: 0, sectionListOtherItemSection: 0, other: 0 };
    const inc = (object, key) => { object[key] = Math.min(10000, object[key] + 1); };
    // Report only bounded counts and comparisons; never echo channel IDs or metadata values.
    const identityEvidence = { targetHeaders: 0, primaryPlaylistIds: 0, primaryNavigationPlaylistIds: 0, canonicalPlaylistIds: 0, editEndpointPlaylistIds: 0, privacyFieldPresent: 0, privateHeaders: 0, publicHeaders: 0, unlistedHeaders: 0, unknownPrivacyHeaders: 0, ownerEndpoints: 0, ownerMatchesConfirmedAccount: 0, ownerMismatches: 0, sidebarOwnerEndpoints: 0, sidebarOwnerMatchesConfirmedAccount: 0, sidebarOwnerMismatches: 0, sidebarPrivacyFields: 0, sidebarPrivateValues: 0, sidebarPublicValues: 0, sidebarUnlistedValues: 0, sidebarUnknownPrivacyValues: 0, privacyDropdownEntries: 0, privacyDropdownSelectedPrivate: 0, privacyDropdownSelectedPublic: 0, privacyDropdownSelectedUnlisted: 0, privacyDropdownSelectedUnknown: 0, sidebarVideoOwnerNodes: 0, sidebarVideoOwnerBrowseEndpoints: 0, sidebarVideoOwnerMatchesConfirmedAccount: 0, sidebarVideoOwnerMismatches: 0 };
    let evidenceVisits = 0, evidenceTruncated = false;
    const inspectIdentity = (value, inSidebar = false, inVideoOwner = false, depth = 0) => {
      if (!value || typeof value !== 'object') return;
      if (depth > 64 || ++evidenceVisits > 30000) { evidenceTruncated = true; return; }
      const sidebar = inSidebar || Object.prototype.hasOwnProperty.call(value, 'playlistSidebarRenderer');
      if (Object.prototype.hasOwnProperty.call(value, 'playlistSidebarPrimaryInfoRenderer')) {
        const primary = value.playlistSidebarPrimaryInfoRenderer;
        if (primary?.playlistId === playlistId || primary?.playlistId === `VL${playlistId}`) inc(identityEvidence, 'primaryPlaylistIds');
        const nav = primary?.navigationEndpoint?.watchEndpoint?.playlistId || primary?.navigationEndpoint?.browseEndpoint?.browseId;
        if (nav === playlistId || nav === `VL${playlistId}`) inc(identityEvidence, 'primaryNavigationPlaylistIds');
      }
      if (Object.prototype.hasOwnProperty.call(value, 'microformatDataRenderer')) {
        const canonical = value.microformatDataRenderer?.urlCanonical ?? value.microformatDataRenderer?.canonicalUrl;
        try {
          const u = new URL(canonical);
          if (['youtube.com', 'www.youtube.com', 'music.youtube.com'].includes(u.hostname) && (u.searchParams.get('list') === playlistId || u.searchParams.get('list') === `VL${playlistId}`)) inc(identityEvidence, 'canonicalPlaylistIds');
        } catch { /* malformed and foreign URLs remain unknown */ }
      }
      if (Object.prototype.hasOwnProperty.call(value, 'editEndpoint') || Object.prototype.hasOwnProperty.call(value, 'playlistEditEndpoint')) {
        const edit = value.playlistEditEndpoint || value.editEndpoint;
        const editIds = [edit?.playlistId, edit?.browseEndpoint?.browseId, edit?.watchEndpoint?.playlistId, edit?.editPlaylistEndpoint?.playlistId];
        if (editIds.some(id => id === playlistId || id === `VL${playlistId}`)) inc(identityEvidence, 'editEndpointPlaylistIds');
      }
      for (const key of ['playlistHeaderRenderer', 'musicPlaylistEditHeaderRenderer', 'musicEditablePlaylistDetailHeaderRenderer']) {
        const header = value[key];
        if (!header || typeof header !== 'object' || ![playlistId, `VL${playlistId}`].includes(header.playlistId)) continue;
        inc(identityEvidence, 'targetHeaders');
        if (Object.prototype.hasOwnProperty.call(header, 'privacy')) {
          inc(identityEvidence, 'privacyFieldPresent');
          if (header.privacy === 'PRIVATE') inc(identityEvidence, 'privateHeaders');
          else if (header.privacy === 'PUBLIC') inc(identityEvidence, 'publicHeaders');
          else if (header.privacy === 'UNLISTED') inc(identityEvidence, 'unlistedHeaders');
          else inc(identityEvidence, 'unknownPrivacyHeaders');
        }
        const owner = header.ownerEndpoint?.browseEndpoint?.browseId;
        if (typeof owner === 'string' && owner) {
          inc(identityEvidence, 'ownerEndpoints');
          inc(identityEvidence, owner === accountId ? 'ownerMatchesConfirmedAccount' : 'ownerMismatches');
          if (sidebar) inc(identityEvidence, owner === accountId ? 'sidebarOwnerMatchesConfirmedAccount' : 'sidebarOwnerMismatches');
        }
      }
      if (sidebar && (Object.prototype.hasOwnProperty.call(value, 'videoOwner') || Object.prototype.hasOwnProperty.call(value, 'videoOwnerRenderer'))) inc(identityEvidence, 'sidebarVideoOwnerNodes');
      const ownerNode = inVideoOwner || Object.prototype.hasOwnProperty.call(value, 'videoOwner') || Object.prototype.hasOwnProperty.call(value, 'videoOwnerRenderer');
      if (sidebar && ownerNode && Object.prototype.hasOwnProperty.call(value, 'browseEndpoint')) {
        const owner = value.browseEndpoint?.browseId;
        if (typeof owner === 'string' && /^UC[A-Za-z0-9_-]{20,30}$/.test(owner)) {
          inc(identityEvidence, 'sidebarVideoOwnerBrowseEndpoints');
          inc(identityEvidence, owner === accountId ? 'sidebarVideoOwnerMatchesConfirmedAccount' : 'sidebarVideoOwnerMismatches');
        }
      }
      if (sidebar && Object.prototype.hasOwnProperty.call(value, 'ownerEndpoint')) {
        const owner = value.ownerEndpoint?.browseEndpoint?.browseId;
        if (typeof owner === 'string' && owner) {
          inc(identityEvidence, 'sidebarOwnerEndpoints');
          inc(identityEvidence, owner === accountId ? 'sidebarOwnerMatchesConfirmedAccount' : 'sidebarOwnerMismatches');
        }
      }
      if (sidebar && Object.prototype.hasOwnProperty.call(value, 'privacy')) {
        inc(identityEvidence, 'sidebarPrivacyFields');
        if (value.privacy === 'PRIVATE') inc(identityEvidence, 'sidebarPrivateValues');
        else if (value.privacy === 'PUBLIC') inc(identityEvidence, 'sidebarPublicValues');
        else if (value.privacy === 'UNLISTED') inc(identityEvidence, 'sidebarUnlistedValues');
        else inc(identityEvidence, 'sidebarUnknownPrivacyValues');
      }
      const entries = value.dropdownRenderer?.entries;
      if (sidebar && Array.isArray(entries)) for (const entry of entries) {
        const item = entry?.privacyDropdownItemRenderer;
        if (!item) continue;
        inc(identityEvidence, 'privacyDropdownEntries');
        if (item.isSelected === true && item.icon?.iconType === 'PRIVACY_PRIVATE') inc(identityEvidence, 'privacyDropdownSelectedPrivate');
        else if (item.isSelected === true && item.icon?.iconType === 'PRIVACY_PUBLIC') inc(identityEvidence, 'privacyDropdownSelectedPublic');
        else if (item.isSelected === true && item.icon?.iconType === 'PRIVACY_UNLISTED') inc(identityEvidence, 'privacyDropdownSelectedUnlisted');
        else if (item.isSelected === true) inc(identityEvidence, 'privacyDropdownSelectedUnknown');
      }
      for (const [key, child] of Object.entries(value)) inspectIdentity(child, sidebar, ownerNode || key === 'videoOwner' || key === 'videoOwnerRenderer', depth + 1);
    };
    inspectIdentity(body);
    // Empty rows are meaningful only alongside explicit, parseable count evidence.
    // Keep this diagnostic categorical and do not return source values or text.
    const totalEvidence = { sidebarPrimaryInfoRenderers: 0, sidebarStatsArrays: 0, sidebarFirstStatPresent: 0, sidebarFirstStatParseable: 0, sidebarFirstStatNumericZero: 0, sidebarFirstStatEnglishNoVideos: 0, sidebarFirstStatEnglishLocale: /^en(?:[-_]|$)/i.test(hl), sidebarFirstStatTextShape: { string: 0, simpleText: 0, runs: 0, content: 0, dynamicTextViewModel: 0, textContent: 0, accessibilityLabel: 0, other: 0 }, exactTotalRecognized: false, exactTotalIsZero: false, truncated: false };
    let totalVisits = 0;
    const inspectTotal = (value, depth = 0) => {
      if (!value || typeof value !== 'object' || depth > 64) return;
      if (++totalVisits > 30000) { totalEvidence.truncated = true; return; }
      if (value.playlistSidebarPrimaryInfoRenderer && typeof value.playlistSidebarPrimaryInfoRenderer === 'object') {
        const primary = value.playlistSidebarPrimaryInfoRenderer;
        inc(totalEvidence, 'sidebarPrimaryInfoRenderers');
        if (Array.isArray(primary.stats)) {
          inc(totalEvidence, 'sidebarStatsArrays');
          if (Object.prototype.hasOwnProperty.call(primary.stats, 0)) {
            inc(totalEvidence, 'sidebarFirstStatPresent');
            const stat = primary.stats[0];
            const shapeName = typeof stat === 'string' ? 'string' : typeof stat === 'number' ? 'other' : typeof stat !== 'object' || stat === null ? 'other' : typeof stat.simpleText === 'string' ? 'simpleText' : Array.isArray(stat.runs) ? 'runs' : typeof stat.content === 'string' ? 'content' : typeof stat.dynamicTextViewModel?.text?.content === 'string' ? 'dynamicTextViewModel' : typeof stat.text?.content === 'string' ? 'textContent' : typeof stat.accessibility?.accessibilityData?.label === 'string' ? 'accessibilityLabel' : 'other';
            inc(totalEvidence.sidebarFirstStatTextShape, shapeName);
            const numeric = Number.isSafeInteger(stat) && stat >= 0 ? stat : (() => {
              const match = viewText(stat).trim().match(/^([\d,]+)(?:\s+(?:videos?|songs?|tracks?))?$/i);
              if (!match) return null;
              const parsed = Number(match[1].replace(/,/g, ''));
              return Number.isSafeInteger(parsed) ? parsed : null;
            })();
            if (numeric !== null) {
              inc(totalEvidence, 'sidebarFirstStatParseable');
              if (numeric === 0) inc(totalEvidence, 'sidebarFirstStatNumericZero');
            } else if (totalEvidence.sidebarFirstStatEnglishLocale && /^no videos$/i.test(viewText(stat).normalize('NFKC').trim().replace(/\s+/g, ' '))) {
              // yt-dlp's YouTube count parser treats the label prefix "No " as
              // zero. Restrict our diagnostic to the precise English playlist
              // label observed in this primary item-count slot.
              inc(totalEvidence, 'sidebarFirstStatParseable');
              inc(totalEvidence, 'sidebarFirstStatEnglishNoVideos');
            }
          }
        }
      }
      for (const child of Object.values(value)) inspectTotal(child, depth + 1);
    };
    inspectTotal(body);
    const tokenOf = playlistContinuationToken;
    const namedKeys = new Set(['playlistVideoListRenderer', 'playlistVideoListContinuation', 'musicPlaylistShelfRenderer', 'musicPlaylistShelfContinuation']);
    const memoBuckets = ['contents', 'continuationContents', 'onResponseReceivedCommands', 'onResponseReceivedEndpoints', 'onResponseReceivedActions', 'sidebar', 'header'];
    const candidates = [], sectionLists = [];
    let structureVisits = 0;
    const inspect = (value, ancestors, rowCounts, recordCursors, depth = 0) => {
      if (!value || typeof value !== 'object') return;
      if (depth > 64 || ++structureVisits > 30000) { truncated = true; return; }
      if (value.playlistVideoRenderer) {
        const row = value.playlistVideoRenderer;
        const flags = rowCounts === membership.allResponse ? playability.allResponse : playability.recognizedSections;
        const present = Object.prototype.hasOwnProperty.call(row, 'isPlayable');
        inc(flags, row.isPlayable === true ? 'explicitTrue' : row.isPlayable === false ? 'explicitFalse' : present ? 'invalid' : 'missing');
        if (row.navigationEndpoint?.watchEndpoint && typeof row.navigationEndpoint.watchEndpoint === 'object') inc(flags, 'withWatchEndpoint');
        if (/^(?:\[(?:private|deleted) video\]|(?:private|deleted) video)$/i.test(viewText(row.title).trim())) inc(flags, 'knownPlaceholder');
        inc(rowCounts, row.style === 'PLAYLIST_VIDEO_RENDERER_STYLE_RECOMMENDED_VIDEO' ? 'recommendedRows' : 'ordinaryRows');
        // Diagnostic unavailableRows means not confirmed playable, including absent flags.
        if (row.isPlayable !== true || row.unplayableText || typeof row.videoId !== 'string' || !/^[A-Za-z0-9_-]{11}$/.test(row.videoId)) inc(rowCounts, 'unavailableRows');
        if (row.setVideoId || row.playlistVideoId || row.playlistItemData?.playlistSetVideoId) inc(rowCounts, 'rowsWithOccurrenceId');
      }
      if (recordCursors && (value.continuationItemRenderer || value.continuationItemViewModel)) {
        candidates.push({ value, ancestors, token: tokenOf(value), view: Boolean(value.continuationItemViewModel) });
      }
      for (const [key, child] of Object.entries(value)) {
        if (recordCursors && key === 'sectionListRenderer' && child && typeof child === 'object') sectionLists.push(child);
        inspect(child, [...ancestors, { key, value: child }], rowCounts, recordCursors, depth + 1);
      }
    };
    // Feed.concatMemos order, then Memo.getType(ContinuationItem, ContinuationItemView).
    for (const key of [...memoBuckets, ...Object.keys(body || {}).filter(key => !memoBuckets.includes(key))]) inspect(body?.[key], [{ key, value: body?.[key] }], membership.allResponse, memoBuckets.includes(key));
    structureVisits = 0;
    for (const section of pageInfo.sections) inspect(section.contents || section, [], membership.recognizedSections, false);
    const firstSection = sectionLists[0];
    const firstSectionItems = new Set(Array.isArray(firstSection?.contents) ? firstSection.contents : []);
    const directSectionItems = new Set(sectionLists.flatMap(section => Array.isArray(section.contents) ? section.contents : []));
    const ordered = [...candidates.filter(candidate => !candidate.view), ...candidates.filter(candidate => candidate.view)];
    const namedDirectItems = new WeakMap();
    for (const candidate of candidates) {
      const namedAt = candidate.ancestors.findLastIndex(parent => namedKeys.has(parent.key));
      const itemAt = candidate.ancestors.findLastIndex(parent => parent.key === 'itemSectionRenderer');
      const directSection = directSectionItems.has(candidate.value);
      const inSection = candidate.ancestors.some(parent => parent.key === 'sectionListRenderer');
      const named = namedAt >= 0 ? candidate.ancestors[namedAt].value : null;
      if (named && !namedDirectItems.has(named)) namedDirectItems.set(named, new Set(Array.isArray(named.contents) ? named.contents : []));
      const namedDirect = named ? namedDirectItems.get(named).has(candidate.value) : false;
      const location = namedAt >= 0 ? (itemAt > namedAt ? 'namedListItemSection' : namedDirect ? 'namedListDirect' : 'other') : directSection ? 'sectionListDirect' : inSection && itemAt >= 0 ? 'sectionListOtherItemSection' : 'other';
      inc(cursorLocations, location);
    }
    const retainedCandidate = ordered.find(candidate => firstSection ? !firstSectionItems.has(candidate.value) : !candidate.ancestors.some(parent => parent.key === 'header'));
    const exactTotal = playlistExactTotal(body, hl);
    totalEvidence.exactTotalRecognized = exactTotal !== null;
    totalEvidence.exactTotalIsZero = exactTotal === 0;
    totalEvidence.sidebarFirstStatZero = totalEvidence.sidebarFirstStatNumericZero + totalEvidence.sidebarFirstStatEnglishNoVideos;
    return { version: 1, continuation: isContinuation, counts, appendTargets: targets, recognizedSections: Math.min(10000, pageInfo.sections.length), recognizedCursor: Boolean(pageInfo.cursor), ambiguous: pageInfo.ambiguous, truncated: truncated || evidenceTruncated || totalEvidence.truncated, membership, playability, cursorLocations, retainedRuleCandidatePresent: Boolean(retainedCandidate), retainedRuleEqualsSelected: Boolean(retainedCandidate?.token && retainedCandidate.token === pageInfo.cursor), identityEvidence: { ...identityEvidence, ...modernReadbackReasons(body, playlistId, accountId, hl), ...modernEditEvidence(body, playlistId) }, totalEvidence };
  };
  const playlistExactTotal = (body, hl = 'en') => {
    const header = body.header || {}, metadata = header.musicDetailHeaderRenderer || header.playlistHeaderRenderer || {};
    const pageView = body.contents?.pageHeaderViewModel || header.pageHeaderViewModel || header.pageHeaderRenderer?.content?.pageHeaderViewModel || body.pageHeaderRenderer?.content?.pageHeaderViewModel || {};
    const playlistMetadata = body.metadata?.playlistMetadataRenderer || body.playlistMetadataRenderer || {};
    const sidebarItems = body.sidebar?.playlistSidebarRenderer?.items;
    const primaryInfo = (Array.isArray(sidebarItems) ? sidebarItems : []).map(item => item?.playlistSidebarPrimaryInfoRenderer).filter(Boolean);
    // YouTube.js Playlist reads total_items from primary sidebar stats[0];
    // stats[1] is views and must never be used as the playlist item count.
    const sidebarTotal = primaryInfo.length === 1 ? primaryInfo[0].stats?.[0] : undefined;
    for (const value of [playlistMetadata.videoCount, playlistMetadata.trackCount, metadata.secondSubtitle, metadata.numVideosText, pageView.metadata, sidebarTotal]) {
      if (Number.isSafeInteger(value) && value >= 0) return value;
      const match = viewText(value).trim().match(/^([\d,]+)(?:\s+(?:videos?|songs?|tracks?))?$/i);
      if (match) {
        const parsed = Number(match[1].replace(/,/g, ''));
        if (Number.isSafeInteger(parsed)) return parsed;
      }
      if (value === sidebarTotal && /^en(?:[-_]|$)/i.test(hl)
          && /^no videos$/i.test(viewText(value).normalize('NFKC').trim().replace(/\s+/g, ' '))) return 0;
    }
    return null;
  };

// HTTP urlCanonical is declarative metadata observed on owned empty WEB pages.
// It is used only to compare playlist identity; requests remain HTTPS.
// Paths grounded in yt-dlp 8973412 _tab.py 730-740,800-838; _base.py1127.
function youtubeWriteCapabilities(body,id,accountId,hl) {
  const denied={add:false,remove:false,reorder:false};
  if(['LM','SE','LL','WL'].includes(id))return denied;
  if(editablePlaylistHeader(body,id,accountId))return {add:true,remove:true,reorder:true};
  if(!/^PL[A-Za-z0-9_-]{8,78}$/.test(id) || !modernExplicitEmpty(body,id,accountId,hl,playlistExactTotal(body,hl)))return denied;
  const evidence=modernEditEvidence(body,id);
  // Supported metadata controls establish this authenticated owner's editing
  // context; the provider contract supports adding to an ordinary owned list.
  // Empty lists cannot supply remove/reorder occurrences or item-list flags.
  const add=evidence.editMatchedCommands>0 && evidence.editApiPathMatched===evidence.editMatchedCommands
    && evidence.editForeignCommands===0 && evidence.editMalformedCommands===0
    && evidence.editTraversalTruncated===0 && (evidence.editActionMetadata>0 || evidence.editActionPrivacy>0);
  return {add,remove:false,reorder:false};
}

function modernEditEvidence(body,id) {
  const result={editMatchedCommands:0,editForeignCommands:0,editMalformedCommands:0,editApiPathMatched:0,editParamsPresent:0,editActionsMissing:0,editActionsEmpty:0,editActionsMalformed:0,editActionAdd:0,editActionRemove:0,editActionMove:0,editActionPrivacy:0,editActionMetadata:0,editActionOther:0,editActionCollection:0,editActionCollaboration:0,editActionVoting:0,editFlagTrue:0,editFlagFalse:0,editFlagMalformed:0,editFlagUnbound:0,editReorderTrue:0,editTraversalTruncated:0};
  const inc=k=>{result[k]=Math.min(10000,result[k]+1);};
  const matches=x=>x===id || x===`VL${id}`;
  const actions=new Map([['ACTION_ADD_VIDEO','editActionAdd'],['ACTION_REMOVE_VIDEO','editActionRemove'],['ACTION_MOVE_VIDEO_BEFORE','editActionMove'],['ACTION_SET_PLAYLIST_PRIVACY','editActionPrivacy'],['ACTION_SET_PLAYLIST_NAME','editActionMetadata'],['ACTION_SET_PLAYLIST_DESCRIPTION','editActionMetadata'],['ACTION_SET_PLAYLIST_VIDEO_ORDER','editActionMetadata'],['ACTION_SET_ADD_TO_TOP','editActionMetadata'],['ACTION_ADD_PLAYLIST','editActionCollection'],['ACTION_JOIN_COLLABORATION','editActionCollaboration'],['ACTION_CREATE_COLLABORATION_INVITE_LINK','editActionCollaboration'],['ACTION_SET_CLOSED_TO_CONTRIBUTIONS','editActionCollaboration'],['ACTION_SET_ALLOW_ITEM_VOTE','editActionVoting']]);
  let visits=0;
  const walk=(value,depth=0)=>{
    if(!value || typeof value!=='object')return;
    if(++visits>10000 || depth>40){result.editTraversalTruncated=1;return;}
    if(Object.prototype.hasOwnProperty.call(value,'playlistEditEndpoint')){
      const endpoint=value.playlistEditEndpoint;
      if(typeof endpoint?.playlistId!=='string' || !endpoint.playlistId)inc('editMalformedCommands');
      else if(!matches(endpoint.playlistId))inc('editForeignCommands');
      else {
        inc('editMatchedCommands');
        if(value.commandMetadata?.webCommandMetadata?.apiUrl==='/youtubei/v1/browse/edit_playlist')inc('editApiPathMatched');
        if(typeof endpoint.params==='string' && endpoint.params)inc('editParamsPresent');
        if(endpoint.actions===undefined)inc('editActionsMissing');
        else if(!Array.isArray(endpoint.actions))inc('editActionsMalformed');
        else if(!endpoint.actions.length)inc('editActionsEmpty');
        else {
          if(endpoint.actions.length>10000)result.editTraversalTruncated=1;
          for(const action of endpoint.actions.slice(0,10000)){
            if(!action || typeof action!=='object' || typeof action.action!=='string')inc('editActionsMalformed');
            else inc(actions.get(action.action)||'editActionOther');
          }
        }
      }
    }
    for(const renderer of ['playlistHeaderRenderer','playlistVideoListRenderer']){
      const row=value[renderer];if(!row || typeof row!=='object')continue;
      if(Object.prototype.hasOwnProperty.call(row,'isEditable')){
        if(!matches(row.playlistId))inc('editFlagUnbound');
        else inc(row.isEditable===true?'editFlagTrue':row.isEditable===false?'editFlagFalse':'editFlagMalformed');
      }
      if(renderer==='playlistVideoListRenderer' && matches(row.playlistId) && row.canReorder===true)inc('editReorderTrue');
    }
    for(const child of Object.values(value))walk(child,depth+1);
  };
  walk(body.header);walk(body.sidebar);walk(body.contents);
  return result;
}

function canonicalRejectionShape(value,id) {
  const shape={canonicalRejectReason:0,canonicalProtocolClass:0,canonicalHostClass:0,canonicalHasCredentials:0,canonicalHasPort:0,canonicalPathClass:0,canonicalListCount:0,canonicalListMatches:0};
  if(typeof value!=='string'){shape.canonicalRejectReason=1;return shape;}
  let u;try{u=new URL(value);}catch{shape.canonicalRejectReason=2;return shape;}
  shape.canonicalProtocolClass=u.protocol==='https:'?1:u.protocol==='http:'?2:3;
  shape.canonicalHostClass=u.hostname==='www.youtube.com'?1:u.hostname==='youtube.com'?2:u.hostname==='music.youtube.com'?3:4;
  shape.canonicalHasCredentials=Number(Boolean(u.username||u.password));
  shape.canonicalHasPort=Number(Boolean(u.port));
  shape.canonicalPathClass=u.pathname==='/playlist'?1:u.pathname==='/watch'?2:3;
  shape.canonicalListCount=Math.min(10000,u.searchParams.getAll('list').length);
  shape.canonicalListMatches=Number(u.searchParams.get('list')===id);
  shape.canonicalRejectReason=shape.canonicalHasCredentials?3:![1,2].includes(shape.canonicalProtocolClass)?4:shape.canonicalHostClass!==1?5:shape.canonicalHasPort?6:shape.canonicalPathClass!==1?7:shape.canonicalListCount!==1?8:!shape.canonicalListMatches?9:0;
  return shape;
}

function modernReadbackReasons(body,id,accountId,hl) {
  const result={canonicalRejectReason:0,canonicalProtocolClass:0,canonicalHostClass:0,canonicalHasCredentials:0,canonicalHasPort:0,canonicalPathClass:0,canonicalListCount:0,canonicalListMatches:0,emptyRejectReason:0,emptyContinuationHeader:0,emptyContinuationSidebar:0,emptyContinuationContents:0,emptyContinuationOther:0,emptyMembershipNodes:0,emptyTraversalTruncated:0,modernPrimaryCount:0,modernSecondaryCount:0,modernMatchingIds:0,modernConflictingIds:0,modernMalformedEditIds:0,modernMalformedTitleRuns:0,modernMatchingCanonicals:0,modernRejectedCanonicals:0,modernTraversalTruncated:0,modernOwnerRuns:0,modernMatchingOwners:0,modernConflictingOwners:0,modernMalformedOwnerRuns:0,modernSelectedPrivacyCount:0,modernTargetBound:0,modernIdentityRecognized:0,modernSelectedTabs:0,modernMemberSections:0,modernMessageOnlySection:0,modernEmptyRecognized:0};
  const inc=k=>{result[k]=Math.min(10000,result[k]+1);};
  const record=x=>inc(x===id || x===`VL${id}`?'modernMatchingIds':'modernConflictingIds');
  const items=body.sidebar?.playlistSidebarRenderer?.items;
  const primary=(Array.isArray(items)?items:[]).filter(x=>x?.playlistSidebarPrimaryInfoRenderer).map(x=>x.playlistSidebarPrimaryInfoRenderer);
  const secondary=(Array.isArray(items)?items:[]).filter(x=>x?.playlistSidebarSecondaryInfoRenderer).map(x=>x.playlistSidebarSecondaryInfoRenderer);
  result.modernPrimaryCount=Math.min(10000,primary.length);result.modernSecondaryCount=Math.min(10000,secondary.length);
  for(const row of primary){
    if(row.playlistId!==undefined)record(row.playlistId);
    const runs=row.title?.runs;
    if(runs!==undefined && !Array.isArray(runs))inc('modernMalformedTitleRuns');
    for(const ep of [row.navigationEndpoint,...(Array.isArray(runs)?runs.slice(0,10000).map(x=>x?.navigationEndpoint):[])]){
      if(ep?.browseEndpoint?.browseId!==undefined)record(ep.browseEndpoint.browseId);
      if(ep?.watchEndpoint?.playlistId!==undefined)record(ep.watchEndpoint.playlistId);
    }
    const entries=row.privacyForm?.dropdownFormFieldRenderer?.dropdown?.dropdownRenderer?.entries;
    for(const entry of Array.isArray(entries)?entries.slice(0,10000):[])if(entry?.privacyDropdownItemRenderer?.isSelected===true)inc('modernSelectedPrivacyCount');
  }
  for(const row of secondary){
    const runs=row.videoOwner?.videoOwnerRenderer?.title?.runs;
    if(runs!==undefined && !Array.isArray(runs))inc('modernMalformedOwnerRuns');
    for(const run of Array.isArray(runs)?runs.slice(0,10000):[]){
      inc('modernOwnerRuns');const owner=run?.navigationEndpoint?.browseEndpoint?.browseId;
      if(owner)inc(owner===accountId?'modernMatchingOwners':'modernConflictingOwners');
    }
  }
  let visits=0;
  const walk=(value,depth=0)=>{
    if(!value || typeof value!=='object')return;
    if(++visits>10000 || depth>40){result.modernTraversalTruncated=1;return;}
    for(const [key,child] of Object.entries(value)){
      if(['playlistHeaderRenderer','musicPlaylistEditHeaderRenderer','musicEditablePlaylistDetailHeaderRenderer'].includes(key) && child?.playlistId!==undefined)record(child.playlistId);
      if(key==='playlistEditEndpoint'){
        if(typeof child?.playlistId!=='string' || !child.playlistId)inc('modernMalformedEditIds');else record(child.playlistId);
      }
      walk(child,depth+1);
    }
  };
  walk(body.header);walk(body.sidebar);
  let canonicalClassified=false;
  for(const value of [body.microformat?.microformatDataRenderer?.urlCanonical,body.microformat?.microformatDataRenderer?.canonicalUrl]){
    if(value===undefined)continue;
    const classified=canonicalRejectionShape(value,id);
    inc(classified.canonicalRejectReason===0?'modernMatchingCanonicals':'modernRejectedCanonicals');
    if(!canonicalClassified || (classified.canonicalRejectReason!==0 && result.canonicalRejectReason===0)){Object.assign(result,classified);canonicalClassified=true;}
  }
  if(primary.length===1 && secondary.length===1){
    result.modernTargetBound=Number(modernPlaylistTargetBound(body,id,primary[0]));
    result.modernIdentityRecognized=Number(Boolean(modernSidebarIdentity(body,id,accountId)));
  }
  const tabs=body.contents?.twoColumnBrowseResultsRenderer?.tabs;
  const selected=(Array.isArray(tabs)?tabs:[]).filter(x=>x?.tabRenderer?.selected===true);
  result.modernSelectedTabs=Math.min(10000,selected.length);
  const sections=selected.length===1?selected[0].tabRenderer.content?.sectionListRenderer?.contents:null;
  result.modernMemberSections=Array.isArray(sections)?Math.min(10000,sections.length):0;
  const memberItems=Array.isArray(sections)&&sections.length===1?sections[0]?.itemSectionRenderer?.contents:null;
  result.modernMessageOnlySection=Number(Array.isArray(memberItems)&&memberItems.every(x=>x?.messageRenderer && Object.keys(x).length===1));
  result.modernEmptyRecognized=Number(modernExplicitEmpty(body,id,accountId,hl,playlistExactTotal(body,hl),result));
  return result;
}

function modernPlaylistTargetBound(body, id, primary) {
  const ids=[];
  const target=x=>x===id || x===`VL${id}`;
  if(primary.playlistId!==undefined)ids.push(primary.playlistId);
  const titleRuns=primary.title?.runs;
  if(titleRuns!==undefined && !Array.isArray(titleRuns))return false;
  for(const endpoint of [primary.navigationEndpoint,...(Array.isArray(titleRuns)?titleRuns.map(x=>x?.navigationEndpoint):[])]) {
    if(endpoint?.browseEndpoint?.browseId!==undefined)ids.push(endpoint.browseEndpoint.browseId);
    if(endpoint?.watchEndpoint?.playlistId!==undefined)ids.push(endpoint.watchEndpoint.playlistId);
  }
  // Empty playlists omit a play/navigation endpoint. Their own header/sidebar
  // edit commands still explicitly name the playlist without executing a write.
  let invalid=false,visits=0;
  function walk(value,depth=0){
    if(!value || typeof value!=='object')return;
    if(++visits>10000 || depth>40){invalid=true;return;}
    for(const [key,child] of Object.entries(value)){
      if(['playlistHeaderRenderer','musicPlaylistEditHeaderRenderer','musicEditablePlaylistDetailHeaderRenderer'].includes(key) && child?.playlistId!==undefined)ids.push(child.playlistId);
      if(key==='playlistEditEndpoint'){
        if(typeof child?.playlistId!=='string' || !child.playlistId)invalid=true;
        else ids.push(child.playlistId);
      }
      walk(child,depth+1);
    }
  }
  walk(body.header);walk(body.sidebar);
  for(const value of [body.microformat?.microformatDataRenderer?.urlCanonical,body.microformat?.microformatDataRenderer?.canonicalUrl]){
    if(value===undefined)continue;
    // Parse and compare metadata only. Never fetch or navigate this URL.
    if(canonicalRejectionShape(value,id).canonicalRejectReason!==0){invalid=true;continue;}
    ids.push(id);
  }
  return !invalid && ids.length>0 && ids.every(target);
}

function modernSidebarIdentity(body, id, accountId) {
  const items = body.sidebar?.playlistSidebarRenderer?.items;
  if (!Array.isArray(items)) return null;
  const primary = items.filter(x=>x?.playlistSidebarPrimaryInfoRenderer).map(x=>x.playlistSidebarPrimaryInfoRenderer);
  const secondary = items.filter(x=>x?.playlistSidebarSecondaryInfoRenderer).map(x=>x.playlistSidebarSecondaryInfoRenderer);
  if (primary.length !== 1 || secondary.length !== 1) return null;
  if (!modernPlaylistTargetBound(body,id,primary[0])) return null;
  const ownerRuns = secondary[0].videoOwner?.videoOwnerRenderer?.title?.runs;
  const owners = (Array.isArray(ownerRuns) ? ownerRuns : []).map(r=>r?.navigationEndpoint?.browseEndpoint?.browseId).filter(Boolean);
  if (owners.length!==1 || owners[0]!==accountId || !/^UC[A-Za-z0-9_-]{20,30}$/.test(owners[0])) return null;
  const entries = primary[0].privacyForm?.dropdownFormFieldRenderer?.dropdown?.dropdownRenderer?.entries;
  if (!Array.isArray(entries)) return null;
  const selected = entries.map(e=>e?.privacyDropdownItemRenderer).filter(e=>e?.isSelected===true);
  if (selected.length!==1) return null;
  const privacy = new Map([['PRIVACY_PRIVATE','private'],['PRIVACY_PUBLIC','public'],['PRIVACY_UNLISTED','unlisted']]).get(selected[0].icon?.iconType);
  if (!privacy) return null;
  return {ownerId:owners[0],visibility:privacy};
}

function modernExplicitEmpty(body,id,accountId,hl,decodedTotal,evidence=null) {
  const fail=code=>{if(evidence)evidence.emptyRejectReason=code;return false;};
  if (!/^en(?:[-_]|$)/i.test(hl)) return fail(1);
  if (decodedTotal !== 0) return fail(2);
  if (modernSidebarIdentity(body,id,accountId)?.visibility !== 'private') return fail(3);
  const primary=body.sidebar.playlistSidebarRenderer.items.find(x=>x.playlistSidebarPrimaryInfoRenderer).playlistSidebarPrimaryInfoRenderer;
  const stat=primary.stats?.[0];
  const statText=typeof stat?.simpleText==='string'?stat.simpleText:Array.isArray(stat?.runs)?stat.runs.map(r=>r.text||'').join(''):'';
  if (!/^(?:no videos|0 videos?)$/i.test(statText.normalize('NFKC').trim().replace(/\s+/g, ' '))) return fail(4);
  // Any discovered count conflict or membership/cursor rejects the exception.
  for (const value of [body.metadata?.playlistMetadataRenderer?.videoCount,body.metadata?.playlistMetadataRenderer?.trackCount,body.header?.playlistHeaderRenderer?.numVideosText,body.header?.playlistHeaderRenderer?.secondSubtitle]) {
    if(value===undefined)continue;
    if(Number.isSafeInteger(value)){if(value!==0)return fail(5);continue;}
    const countText=viewText(value).normalize('NFKC').trim().replace(/\s+/g, ' ');
    if(!/^(?:0(?: videos?)?|no videos)$/i.test(countText))return fail(5);
  }
  const oldHeader=body.header?.playlistHeaderRenderer;
  if(oldHeader && ((oldHeader.playlistId && ![id,`VL${id}`].includes(oldHeader.playlistId)) || (oldHeader.privacy && oldHeader.privacy!=='PRIVATE') || (oldHeader.ownerEndpoint?.browseEndpoint?.browseId && oldHeader.ownerEndpoint.browseEndpoint.browseId!==accountId)))return fail(6);
  const tabs=body.contents?.twoColumnBrowseResultsRenderer?.tabs;
  const selected=(Array.isArray(tabs)?tabs:[]).filter(x=>x?.tabRenderer?.selected===true);
  const sections=selected.length===1?selected[0].tabRenderer.content?.sectionListRenderer?.contents:null;
  const memberItems=Array.isArray(sections)&&sections.length===1?sections[0]?.itemSectionRenderer?.contents:null;
  if(!Array.isArray(memberItems) || !memberItems.every(x=>x?.messageRenderer && Object.keys(x).length===1))return fail(7);
  let continuation=false,membership=false,truncated=false,visited=0;
  function walk(value,depth=0,region='Other'){
    if(!value || typeof value!=='object')return;
    if(++visited>10000 || depth>40){truncated=true;if(evidence)evidence.emptyTraversalTruncated=1;return;}
    for(const [key,child] of Object.entries(value)){
      const childRegion=depth===0?(key==='header'?'Header':key==='sidebar'?'Sidebar':key==='contents'?'Contents':'Other'):region;
      if(/continuation/i.test(key)){
        continuation=true;
        if(evidence){const field=`emptyContinuation${childRegion}`;evidence[field]=Math.min(10000,evidence[field]+1);}
      }
      if(['playlistVideoRenderer','playlistPanelVideoRenderer','musicResponsiveListItemRenderer'].includes(key) || (key==='lockupViewModel' && child?.contentType==='LOCKUP_CONTENT_TYPE_VIDEO')){
        membership=true;if(evidence)evidence.emptyMembershipNodes=Math.min(10000,evidence.emptyMembershipNodes+1);
      }
      walk(child,depth+1,childRegion);
    }
  }
  // Membership is scoped to the uniquely selected playlist tab and response
  // action/continuation containers (YouTube.js Feed/Playlist, yt-dlp _entries).
  // Account/framework/header metadata is not a playlist membership source.
  walk(selected[0].tabRenderer.content,1,'Contents');
  if(body.continuationContents!==undefined){
    continuation=true;
    if(evidence)evidence.emptyContinuationOther=Math.min(10000,evidence.emptyContinuationOther+1);
    walk(body.continuationContents,1,'Other');
  }
  for(const key of ['onResponseReceivedActions','onResponseReceivedEndpoints','onResponseReceivedCommands']){
    walk(body[key],1,'Other');
  }
  if(truncated)return fail(8);
  if(continuation)return fail(9);
  if(membership)return fail(10);
  return true;
}


  async function run(operation, args = {}, session = {}, runtime = {}) {
    const fetchImpl = runtime.fetchImpl || fetch;
    const origin = session.origin;
    const host = origin === 'https://www.youtube.com' ? 'www.youtube.com' : origin === 'https://music.youtube.com' ? 'music.youtube.com' : null;
    if (!host) throw Object.assign(new Error('YouTube session origin is invalid.'), { code: 'UNSUPPORTED' });
    const cookie = String(session.cookieHeader || '');
    const sessionCookies = parseSessionCookies(cookie);
    if (!sessionCookies) throw Object.assign(new Error('YouTube session credential is invalid.'), { code: 'UNSUPPORTED' });
    const expectedAccountId = typeof session.accountId === 'string' && /^[A-Za-z0-9_@.-]{3,120}$/.test(session.accountId) ? session.accountId : null;
    if (!expectedAccountId && operation !== 'bootstrap') throw Object.assign(new Error('YouTube account identity must be confirmed before native access.'), { code: 'SIGN_IN_REQUIRED' });
    const initialClient = clientIdentity(session.clientName || 'WEB');
    if (!initialClient) throw Object.assign(new Error('YouTube client context is invalid.'), { code: 'UNSUPPORTED' });
    let clientName = initialClient.name;
    let clientHeaderName = initialClient.id;
    let clientVersion = String(session.clientVersion || '');
    let apiKey = String(session.apiKey || '');
    let visitorData = String(session.visitorData || '');
    let dataSyncId = String(session.dataSyncId || '');
    let authUser = String(session.authUser || '0');
    let hl = String(session.hl || 'en'), gl = String(session.gl || 'US');
    if ((operation !== 'bootstrap' && !/^[A-Za-z0-9._-]{1,64}$/.test(clientVersion)) || (apiKey && !/^[A-Za-z0-9_-]{10,160}$/.test(apiKey))) throw Object.assign(new Error('YouTube client context is invalid.'), { code: 'UNSUPPORTED' });
    if (!['bootstrap', 'status', 'search', 'listPlaylists', 'getPlaylist', 'applyPlaylistEdits', 'createVerificationPlaylist'].includes(operation)) throw Object.assign(new Error('This YouTube operation is not available.'), { code: 'UNSUPPORTED' });
    const now = runtime.now || Date.now;
    if (!/^\d{1,3}$/.test(authUser)) throw Object.assign(new Error('YouTube session context is invalid.'), { code: 'UNSUPPORTED' });
    if (Object.prototype.hasOwnProperty.call(session, 'dataSyncId') && typeof session.dataSyncId !== 'string') throw Object.assign(new Error('YouTube session context is invalid.'), { code: 'UNSUPPORTED' });
    if (dataSyncId && !/^[A-Za-z0-9_:-]{1,256}$/.test(dataSyncId)) throw Object.assign(new Error('YouTube session context is invalid.'), { code: 'UNSUPPORTED' });
    if (session.pageContextCaptured !== undefined && typeof session.pageContextCaptured !== 'boolean') throw Object.assign(new Error('YouTube session context is invalid.'), { code: 'UNSUPPORTED' });
    if (visitorData) {
      if (visitorData.length > 512 || /[\r\n\u0000-\u001f\u007f]/u.test(visitorData)) throw Object.assign(new Error('YouTube session context is invalid.'), { code: 'UNSUPPORTED' });
    }
    const authSources = [
      ['SAPISIDHASH', sessionCookies.SAPISID || sessionCookies['__Secure-3PAPISID']],
      ['SAPISID1PHASH', sessionCookies['__Secure-1PAPISID']],
      ['SAPISID3PHASH', sessionCookies['__Secure-3PAPISID']],
    ].filter(([, value]) => value);
    let query = apiKey ? `?key=${encodeURIComponent(apiKey)}&prettyPrint=false` : '?prettyPrint=false';
    const timeoutMs = Math.max(1, Math.min(20_000, Number(runtime.timeoutMs) || 20_000));
    let selectedBrandId = null;
    let bootstrapLoggedIn = 'unknown';
    const generalOperation = ['listPlaylists', 'getPlaylist', 'applyPlaylistEdits', 'createVerificationPlaylist', 'search'].includes(operation);
    const fetchBounded = async (path, method = 'POST', body, useDataSyncId = true, forceWeb = false) => {
      if (!/^\/(?:|youtubei\/v1\/(?:browse|browse\/edit_playlist|playlist\/create|search|navigation\/resolve_url|account\/(?:accounts_list|account_menu))|feed\/playlists)$/.test(path)) throw Object.assign(new Error('YouTube request path is not allowed.'), { code: 'UNSUPPORTED' });
      const accountList = path === '/youtubei/v1/account/accounts_list';
      const webResolve = path === '/youtubei/v1/navigation/resolve_url';
      // A bounded no-auth resolve_url comparison returned UC browse IDs with
      // WEB at both hosts, but WEB_REMIX returned a urlEndpoint on Music and
      // HTTP 404 on WWW. Use the stable WWW/WEB result for channel identity.
      const generalLibrary = forceWeb || (generalOperation && ['/feed/playlists', '/youtubei/v1/browse', '/youtubei/v1/browse/edit_playlist', '/youtubei/v1/search', '/youtubei/v1/playlist/create', '/youtubei/v1/account/account_menu'].includes(path));
      const requestOrigin = accountList || webResolve || generalLibrary ? 'https://www.youtube.com' : origin;
      const requestClient = accountList || webResolve || generalLibrary ? { name: 'WEB', id: '1', version: '2.20260708.00.00' } : { name: clientName, id: clientHeaderName, version: clientVersion || '0.0' };
      const controller = new AbortController(), timer = setTimeout(() => controller.abort(), timeoutMs);
      try {
        // The captured profile selector is reused unchanged; the WWW active
        // header below must independently resolve to the confirmed channel.
        // YouTube.js Session.on_behalf_of_user + HTTPClient X-Goog-PageId.
        const onBehalfOfUser = useDataSyncId && !webResolve ? (generalLibrary ? dataSyncId : (dataSyncId || selectedBrandId)) : null;
        const context = { client: { clientName: requestClient.name, clientVersion: requestClient.version, hl, gl, visitorData: visitorData || undefined }, user: { lockedSafetyMode: false, ...(onBehalfOfUser ? { onBehalfOfUser } : {}) } };
        const headers = { 'Content-Type': 'application/json', Cookie: cookie, Origin: requestOrigin, 'X-Origin': requestOrigin, Referer: `${requestOrigin}/`, 'X-YouTube-Client-Name': requestClient.id, 'X-YouTube-Client-Version': requestClient.version, 'X-Goog-AuthUser': authUser };
        if (onBehalfOfUser) headers['X-Goog-PageId'] = onBehalfOfUser;
        if (visitorData) headers['X-Goog-Visitor-Id'] = visitorData;
        if (bootstrapLoggedIn === 'true') headers['X-Youtube-Bootstrap-Logged-In'] = 'true';
        const ts = Math.floor(now() / 1000);
        const authorization = [];
        for (const [scheme, value] of authSources) {
          const digest = await crypto.subtle.digest('SHA-1', new TextEncoder().encode(`${ts} ${value} ${requestOrigin}`));
          const hash = [...new Uint8Array(digest)].map(byte => byte.toString(16).padStart(2, '0')).join('');
          authorization.push(`${scheme} ${ts}_${hash}`);
        }
        headers.Authorization = authorization.join(' ');
        const response = await fetchImpl(`${requestOrigin}${path}${path.startsWith('/youtubei/') ? (generalLibrary ? '?prettyPrint=false' : query) : ''}`, { method, headers: { ...headers, ...(method === 'GET' ? { Accept: 'text/html' } : {}) }, ...(body ? { body: JSON.stringify({ context, ...body }) } : {}), redirect: 'error', credentials: 'omit', signal: controller.signal, cache: 'no-store' });
        if (response.status === 401 || response.status === 403) throw Object.assign(new Error('YouTube session expired or was denied. Use the existing session again.'), { code: 'SIGN_IN_REQUIRED', phase: path.includes('youtubei') ? 'innertube' : 'playlist_library', httpStatus: response.status });
        if (response.status === 429) throw Object.assign(new Error('YouTube temporarily limited this request. Try again shortly.'), { code: 'RATE_LIMITED', phase: path.includes('youtubei') ? 'innertube' : 'playlist_library', httpStatus: 429, retryAfterSeconds: Number(response.headers.get('retry-after')) || null });
        if (!response.ok) throw Object.assign(new Error('YouTube returned an unsupported response.'), { code: 'PROVIDER_CHANGED', phase: 'innertube', httpStatus: response.status });
        const text = await response.text();
        if (text.length > 2 * 1024 * 1024) throw Object.assign(new Error('YouTube response exceeded the size limit.'), { code: 'PROVIDER_CHANGED' });
        if (method === 'GET') return text;
        try { return JSON.parse(text); } catch { throw Object.assign(new Error('YouTube response was not recognized.'), { code: 'PROVIDER_CHANGED' }); }
      } catch (error) {
        if (controller.signal.aborted) throw Object.assign(new Error('YouTube request timed out.'), { code: 'RATE_LIMITED' });
        throw error;
      } finally { clearTimeout(timer); }
    };
    const call = (body, endpoint = operation === 'search' ? 'search' : 'browse', useDataSyncId = true, forceWeb = false) => fetchBounded(`/youtubei/v1/${endpoint}`, 'POST', body, useDataSyncId, forceWeb);
    if (operation === 'bootstrap') {
      const html = await fetchBounded('/', 'GET');
      const config = parseYtcfg(html);
      if (!config) throw Object.assign(new Error('YouTube client context could not be read safely.'), { code: 'PROVIDER_CHANGED', phase: 'bootstrap_config' });
      const configuredClient = clientIdentity(config.INNERTUBE_CLIENT_NAME || 'WEB');
      if (!configuredClient) throw Object.assign(new Error('YouTube client context is incomplete.'), { code: 'PROVIDER_CHANGED', phase: 'bootstrap_config' });
      clientName = configuredClient.name;
      clientHeaderName = configuredClient.id;
      clientVersion = String(config.INNERTUBE_CLIENT_VERSION || '');
      apiKey = String(config.INNERTUBE_API_KEY || '');
      const configuredVisitorData = typeof config.VISITOR_DATA === 'string' ? config.VISITOR_DATA : '';
      if (configuredVisitorData.length > 512 || /[\r\n\u0000-\u001f\u007f]/u.test(configuredVisitorData)) throw Object.assign(new Error('YouTube client context is incomplete.'), { code: 'PROVIDER_CHANGED', phase: 'bootstrap_config' });
      visitorData = configuredVisitorData || visitorData;
      const rawDataSyncId = config.DATASYNC_ID;
      if (rawDataSyncId !== undefined && rawDataSyncId !== null && (typeof rawDataSyncId !== 'string' || rawDataSyncId.length > 512)) throw Object.assign(new Error('YouTube client context is incomplete.'), { code: 'PROVIDER_CHANGED', phase: 'bootstrap_config' });
      const configuredDataSyncId = typeof rawDataSyncId === 'string' ? rawDataSyncId.split('||', 1)[0] : '';
      if (typeof rawDataSyncId === 'string' && rawDataSyncId.includes('||') && !configuredDataSyncId) throw Object.assign(new Error('YouTube client context is incomplete.'), { code: 'PROVIDER_CHANGED', phase: 'bootstrap_config' });
      if (configuredDataSyncId && !/^[A-Za-z0-9_:-]{1,256}$/.test(configuredDataSyncId)) throw Object.assign(new Error('YouTube client context is incomplete.'), { code: 'PROVIDER_CHANGED', phase: 'bootstrap_config' });
      if (dataSyncId && configuredDataSyncId && dataSyncId !== configuredDataSyncId) throw Object.assign(new Error('YouTube account context changed during confirmation.'), { code: 'ACCOUNT_CHANGED', phase: 'account_identity' });
      dataSyncId = configuredDataSyncId || dataSyncId;
      const rawAuthUser = config.SESSION_INDEX;
      if (rawAuthUser !== undefined && rawAuthUser !== null && !(typeof rawAuthUser === 'string' || Number.isInteger(rawAuthUser))) throw Object.assign(new Error('YouTube client context is incomplete.'), { code: 'PROVIDER_CHANGED', phase: 'bootstrap_config' });
      const configuredAuthUser = rawAuthUser === undefined || rawAuthUser === null ? '' : String(rawAuthUser);
      if (configuredAuthUser && !/^\d{1,3}$/.test(configuredAuthUser)) throw Object.assign(new Error('YouTube client context is incomplete.'), { code: 'PROVIDER_CHANGED', phase: 'bootstrap_config' });
      if (session.pageContextCaptured === true && configuredAuthUser && configuredAuthUser !== authUser) throw Object.assign(new Error('YouTube account context changed during confirmation.'), { code: 'ACCOUNT_CHANGED', phase: 'account_identity' });
      authUser = configuredAuthUser || authUser || '0';
      hl = String(config.HL || 'en'); gl = String(config.GL || 'US');
      bootstrapLoggedIn = typeof config.LOGGED_IN === 'boolean' ? String(config.LOGGED_IN) : 'unknown';
      if (!/^[A-Za-z0-9._-]{1,64}$/.test(clientVersion) || (apiKey && !/^[A-Za-z0-9_-]{10,160}$/.test(apiKey)) || !/^\d{1,3}$/.test(authUser) || (session.pageContextCaptured === true && !visitorData) || visitorData.length > 512) throw Object.assign(new Error('YouTube client context is incomplete.'), { code: 'PROVIDER_CHANGED', phase: 'bootstrap_config' });
      query = apiKey ? `?key=${encodeURIComponent(apiKey)}&prettyPrint=false` : '?prettyPrint=false';
    }
    const accountMenu = await call({ requestType: 'ACCOUNTS_LIST_REQUEST_TYPE_CHANNEL_SWITCHER', callCircumstance: 'SWITCHING_USERS_FULL' }, 'account/accounts_list', false);
    const accountItems = [];
    const findItems = value => {
      if (!value || typeof value !== 'object') return;
      const contents = value.accountItemSectionRenderer?.contents;
      if (Array.isArray(contents)) for (const wrapper of contents) if (wrapper.accountItem) accountItems.push(wrapper.accountItem);
      for (const child of Object.values(value)) findItems(child);
    };
    findItems(accountMenu);
    let activeHeaderForIdentity = null;
    const schemaFlags = body => {
      let flags = (body?.responseContext ? 1 : 0) | (body?.actions?.length ? 2 : 0) | (body?.contents ? 4 : 0);
      const walk = value => { if (!value || typeof value !== 'object') return;
        if (value.accountItem) flags |= 8; if (value.signInEndpoint) flags |= 16;
        if (value.activeAccountHeaderRenderer) flags |= 32; if (value.accountItemRenderer) flags |= 64;
        if (value.accountItemViewModel) flags |= 128; if (value.loggedOut === true) flags |= 256;
        for (const child of Object.values(value)) walk(child);
      }; walk(body); return flags;
    };
    let menuShape = 0; let menuStructures = [];
    if (accountItems.length === 0) {
      // Music's active-account menu is independently documented by ytmusicapi.
      // Only its explicit active header is eligible; a display name alone never establishes identity.
      const menu = await call({}, 'account/account_menu', true);
      menuShape = schemaFlags(menu);
      const structures = new Set();
      const readStructure = value => { if (!value || typeof value !== 'object') return; for (const [key, child] of Object.entries(value)) { if (/^[a-zA-Z]{1,60}(?:Renderer|ViewModel|Action)$/.test(key) && structures.size < 12) structures.add(key); readStructure(child); } };
      readStructure(menu); menuStructures = [...structures];
      const activeHeaders = (Array.isArray(menu.actions) ? menu.actions : []).map(action =>
        action.openPopupAction?.popup?.multiPageMenuRenderer?.header?.activeAccountHeaderRenderer).filter(Boolean);
      if (activeHeaders.length === 1) activeHeaderForIdentity = activeHeaders[0];
      if (activeHeaders.length === 1) accountItems.push({
        isSelected: true,
        accountName: activeHeaders[0].accountName,
        channelHandle: activeHeaders[0].channelHandle,
      });
    }
    let selected = accountItems.filter(item => item.isSelected === true);
    if (selected.length !== 1) {
      let rendererCount = 0;
      const countRenderers = value => { if (!value || typeof value !== 'object') return; if (value.accountItemRenderer) rendererCount++; for (const child of Object.values(value)) countRenderers(child); };
      countRenderers(accountMenu);
      throw Object.assign(new Error('YouTube could not verify the active account.'), { code: 'SIGN_IN_REQUIRED', phase: 'account_selection', diagnostics: {
        structures: menuStructures, listShape: schemaFlags(accountMenu), menuShape, items: Math.min(100, accountItems.length), selected: Math.min(100, selected.length), renderers: Math.min(100, rendererCount),
        loggedOut: accountMenu.responseContext?.mainAppWebResponseContext?.loggedOut === true,
        bootstrapLoggedIn,
        providerError: Number.isInteger(accountMenu.error?.code) ? accountMenu.error.code : 0,
      } });
    }
    const item = selected[0];
    const accountName = text(item.accountName).trim();
    let handle = text(item.channelHandle).trim().replace(/^@/, '');
    const verifiedName = accountName && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(accountName) ? accountName : '';
    const pageId = item.serviceEndpoint?.selectActiveIdentityEndpoint?.supportedTokens?.map(token => token.pageIdToken?.pageId).find(value => typeof value === 'string' && /^\d{10,30}$/.test(value));
    selectedBrandId = pageId || null;
    // pageIdToken.pageId is not a YouTube channel UCID.
    let verifiedAccountId = null;
    const identityShape = { activeHeader: false, headerHandle: false, selectedHandle: Boolean(handle), pageIdToken: Boolean(pageId), directBrowseId: false, urlEndpoint: false, secondResolve: false };
    const resolvedHandles = new Set();
    const resolveHandle = async candidate => {
      const clean = String(candidate || '').trim().replace(/^@/, '');
      if (!validHandle(clean) || resolvedHandles.has(clean.toLowerCase())) return null;
      resolvedHandles.add(clean.toLowerCase());
      const resolved = await call({ url: `https://www.youtube.com/@${encodeURIComponent(clean)}` }, 'navigation/resolve_url', false);
      let id = channelIdFromResolution(resolved);
      identityShape.directBrowseId ||= Boolean(id);
      if (id) return id;
      const redirect = resolved.endpoint?.urlEndpoint?.url;
      identityShape.urlEndpoint ||= typeof redirect === 'string';
      const safeUrl = safeChannelResolutionUrl(redirect);
      if (!safeUrl) return null;
      identityShape.secondResolve = true;
      const second = await call({ url: safeUrl }, 'navigation/resolve_url', false);
      id = channelIdFromResolution(second);
      identityShape.directBrowseId ||= Boolean(id);
      return id;
    };
    if (handle) {
      verifiedAccountId = await resolveHandle(handle);
    }
    // accounts_list can have exactly one selected Google account but omit its
    // channel handle. Consult Music's explicit active header only in that case.
    if (!verifiedAccountId) {
      if (!activeHeaderForIdentity) {
        const menu = await call({}, 'account/account_menu', true);
        const activeHeaders = (Array.isArray(menu.actions) ? menu.actions : []).map(action =>
          action.openPopupAction?.popup?.multiPageMenuRenderer?.header?.activeAccountHeaderRenderer).filter(Boolean);
        if (activeHeaders.length === 1) activeHeaderForIdentity = activeHeaders[0];
      }
      identityShape.activeHeader = Boolean(activeHeaderForIdentity);
      if (activeHeaderForIdentity) {
        const headerHandle = text(activeHeaderForIdentity.channelHandle).trim().replace(/^@/, '');
        identityShape.headerHandle = Boolean(headerHandle);
        // When both endpoints expose a handle they must agree. Names are display-only.
        if (headerHandle && (!handle || headerHandle.toLowerCase() === handle.toLowerCase())) {
          handle = headerHandle;
          verifiedAccountId = await resolveHandle(headerHandle);
        }
      }
    }
    if (!verifiedAccountId) throw Object.assign(new Error('YouTube authenticated account is present, but no stable active channel identity could be verified.'), { code: 'PROVIDER_CHANGED', phase: 'channel_identity', diagnostics: { shape: identityShape } });
    const selectedHandle = handle ? `@${handle}` : null;
    if (operation !== 'bootstrap' && expectedAccountId && verifiedAccountId !== expectedAccountId && !(expectedAccountId.startsWith('@') && selectedHandle?.toLowerCase() === expectedAccountId.toLowerCase())) throw Object.assign(new Error('The active YouTube account changed. Confirm the account again before continuing.'), { code: 'ACCOUNT_CHANGED', phase: 'account_identity' });
    const accountId = verifiedAccountId;
    if (generalOperation) {
      // Music and WWW may have different active brand channels. General content
      // uses the captured profile selector, then verifies WWW's active channel.
      const webMenu = await call({}, 'account/account_menu', true, true);
      const headers = (Array.isArray(webMenu.actions) ? webMenu.actions : []).map(action =>
        action.openPopupAction?.popup?.multiPageMenuRenderer?.header?.activeAccountHeaderRenderer).filter(Boolean);
      const webHandle = headers.length === 1 ? text(headers[0].channelHandle).trim().replace(/^@/, '') : '';
      if (!validHandle(webHandle)) throw Object.assign(new Error('YouTube could not verify the active general channel.'), { code: 'SIGN_IN_REQUIRED', phase: 'account_identity' });
      const resolved = await call({ url: `https://www.youtube.com/@${encodeURIComponent(webHandle)}` }, 'navigation/resolve_url', false);
      const webAccountId = channelIdFromResolution(resolved);
      if (!webAccountId) throw Object.assign(new Error('YouTube could not resolve the active general channel.'), { code: 'PROVIDER_CHANGED', phase: 'account_identity' });
      if (webAccountId !== accountId) throw Object.assign(new Error('YouTube and Music have different active channels. Select the intended YouTube channel and confirm again.'), { code: 'ACCOUNT_CHANGED', phase: 'account_identity' });
    }
    if (operation === 'bootstrap') return { accountId, displayName: verifiedName || (handle ? `@${handle}` : 'YouTube account'), clientName, clientVersion, apiKey, visitorData, ...(dataSyncId ? { dataSyncId } : {}), authUser, hl, gl };
    if (operation === 'status') return { accountId, displayName: verifiedName || (handle ? `@${handle}` : 'YouTube account') };
    const verificationTitle=id=>id===undefined?'Burrata verification':typeof id==='string'&&/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id)?`Burrata verification ${id}`:null;
    if (operation === 'createVerificationPlaylist') {
      if (Object.keys(args).length !== (args.verificationIntentId===undefined?1:2) || !verificationTitle(args.verificationIntentId) || Object.keys(args).some(key=>!['accountId','verificationIntentId'].includes(key)) || args.accountId !== accountId) throw Object.assign(new Error('Confirm the same YouTube channel before creating the verification playlist.'), { code: 'ACCOUNT_CHANGED' });
      let created;
      try {
        runtime.onMutationDispatch?.();
        created = await call({ title: verificationTitle(args.verificationIntentId), privacyStatus: 'PRIVATE', videoIds: [] }, 'playlist/create');
        if (!/^PL[A-Za-z0-9_-]{10,78}$/.test(created?.playlistId || '')) throw Object.assign(new Error('unrecognized creation response'), { code: 'PROVIDER_CHANGED', phase: 'playlist_creation_response' });
      } catch (error) {
        throw Object.assign(new Error('Playlist creation may have succeeded. Do not retry creation; inspect the provider library.'), { code: 'OUTCOME_UNKNOWN', phase: error?.phase, httpStatus: error?.httpStatus });
      }
      // Creation response only supplies its ID. The caller must read this ID to verify privacy and emptiness.
      return { accountId, playlistId: created.playlistId, title: verificationTitle(args.verificationIntentId), status: 'CREATED_REQUIRES_READBACK', url: `https://www.youtube.com/playlist?list=${created.playlistId}` };
    }
    if (operation === 'applyPlaylistEdits') {
      if (['LM', 'SE', 'LL', 'WL'].includes(String(args.playlistId || ''))) throw Object.assign(new Error('YouTube system collections are read-only.'), { code: 'UNSUPPORTED' });
      if (args.previewApproved !== true) throw Object.assign(new Error('Playlist changes require an approved sync preview.'), { code: 'UNSUPPORTED' });
      const preview = args.syncPreview;
      if (!preview || Object.keys(preview).length !== 5
          || typeof preview.previewId !== 'string' || !/^[A-Za-z0-9-]{8,128}$/.test(preview.previewId)
          || preview.accountId !== accountId
          || typeof preview.baselineRevision !== 'string' || preview.baselineRevision.length > 256
          || typeof preview.remoteRevision !== 'string' || preview.remoteRevision.length > 256
          || typeof preview.remoteHash !== 'string' || !/^[a-f0-9]{64}$/i.test(preview.remoteHash)) {
        throw Object.assign(new Error('YouTube edits require the current stored sync preview for this account.'), { code: 'CONFLICT' });
      }
      const id = String(args.playlistId || '');
      if (!/^[A-Za-z0-9_-]{10,80}$/.test(id)) throw Object.assign(new Error('YouTube playlist ID is invalid.'), { code: 'UNSUPPORTED' });
      const expected = args.expectedEntries;
      const actions = args.actions;
      if (!Array.isArray(expected) || expected.length > 10_000 || !Array.isArray(actions) || !actions.length || actions.length > 500) throw Object.assign(new Error('The reviewed playlist snapshot is invalid.'), { code: 'UNSUPPORTED' });
      const validEntry = row => row && typeof row.entryId === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(row.entryId) && (row.trackId === null || (typeof row.trackId === 'string' && /^[A-Za-z0-9_-]{11}$/.test(row.trackId)));
      if (!expected.every(validEntry) || new Set(expected.map(row => row.entryId)).size !== expected.length) throw Object.assign(new Error('Playlist items do not have unique occurrence IDs; sync was stopped.'), { code: 'UNSUPPORTED' });
      let page = await call({ browseId: `VL${id}`, params: 'wgYCCAA=' });
      const initial = page;
      const live = playlistEntries(page, id);
      const seenCursors = new Set();
      let pageInfo = playlistPageContinuation(page, id);
      if ((!pageInfo.found && !modernExplicitEmpty(initial,id,accountId,hl,playlistExactTotal(initial,hl))) || pageInfo.ambiguous) throw Object.assign(new Error('YouTube playlist did not include a recognized item section.'), { code: 'PROVIDER_CHANGED' });
      let cursor = pageInfo.cursor;
      while (cursor && live.length <= 10_000) {
        if (seenCursors.has(cursor)) throw Object.assign(new Error('YouTube playlist pagination repeated; sync was stopped.'), { code: 'PROVIDER_CHANGED' });
        seenCursors.add(cursor);
        page = await call({ continuation: cursor });
        live.push(...playlistEntries(page, id));
        pageInfo = playlistPageContinuation(page, id);
        if (!pageInfo.found || pageInfo.ambiguous) throw Object.assign(new Error('YouTube playlist continuation did not include a recognized item section.'), { code: 'PROVIDER_CHANGED' });
        cursor = pageInfo.cursor;
      }
      const total = playlistExactTotal(initial, hl);
      if (cursor || total === null || live.length !== total || live.length > 10_000 || live.some(row => !validEntry(row)) || new Set(live.map(row => row.entryId)).size !== live.length) throw Object.assign(new Error('YouTube playlist could not be proven complete and safe to edit.'), { code: 'PROVIDER_CHANGED' });
      const sameSnapshot = live.length === expected.length && live.every((row, index) => row.entryId === expected[index].entryId && row.trackId === expected[index].trackId);
      if (!sameSnapshot) throw Object.assign(new Error('The YouTube playlist changed after preview. Review a new sync preview.'), { code: 'CONFLICT' });
      const capabilities = youtubeWriteCapabilities(initial,id,accountId,hl);
      if (!actions.every(action=>action?.type==='add'?capabilities.add:action?.type==='remove'?capabilities.remove:action?.type==='moveBefore'?capabilities.reorder:false)) throw Object.assign(new Error('This YouTube playlist is not verified as editable and owned by the active account.'), { code: 'UNSUPPORTED' });
      const byEntry = new Map(live.map(row => [row.entryId, row]));
      const knownEntryIds = new Set(live.map(row => row.entryId));
      const changedOccurrences = new Set();
      const requestActions = actions.map(action => {
        if (!action || typeof action !== 'object') throw Object.assign(new Error('A reviewed playlist action is invalid.'), { code: 'UNSUPPORTED' });
        const allowedKeys = action.type === 'add' ? ['type', 'trackId'] : action.type === 'remove' ? ['type', 'entryId', 'trackId'] : action.type === 'moveBefore' ? ['type', 'entryId', 'trackId', 'successorEntryId'] : [];
        if (!allowedKeys.length || Object.keys(action).some(key => !allowedKeys.includes(key))) throw Object.assign(new Error('A reviewed playlist action contains unsupported fields.'), { code: 'UNSUPPORTED' });
        if (action.type === 'add' && typeof action.trackId === 'string' && /^[A-Za-z0-9_-]{11}$/.test(action.trackId)) return { action: 'ACTION_ADD_VIDEO', addedVideoId: action.trackId };
        if (action.type === 'remove' && typeof action.entryId === 'string' && typeof action.trackId === 'string' && byEntry.get(action.entryId)?.trackId === action.trackId) {
          if (changedOccurrences.has(action.entryId)) throw Object.assign(new Error('A playlist occurrence appears in more than one edit.'), { code: 'UNSUPPORTED' });
          changedOccurrences.add(action.entryId);
          return { action: 'ACTION_REMOVE_VIDEO', setVideoId: action.entryId, removedVideoId: action.trackId };
        }
        if (action.type === 'moveBefore' && typeof action.entryId === 'string' && typeof action.trackId === 'string' && byEntry.get(action.entryId)?.trackId === action.trackId) {
          if (changedOccurrences.has(action.entryId)) throw Object.assign(new Error('A playlist occurrence appears in more than one edit.'), { code: 'UNSUPPORTED' });
          changedOccurrences.add(action.entryId);
          const target = { action: 'ACTION_MOVE_VIDEO_BEFORE', setVideoId: action.entryId };
          if (action.successorEntryId !== null) {
            if (typeof action.successorEntryId !== 'string' || !byEntry.has(action.successorEntryId) || action.successorEntryId === action.entryId) throw Object.assign(new Error('A reviewed playlist move target is invalid.'), { code: 'UNSUPPORTED' });
            target.movedSetVideoIdSuccessor = action.successorEntryId;
          }
          return target;
        }
        throw Object.assign(new Error('A reviewed playlist action no longer matches this playlist.'), { code: 'CONFLICT' });
      });
      let edited;
      try {
        edited = await call({ playlistId: id, actions: requestActions }, 'browse/edit_playlist');
      } catch {
        throw Object.assign(new Error('YouTube may have received the playlist change. Refresh it before trying again.'), { code: 'OUTCOME_UNKNOWN' });
      }
      if (edited?.status !== 'STATUS_SUCCEEDED') throw Object.assign(new Error('YouTube may have received some playlist changes. Refresh the playlist before trying again.'), { code: 'OUTCOME_UNKNOWN' });
      const addedCount = actions.filter(action => action.type === 'add').length;
      const addedResults = edited.playlistEditResults;
      if (addedCount) {
        const requestedAdds = actions.filter(action => action.type === 'add').map(action => action.trackId);
        const returnedIds = Array.isArray(addedResults) ? addedResults.map(result => result?.playlistEditVideoAddedResultData?.setVideoId) : [];
        if (!Array.isArray(addedResults) || addedResults.length !== addedCount || addedResults.some((result, index) => result?.playlistEditVideoAddedResultData?.videoId !== requestedAdds[index] || typeof result?.playlistEditVideoAddedResultData?.setVideoId !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(result.playlistEditVideoAddedResultData.setVideoId)) || new Set(returnedIds).size !== addedCount || returnedIds.some(value => knownEntryIds.has(value))) throw Object.assign(new Error('YouTube may have received the playlist change. Refresh the playlist before trying again.'), { code: 'OUTCOME_UNKNOWN' });
      }
      return { accountId, playlistId: id, applied: actions.length, added: addedCount, status: 'STATUS_SUCCEEDED' };
    }
    if (operation === 'search') {
      const queryText = String(args.query || '').trim();
      if (!queryText || queryText.length > 500) throw Object.assign(new Error('YouTube search text is invalid.'), { code: 'UNSUPPORTED' });
      const kind = ['track', 'playlist', 'channel'].includes(args.kind) ? args.kind : 'track';
      let continuation;
      if (args.cursor) {
        let cursor;
        try { cursor = JSON.parse(decodeURIComponent(String(args.cursor).replace(/^yt-search-v1:/, ''))); } catch { /* rejected below */ }
        if (!String(args.cursor).startsWith('yt-search-v1:') || !cursor || Object.keys(cursor).length !== 5 || cursor.accountId !== accountId || cursor.query !== queryText || cursor.kind !== kind || cursor.origin !== 'https://www.youtube.com' || typeof cursor.token !== 'string' || !cursor.token || cursor.token.length > 16_384) throw Object.assign(new Error('YouTube search cursor does not match this account and query.'), { code: 'UNSUPPORTED' });
        continuation = cursor.token;
      }
      const body = continuation ? await call({ continuation }) : await call({ query: queryText, params: kind === 'playlist' ? 'EgIQAw%3D%3D' : kind === 'channel' ? 'EgIQAg%3D%3D' : undefined });
      const items = decode(body).filter(row => row.kind === kind).slice(0, Math.min(50, Math.max(1, Number(args.limit) || 20)));
      const token = nextToken(body);
      const nextCursor = token ? `yt-search-v1:${encodeURIComponent(JSON.stringify({ accountId, query: queryText, kind, origin: 'https://www.youtube.com', token }))}` : null;
      return { accountId, items, nextCursor };
    }
    if (operation === 'listPlaylists') {
      let body, previousLibraryToken = null;
      const cursorScope = String(args.ownerId || '');
      if (args.cursor) {
        let cursor;
        try { cursor = JSON.parse(decodeURIComponent(String(args.cursor).replace(/^yt-library-v1:/, ''))); } catch { /* rejected below */ }
        if (!String(args.cursor).startsWith('yt-library-v1:') || !cursor || Object.keys(cursor).length !== 4
            || cursor.accountId !== accountId || cursor.ownerId !== cursorScope || cursor.origin !== 'https://www.youtube.com'
            || typeof cursor.token !== 'string' || !cursor.token || cursor.token.length > 16_384) {
          throw Object.assign(new Error('YouTube library cursor does not match this account and source.'), { code: 'UNSUPPORTED' });
        }
        previousLibraryToken = cursor.token;
        body = await call({ continuation: cursor.token });
      }
      else if (args.ownerId) {
        let owner = String(args.ownerId);
        if (/^https?:\/\//i.test(owner)) {
          let parsed; try { parsed = new URL(owner); } catch { throw Object.assign(new Error('YouTube channel URL is invalid.'), { code: 'UNSUPPORTED' }); }
          if (!['www.youtube.com', 'music.youtube.com'].includes(parsed.hostname) || parsed.protocol !== 'https:') throw Object.assign(new Error('Only YouTube channel URLs are supported.'), { code: 'UNSUPPORTED' });
          const path = parsed.pathname.replace(/\/+$/, '').replace(/\/playlists$/, ''), handle = path.match(/^\/@([A-Za-z0-9._-]+)$/), channel = path.match(/^\/channel\/(UC[A-Za-z0-9_-]+)$/);
          owner = handle ? `@${handle[1]}` : channel?.[1] || '';
        }
        if (owner.startsWith('@')) { const resolved = await call({ url: `https://www.youtube.com/${owner}` }, 'navigation/resolve_url'); owner = resolved.endpoint?.browseEndpoint?.browseId || resolved.browseEndpoint?.browseId || ''; }
        if (!/^(?:UC[A-Za-z0-9_-]+|[A-Za-z0-9_-]{20,})$/.test(owner)) throw Object.assign(new Error('YouTube channel could not be resolved.'), { code: 'UNSUPPORTED' });
        body = await call({ browseId: owner.startsWith('UC') ? owner : `UC${owner}` });
      } else body = await call({ browseId: 'FEplaylist_aggregation' });
      // General YouTube libraries include non-music playlists. Even a Music
      // handoff uses WWW/WEB here; identity is verified above for every page.
      // Source: YouTube.js Innertube.getPlaylists (FEplaylist_aggregation).
      if (!body || typeof body !== 'object') throw Object.assign(new Error('YouTube playlist page was not recognized.'), { code: 'PROVIDER_CHANGED', phase: 'playlist_library' });
      const listRoot = !args.ownerId && !args.cursor ? body.contents?.twoColumnBrowseResultsRenderer?.tabs?.find(tab => tab.tabRenderer?.selected)?.tabRenderer?.content || body.contents || body : body;
      const items = decode(listRoot).filter(row => row.kind === 'playlist').map(row => ({ id: row.id, provider: 'youtube', title: row.title || 'Untitled playlist', url: row.url, ownerId: args.ownerId || null, ownerName: row.subtitle || null, artworkUrl: row.artworkUrl, total: row.data.total, canEdit: false }));
      const ownLibrary = !args.ownerId, libraryInfo = ownLibrary ? libraryContinuation(listRoot, previousLibraryToken, args.inspectLibraryResponseShape === true) : null;
      const token = ownLibrary ? libraryInfo.token : nextToken(listRoot);
      const nextCursor = token ? `yt-library-v1:${encodeURIComponent(JSON.stringify({ accountId, ownerId: cursorScope, origin: 'https://www.youtube.com', token }))}` : null;
      return { accountId, items, nextCursor, ...(ownLibrary ? { complete: null, ...(libraryInfo.shape.ambiguous ? { complete: false, truncated: true } : {}), ...(args.inspectLibraryResponseShape === true ? { libraryResponseShape: libraryInfo.shape } : {}) } : {}) };
    }
    const id = String(args.playlistId || '');
    const systemCollection = ['LM', 'SE', 'LL', 'WL'].includes(id);
    if (!systemCollection && !/^[A-Za-z0-9_-]{10,80}$/.test(id)) throw Object.assign(new Error('YouTube playlist ID is invalid.'), { code: 'UNSUPPORTED' });
    let continuation, expectedTotal = null, offset = 0, legacyPaging = false;
    if (args.cursor) {
      let cursor;
      const raw = String(args.cursor), v2 = raw.startsWith('yt-playlist-v2:');
      try { cursor = JSON.parse(decodeURIComponent(raw.replace(/^yt-playlist-v[12]:/, ''))); } catch { /* rejected below */ }
      if ((!v2 && !raw.startsWith('yt-playlist-v1:')) || !cursor || Object.keys(cursor).length !== (v2 ? 6 : 4) || cursor.accountId !== accountId || cursor.playlistId !== id || cursor.origin !== 'https://www.youtube.com' || typeof cursor.token !== 'string' || !cursor.token || cursor.token.length > 16_384 || (v2 && (!Number.isSafeInteger(cursor.offset) || cursor.offset < 0 || (cursor.expectedTotal !== null && (!Number.isSafeInteger(cursor.expectedTotal) || cursor.expectedTotal < 0 || cursor.offset > cursor.expectedTotal))))) throw Object.assign(new Error('YouTube playlist cursor does not match this account and playlist.'), { code: 'UNSUPPORTED' });
      continuation = cursor.token;
      legacyPaging = !v2;
      if (v2) { expectedTotal = cursor.expectedTotal; offset = cursor.offset; }
    }
    const body = continuation ? await call({ continuation }) : await call({ browseId: `VL${id}`, params: 'wgYCCAA=' });
    const header = body.header || {}, metadata = header.musicDetailHeaderRenderer || header.playlistHeaderRenderer || {}, pageHeader = header.pageHeaderRenderer || body.pageHeaderRenderer || {}, pageView = body.contents?.pageHeaderViewModel || header.pageHeaderViewModel || header.pageHeaderRenderer?.content?.pageHeaderViewModel || body.pageHeaderRenderer?.content?.pageHeaderViewModel || {}, playlistMetadata = body.metadata?.playlistMetadataRenderer || body.playlistMetadataRenderer || {};
    const title = first(viewText(metadata.title), viewText(pageView.title), viewText(pageHeader.pageTitle), viewText(playlistMetadata.title));
    const subtitle = first(viewText(metadata.strapline), viewText(pageHeader.subtitle), viewText(pageView.metadata), viewText(playlistMetadata.description)) || '';
    const total = continuation ? expectedTotal : playlistExactTotal(body, hl);
    // Accept privacy only from a header explicitly bound to this playlist ID.
    let verifiedPrivacy = null, verifiedOwnerId = null;
    const inspectPrivacy = value => {
      if (!value || typeof value !== 'object') return;
      const header = value.playlistHeaderRenderer || value.musicPlaylistEditHeaderRenderer;
      if (header?.playlistId === id && /^UC[A-Za-z0-9_-]{20,30}$/.test(header.ownerEndpoint?.browseEndpoint?.browseId || '')) verifiedOwnerId = header.ownerEndpoint.browseEndpoint.browseId;
      if (header && (header.playlistId === id || header.playlistId === `VL${id}`) && ['PRIVATE', 'PUBLIC', 'UNLISTED'].includes(header.privacy)) verifiedPrivacy = header.privacy.toLowerCase();
      for (const nested of Object.values(value)) inspectPrivacy(nested);
    };
    inspectPrivacy(body);
    const sidebarIdentity = !continuation ? modernSidebarIdentity(body, id, accountId) : null;
    if (sidebarIdentity && (verifiedPrivacy === null || verifiedPrivacy === sidebarIdentity.visibility)
        && (verifiedOwnerId === null || verifiedOwnerId === sidebarIdentity.ownerId)) {
      verifiedPrivacy = sidebarIdentity.visibility;
      verifiedOwnerId = sidebarIdentity.ownerId;
    }
    const entries = playlistEntries(body, id), pageInfo = playlistPageContinuation(body, id);
    const cumulative = offset + entries.length;
    if (!Number.isSafeInteger(cumulative)) throw Object.assign(new Error('YouTube playlist page count exceeded the safe limit.'), { code: 'PROVIDER_CHANGED' });
    if (pageInfo.cursor && total !== null && cumulative > total) throw Object.assign(new Error('YouTube playlist page count changed during browsing.'), { code: 'PROVIDER_CHANGED' });
    const nextCursor = pageInfo.cursor ? `${legacyPaging ? 'yt-playlist-v1:' : 'yt-playlist-v2:'}${encodeURIComponent(JSON.stringify({ accountId, playlistId: id, origin: 'https://www.youtube.com', token: pageInfo.cursor, ...(!legacyPaging ? { expectedTotal: total, offset: cumulative } : {}) }))}` : null;
    return { accountId, ...(args.inspectResponseShape === true ? { responseShape: playlistResponseShape(body, id, accountId, hl, Boolean(continuation), pageInfo) } : {}), playlist: { id, provider: 'youtube', title: title || (id === 'LM' ? 'Liked Music' : id === 'SE' ? 'Saved Episodes' : ''), url: `https://www.youtube.com/playlist?list=${id}`, visibility: verifiedPrivacy, isPublic: verifiedPrivacy === null ? null : verifiedPrivacy === 'public', ownerId: verifiedOwnerId, ownerName: subtitle || null, artworkUrl: image(metadata.thumbnail || pageView.image || pageView.contentImage || pageView.heroImage || playlistMetadata.thumbnail), total, canEdit: !systemCollection && Boolean(editablePlaylistHeader(body, id, accountId)), writeCapabilities: youtubeWriteCapabilities(body,id,accountId,hl) }, entries, nextCursor, total, revision: null, complete: !legacyPaging && !pageInfo.ambiguous && !nextCursor && total !== null && cumulative === total && (pageInfo.found || (!continuation && modernExplicitEmpty(body, id, accountId, hl, total))) };
  }

  function editablePlaylistHeader(root, playlistId, accountId) {
    const rows = [];
    const visit = value => {
      if (!value || typeof value !== 'object') return;
      for (const key of ['musicEditablePlaylistDetailHeaderRenderer', 'musicPlaylistEditHeaderRenderer']) if (value[key]) rows.push(value[key]);
      const webHeader = value.playlistHeaderRenderer;
      if (webHeader?.isEditable === true && webHeader.playlistId === playlistId && webHeader.ownerEndpoint?.browseEndpoint?.browseId === accountId) rows.push({ playlistId, ownerId: accountId });
      for (const nested of Object.values(value)) visit(nested);
    };
    visit(root);
    return rows.find(header => {
      const id = header.playlistId;
      const ownerId = header.ownerId || header.ownerChannelId || header.author?.channelId;
      return (id === playlistId || id === `VL${playlistId}`) && ownerId === accountId;
    }) || null;
  }

  globalThis.BurrataYouTubeSessionProvider = { run };
})();
