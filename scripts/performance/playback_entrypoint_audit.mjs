import { lstat, readFile, realpath, writeFile, unlink } from 'node:fs/promises';
import { basename, dirname, join, resolve } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const execFileAsync = promisify(execFile);
const ENTRYPOINTS = new Set(['library_row', 'search_result', 'spotify_artist_track', 'spotify_album_track']);
const ROUTES = new Set(['library', 'favorites', 'history', 'downloads', 'settings', 'radio']);
const SEARCH_SOURCES = new Set(['all', 'library', 'musicbrainz', 'discogs', 'youtube', 'soundcloud', 'bandcamp', 'soulseek', 'monochrome', 'spotify']);
const SEARCH_KINDS = new Set(['all', 'tracks', 'artists', 'albums', 'videos', 'playlists', 'profiles', 'channels']);
const SAFE_ID = /^[A-Za-z0-9._:-]{1,100}$/;
const VERIFIED_PRIVATE_TARGET = Symbol('verified-private-target');

export function validateEntryPointSpec(spec) {
  if (!spec || typeof spec !== 'object' || !ENTRYPOINTS.has(spec.entrypoint)) throw new Error('invalid_entrypoint');
  if (spec.route && !ROUTES.has(spec.route)) throw new Error('invalid_route');
  if (!SAFE_ID.test(spec.trackId ?? '') || !SAFE_ID.test(spec.expectedProvider ?? '')
      || !SAFE_ID.test(spec.expectedRecordingId ?? '')) throw new Error('invalid_expected_identity');
  if (!Number.isFinite(spec.expectedDurationSeconds) || spec.expectedDurationSeconds <= 1) throw new Error('invalid_expected_duration');
  if (spec.entrypoint === 'search_result' && (!spec.query || typeof spec.query !== 'string' || spec.query.length > 500)) {
    throw new Error('invalid_search_query');
  }
  if (spec.entrypoint !== 'search_result' && spec.route !== 'library' && spec.entrypoint === 'library_row') {
    throw new Error('library_row_requires_library_route');
  }
  return spec;
}

export function validateUiAction(action) {
  if (!action || typeof action !== 'object' || !Number.isSafeInteger(action.id) || action.id < 1
      || action.kind !== 'click' || !Number.isSafeInteger(action.x) || action.x < 0 || action.x >= 1280
      || !Number.isSafeInteger(action.y) || action.y < 0 || action.y >= 900
      || (action.button ?? 1) !== 1) throw new Error('invalid_ui_action');
  return { id: action.id, kind: 'click', x: action.x, y: action.y, button: 1 };
}

function toolResult(response, operation) {
  if (response?.error || response?.isError || response?.result?.ok === false) throw new Error(`${operation}_failed`);
  return response?.result ?? response?.structuredContent?.result ?? response?.structuredContent ?? response;
}

export async function playVisibleSearchOccurrence(call, expected) {
  if (!expected || expected.entrypoint !== 'search_result' || typeof expected.query !== 'string'
      || !SEARCH_SOURCES.has(expected.searchSourceId) || !SEARCH_KINDS.has(expected.kind)
      || typeof expected.title !== 'string' || typeof expected.artist !== 'string') throw new Error('invalid_search_occurrence_spec');
  const search = toolResult(await call('search_ui', {
    query: expected.query, sourceId: expected.searchSourceId, kind: expected.kind,
  }), 'search_ui');
  if (search?.status !== 'complete' && search?.status !== 'ready') throw new Error('search_ui_not_complete');
  for (let attempt = 0; attempt < 3; attempt++) {
    const page = toolResult(await call('inspect_search_ui', { offset: 0, limit: 100 }), 'inspect_search_ui');
    if (page?.status !== 'ready' || !/^[0-9a-f-]{36}$/i.test(page.revision ?? '') || !Array.isArray(page.rows)) {
      throw new Error('search_ui_rows_unavailable');
    }
    const rows = page.rows.filter((row) => row.sourceId === expected.searchSourceId
      && row.title === expected.title && row.artist === expected.artist
      && Number.isFinite(row.durationSeconds)
      && Math.abs(row.durationSeconds - expected.expectedDurationSeconds) <= 1);
    if (rows.length !== 1) throw new Error(rows.length ? 'search_occurrence_ambiguous' : 'search_occurrence_missing');
    const row = rows[0];
    const result = toolResult(await call('play_search_result', {
      revision: page.revision, occurrenceToken: row.occurrenceToken,
    }), 'play_search_result');
    // These explicit refusals did not dispatch playback. Reinspect the changed
    // row set; a timeout or uncertain response must never trigger a second click.
    if (result?.status === 'stale' || result?.status === 'expired') continue;
    if (result?.status !== 'dispatched') throw new Error('search_occurrence_not_dispatched');
    if (result.revision !== page.revision || result.sourceId !== row.sourceId) {
      throw new Error('search_occurrence_dispatch_mismatch');
    }
    return { entrypoint: 'search_result', searchMode: page.mode, searchSourceId: expected.searchSourceId,
      query: expected.query, visibleTitle: row.title, visibleArtist: row.artist,
      visibleDurationSeconds: row.durationSeconds, action: 'renderer_occurrence_token',
      selected: true, dispatchStatus: result.status, acousticContentVerified: false };
  }
  throw new Error('search_occurrence_not_dispatched');
}

export async function verifyPrivateX11Target({ profile, appPid, xvfbPid, display }) {
  const canonicalProfile = await realpath(resolve(profile));
  if (dirname(canonicalProfile) !== '/tmp' || !/^burrata-launch\.[A-Za-z0-9_-]+$/.test(basename(canonicalProfile))) {
    throw new Error('isolated_profile_required');
  }
  const marker = join(canonicalProfile, '.burrata-isolated-profile');
  const markerInfo = await lstat(marker);
  if (markerInfo.isSymbolicLink() || !markerInfo.isFile()
      || await readFile(marker, 'utf8') !== 'burrata isolated memory run\n') throw new Error('isolated_profile_required');
  if (!/^:(?:9\d|10\d|11\d)$/.test(display ?? '') || !/^\d+$/.test(String(appPid)) || !/^\d+$/.test(String(xvfbPid))) {
    throw new Error('private_display_required');
  }
  const [appExe, appEnvironment, xvfbCommand] = await Promise.all([
    realpath(`/proc/${appPid}/exe`), readFile(`/proc/${appPid}/environ`), readFile(`/proc/${xvfbPid}/cmdline`),
  ]);
  const entries = new Map(appEnvironment.toString().split('\0').filter(Boolean).map((item) => {
    const split = item.indexOf('='); return [item.slice(0, split), item.slice(split + 1)];
  }));
  const xvfbArgs = xvfbCommand.toString().split('\0').filter(Boolean);
  if (!appExe.endsWith('/usr/bin/burrata') || entries.get('XDG_DATA_HOME') !== join(canonicalProfile, 'data')
      || entries.get('DISPLAY') !== display || !xvfbArgs.some((arg) => arg === display)
      || !xvfbArgs.includes('-nolisten') || !xvfbArgs.includes('tcp')) throw new Error('private_target_mismatch');
  const verified = { profile: canonicalProfile, appPid: Number(appPid), xvfbPid: Number(xvfbPid), display, appExe };
  Object.defineProperty(verified, VERIFIED_PRIVATE_TARGET, { value: true });
  return verified;
}

export async function waitForReviewedPrivateUiClick(target, { screenshot, stepFile, actionFile,
  xdotool = 'xdotool', sleep = (ms) => new Promise((resolveSleep) => setTimeout(resolveSleep, ms)),
  now = Date.now, timeoutMs = 90_000 }) {
  if (!target?.[VERIFIED_PRIVATE_TARGET]) throw new Error('private_target_required');
  const { stdout } = await execFileAsync(xdotool, ['search', '--onlyvisible', '--pid', String(target.appPid)], {
    timeout: 3000, env: { ...process.env, DISPLAY: target.display },
  });
  const windows = stdout.trim().split(/\s+/).filter((value) => /^\d+$/.test(value));
  if (windows.length !== 1) throw new Error('private_app_window_not_unique');
  const before = await screenshot('before-click');
  await writeFile(actionFile, '');
  await writeFile(stepFile, JSON.stringify({ phase: 'review-before-click', window: windows[0],
    display: target.display, screenshot: before, actionFile }));
  const deadline = now() + timeoutMs;
  let action = null;
  while (now() < deadline) {
    try {
      const raw = await readFile(actionFile, 'utf8');
      if (raw.trim()) action = validateUiAction(JSON.parse(raw));
    } catch (error) {
      if (error?.code !== 'ENOENT' && !(error instanceof SyntaxError)) throw error;
    }
    if (action) break;
    await sleep(100);
  }
  if (!action) throw new Error('reviewed_ui_action_timeout');
  await unlink(actionFile);
  await execFileAsync(xdotool, ['mousemove', '--window', windows[0], String(action.x), String(action.y), 'click', '1'], {
    timeout: 3000, env: { ...process.env, DISPLAY: target.display },
  });
  const after = await screenshot(`after-click-${action.id}`);
  await writeFile(stepFile, JSON.stringify({ phase: 'clicked', actionId: action.id,
    window: windows[0], display: target.display, before, screenshot: after }));
  return { window: windows[0], actionId: action.id, before, after };
}

export async function navigateAndCaptureRoute(call, route, { screenshot, stepFile }) {
  if (!ROUTES.has(route)) throw new Error('invalid_route');
  const response = toolResult(await call('navigate_ui', { view: route }), 'navigate_ui');
  if (response?.status !== 'complete' || response?.targetKind !== 'view' || response?.destination !== route) {
    throw new Error('route_transition_unverified');
  }
  const image = await screenshot(`route-${route}`);
  await writeFile(stepFile, JSON.stringify({ phase: 'route-ready', route, completionScope: response.completionScope ?? null, screenshot: image }));
  return { route, completionScope: response.completionScope ?? null, screenshot: image,
    settingsObserved: route === 'settings', settingsChanged: false };
}

export function assessEntryPointPlayback(before, after, expected) {
  if (!after || after.engine !== 'native' || after.trackId !== expected.trackId
      || !Number.isSafeInteger(after.generation) || after.generation === before?.generation) return { status: 'failed', reason: 'track_or_generation_mismatch' };
  if (after.candidate?.provider !== expected.expectedProvider) return { status: 'failed', reason: 'wrong_provider' };
  if (after.candidate?.id !== expected.expectedRecordingId) return { status: 'failed', reason: 'wrong_source_recording' };
  if (after.status !== 'playing' || !Number.isFinite(after.diagnostics?.firstAudioMs)
      || !Number.isFinite(after.position) || after.position < 3) return { status: 'failed', reason: 'startup_not_observed' };
  if (!Number.isFinite(after.duration) || Math.abs(after.duration - expected.expectedDurationSeconds) > 1) {
    return { status: 'failed', reason: 'recording_duration_mismatch' };
  }
  return { status: 'ready', entrypoint: expected.entrypoint, trackId: expected.trackId,
    provider: after.candidate.provider, recordingId: after.candidate.id, generation: after.generation,
    firstAudioMs: after.diagnostics.firstAudioMs, durationSeconds: after.duration,
    acousticContentVerified: false };
}
