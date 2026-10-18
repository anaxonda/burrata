import { lstat, mkdir, readFile, realpath, writeFile } from 'node:fs/promises';
import { basename, dirname, join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';

const [automationFile, trackId, outputFile] = process.argv.slice(2);
if (!automationFile || !trackId || !outputFile) {
  console.error('usage: local_playback_qualification.mjs AUTOMATION_FILE TRACK_ID OUTPUT_FILE');
  process.exit(2);
}

const projectRoot = new URL('../../', import.meta.url).pathname;
const client = new Client({ name: 'burrata-local-playback-qualification', version: '1.0.0' });
const transport = new StdioClientTransport({
  command: 'bash',
  args: [`${projectRoot}scripts/dev/mcp-stdio.sh`],
  cwd: projectRoot,
  env: { ...process.env, BURRATA_AUTOMATION_FILE: automationFile },
  stderr: 'pipe',
});
transport.stderr?.on('data', () => {});

const DEADLINE_MS = 20_000;
const POLL_MS = 100;
const START_ATTEMPTS = 20;
const SEEK_REPEATS_PER_FRACTION = 20;
const SEEK_FRACTIONS = [0.01, 0.5, 0.95];
const identity = (state) => state?.trackId ?? state?.track?.id ?? null;
const finite = (value) => typeof value === 'number' && Number.isFinite(value);
const sleep = (ms) => new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
const safeId = (value) => typeof value === 'string' && /^[a-zA-Z0-9._:-]{1,100}$/.test(value) ? value : null;
const metric = (value) => finite(value) && value >= 0 ? value : null;

async function requireIsolatedProfile(discoveryFile) {
  let canonicalFile;
  try {
    canonicalFile = await realpath(resolve(discoveryFile));
  } catch {
    throw new Error('isolated_profile_required');
  }
  const profileRoot = resolve(dirname(canonicalFile), '../..');
  const profileName = basename(profileRoot);
  if (dirname(profileRoot) !== '/tmp' || !/^burrata-launch\.[A-Za-z0-9_-]+$/.test(profileName)
      || canonicalFile !== join(profileRoot, 'data', 'org.burrata.discovery', 'automation.json')) {
    throw new Error('isolated_profile_required');
  }
  const marker = join(profileRoot, '.burrata-isolated-profile');
  try {
    const markerInfo = await lstat(marker);
    const markerContents = await readFile(marker, 'utf8');
    if (markerInfo.isSymbolicLink() || !markerInfo.isFile() || markerContents !== 'burrata isolated memory run\n') {
      throw new Error('isolated_profile_required');
    }
  } catch {
    throw new Error('isolated_profile_required');
  }
}

async function call(name, args = {}, deadline = Date.now() + DEADLINE_MS) {
  const timeout = Math.max(1, deadline - Date.now());
  let response;
  try {
    response = await client.callTool({ name, arguments: args }, { timeout });
  } catch (error) {
    const failure = new Error('MCP operation failed');
    failure.category = error?.name === 'TimeoutError' || error?.code === 'REQUEST_TIMEOUT' ? 'mcp_timeout' : 'mcp_error';
    throw failure;
  }
  if (response.isError) {
    const error = new Error('MCP operation failed');
    error.category = 'mcp_error';
    throw error;
  }
  return response.structuredContent?.result ?? response.structuredContent ?? null;
}

async function state(deadline = Date.now() + DEADLINE_MS) {
  const value = await call('playback_state', { offset: 0, limit: 10 }, deadline);
  if (!value || typeof value.status !== 'string') {
    const error = new Error('Playback state was invalid');
    error.category = 'invalid_state';
    throw error;
  }
  return value;
}

function diagnostics(snapshot) {
  const value = snapshot?.diagnostics;
  return value && typeof value === 'object' ? value : {};
}

function summarizeState(snapshot) {
  const info = diagnostics(snapshot);
  return {
    activeTrackId: safeId(identity(snapshot)),
    status: ['playing', 'paused', 'buffering', 'stopped', 'idle', 'error', 'ended'].includes(snapshot?.status) ? snapshot.status : 'unknown',
    positionSeconds: metric(snapshot?.position),
    durationSeconds: metric(snapshot?.duration),
    generation: Number.isSafeInteger(snapshot?.generation) ? snapshot.generation : null,
    seekSerial: Number.isSafeInteger(snapshot?.seekSerial) ? snapshot.seekSerial : null,
    nativeFirstAudioMs: metric(info.firstAudioMs),
    nativeSourceReadyMs: metric(info.sourceReadyMs),
    nativeSeekFirstAudioMs: metric(info.seekFirstAudioMs),
    nativeSeekOperationFirstAudioMs: metric(info.seekOperationFirstAudioMs),
    nativeClickToIpcMs: metric(info.clickToIpcMs),
    nativeCommandSerializedBytes: metric(info.commandSerializedBytes),
    nativeSeekClickToIpcMs: metric(info.seekClickToIpcMs),
    nativeSeekCommandSerializedBytes: metric(info.seekCommandSerializedBytes),
    nativeOperationId: safeId(info.operationId),
    nativeSeekOperationId: safeId(info.seekOperationId),
  };
}

async function waitFor(predicate, deadline) {
  let latest = null;
  while (Date.now() < deadline) {
    latest = await state(deadline);
    if (predicate(latest)) return latest;
    await sleep(POLL_MS);
  }
  return latest;
}

function failureForStartup(snapshot, advanced) {
  if (!snapshot || identity(snapshot) !== trackId) return 'wrong_active_track';
  if (!finite(diagnostics(snapshot).firstAudioMs)) return 'first_audio_timeout';
  if (!advanced) return 'playhead_did_not_advance';
  return null;
}

async function startAttempt(kind, index) {
  const harnessOperationId = randomUUID();
  const attemptStartedAt = Date.now();
  const deadline = attemptStartedAt + DEADLINE_MS;
  let before = null;
  let callFinishedAt = attemptStartedAt;
  let latest = null;
  let playheadAdvanced = false;
  let firstAudioObservedAt = null;
  let playheadAdvanceObservedAt = null;
  let failureCategory = null;
  try {
    before = await state(deadline);
    const beforeGeneration = Number.isSafeInteger(before.generation) ? before.generation : null;
    await call('play_tracks', { trackIds: [trackId] }, deadline);
    callFinishedAt = Date.now();
    latest = await waitFor((snapshot) => {
      const generationChanged = beforeGeneration === null || snapshot.generation !== beforeGeneration;
      const rightTrack = generationChanged && identity(snapshot) === trackId;
      if (rightTrack && firstAudioObservedAt === null && finite(diagnostics(snapshot).firstAudioMs)) firstAudioObservedAt = Date.now();
      const advanced = rightTrack && finite(snapshot.position) && snapshot.position > 0.05;
      if (advanced && !playheadAdvanced) playheadAdvanceObservedAt = Date.now();
      if (advanced) playheadAdvanced = true;
      return rightTrack && firstAudioObservedAt !== null && playheadAdvanced;
    }, deadline);
    failureCategory = failureForStartup(latest, playheadAdvanced);
    if (!failureCategory && identity(latest) !== trackId) failureCategory = 'wrong_active_track';
    if (!failureCategory && (!finite(diagnostics(latest).firstAudioMs) || !playheadAdvanced)) failureCategory = 'startup_timeout';
  } catch (error) {
    failureCategory = error?.category ?? 'mcp_error';
    callFinishedAt = Date.now();
  }
  return {
    type: kind,
    index,
    harnessOperationId,
    wallCallMs: Math.max(0, callFinishedAt - attemptStartedAt),
    wallFirstAudioObservedMs: firstAudioObservedAt === null ? null : Math.max(0, firstAudioObservedAt - attemptStartedAt),
    wallPlayheadAdvanceMs: playheadAdvanceObservedAt === null ? null : Math.max(0, playheadAdvanceObservedAt - attemptStartedAt),
    correctActiveId: Boolean(latest && identity(latest) === trackId),
    playheadAdvanced,
    native: latest ? summarizeState(latest) : null,
    failureCategory,
  };
}

async function seekAttempt(fraction, index) {
  const harnessOperationId = randomUUID();
  const attemptStartedAt = Date.now();
  const deadline = attemptStartedAt + DEADLINE_MS;
  let latest = null;
  let failureCategory = null;
  let targetSeconds = null;
  let wallCallMs = null;
  let callFinishedAt = attemptStartedAt;
  let seekRequestStartedAt = null;
  let seekPlayheadAdvanced = false;
  let seekSerialAdvanced = false;
  let seekAudioObservedAt = null;
  let seekAdvanceObservedAt = null;
  try {
    const before = await state(deadline);
    const duration = Number(before.duration);
    if (identity(before) !== trackId || before.status !== 'playing' || !Number.isFinite(duration) || duration <= 0) {
      failureCategory = 'seek_requires_active_playback';
      latest = before;
    } else {
      targetSeconds = Math.min(Math.max(0, duration - 0.5), Math.max(0, duration * fraction));
      const beforeSerial = Number.isSafeInteger(before.seekSerial) ? before.seekSerial : 0;
      seekRequestStartedAt = Date.now();
      await call('seek_playback', { position: targetSeconds }, deadline);
      callFinishedAt = Date.now();
      wallCallMs = callFinishedAt - seekRequestStartedAt;
      latest = await waitFor((snapshot) => {
        const advancedSerial = Number.isSafeInteger(snapshot.seekSerial) && snapshot.seekSerial > beforeSerial;
        seekSerialAdvanced ||= advancedSerial;
        const correctTrack = identity(snapshot) === trackId;
        if (advancedSerial && correctTrack && seekAudioObservedAt === null && finite(diagnostics(snapshot).seekFirstAudioMs)) seekAudioObservedAt = Date.now();
        const atTarget = correctTrack && finite(snapshot.position) && Math.abs(snapshot.position - targetSeconds) <= 2;
        if (atTarget && snapshot.status === 'playing' && snapshot.position > targetSeconds + 0.05) {
          if (!seekPlayheadAdvanced) seekAdvanceObservedAt = Date.now();
          seekPlayheadAdvanced = true;
        }
        return advancedSerial && atTarget && seekAudioObservedAt !== null && seekPlayheadAdvanced;
      }, deadline);
      if (identity(latest) !== trackId) failureCategory = 'wrong_active_track';
      else if (!seekSerialAdvanced || !finite(diagnostics(latest).seekFirstAudioMs)) failureCategory = 'seek_first_audio_timeout';
      else if (!seekPlayheadAdvanced) failureCategory = 'seek_playhead_did_not_advance';
    }
  } catch (error) {
    failureCategory = error?.category ?? 'mcp_error';
    callFinishedAt = Date.now();
  }
  return {
    type: 'playing_seek',
    fraction,
    index,
    harnessOperationId,
    targetSeconds: metric(targetSeconds),
    wallCallMs: metric(wallCallMs),
    wallSeekFirstAudioObservedMs: seekAudioObservedAt === null || seekRequestStartedAt === null ? null : Math.max(0, seekAudioObservedAt - seekRequestStartedAt),
    wallSeekPlayheadAdvanceMs: seekAdvanceObservedAt === null || seekRequestStartedAt === null ? null : Math.max(0, seekAdvanceObservedAt - seekRequestStartedAt),
    correctActiveId: Boolean(latest && identity(latest) === trackId),
    playheadAdvanced: seekPlayheadAdvanced,
    native: latest ? summarizeState(latest) : null,
    failureCategory,
  };
}

async function pausedSeekCheck() {
  const harnessOperationId = randomUUID();
  const seekStartedAt = Date.now();
  const deadline = seekStartedAt + DEADLINE_MS;
  let failureCategory = null;
  let pausedBefore = null;
  let afterSeek = null;
  let afterResume = null;
  let targetSeconds = null;
  let callMs = null;
  let wallPausedSeekMs = null;
  let resumedPlayheadAdvanced = false;
  try {
    await call('pause_playback', {}, deadline);
    pausedBefore = await waitFor((snapshot) => identity(snapshot) === trackId && (snapshot.status === 'paused' || snapshot.paused === true), deadline);
    if (identity(pausedBefore) !== trackId || (pausedBefore.status !== 'paused' && pausedBefore.paused !== true)) {
      failureCategory = 'pause_timeout';
    } else {
      const duration = Number(pausedBefore.duration);
      if (!Number.isFinite(duration) || duration <= 0) failureCategory = 'invalid_duration';
      else {
        targetSeconds = Math.min(duration * 0.33, Math.max(0, duration - 0.5));
        const beforeSerial = Number.isSafeInteger(pausedBefore.seekSerial) ? pausedBefore.seekSerial : 0;
        const seekStarted = Date.now();
        await call('seek_playback', { position: targetSeconds }, deadline);
        const seekCallFinished = Date.now();
        callMs = seekCallFinished - seekStarted;
        afterSeek = await waitFor((snapshot) => Number.isSafeInteger(snapshot.seekSerial) && snapshot.seekSerial > beforeSerial
          && identity(snapshot) === trackId && finite(snapshot.position) && Math.abs(snapshot.position - targetSeconds) <= 2,
        deadline);
        wallPausedSeekMs = Date.now() - seekStarted;
        if (identity(afterSeek) !== trackId || !finite(afterSeek?.position) || Math.abs(afterSeek.position - targetSeconds) > 2) failureCategory = 'paused_seek_timeout';
        else if (afterSeek.status !== 'paused' && afterSeek.paused !== true) failureCategory = 'paused_seek_started_playback';
        else if (finite(diagnostics(afterSeek).seekFirstAudioMs)) failureCategory = 'paused_seek_reported_audio';
      }
    }
    await call('resume_playback', {}, deadline);
    const resumeStart = Date.now();
    afterResume = await waitFor((snapshot) => identity(snapshot) === trackId && snapshot.status === 'playing'
      && finite(snapshot.position) && finite(afterSeek?.position) && snapshot.position > afterSeek.position + 0.05,
    deadline);
    resumedPlayheadAdvanced = Boolean(afterResume && identity(afterResume) === trackId && finite(afterSeek?.position)
      && finite(afterResume.position) && afterResume.position > afterSeek.position + 0.05);
    if (!failureCategory && !resumedPlayheadAdvanced) failureCategory = 'resume_playhead_did_not_advance';
  } catch (error) {
    failureCategory ??= error?.category ?? 'mcp_error';
  } finally {
    await call('pause_playback', {}, Date.now() + 5_000).catch(() => {});
  }
  return {
    type: 'paused_seek',
    harnessOperationId,
    targetSeconds: metric(targetSeconds),
    wallCallMs: metric(callMs),
    wallPausedSeekMs: metric(wallPausedSeekMs),
    pausedCorrectActiveId: Boolean(pausedBefore && identity(pausedBefore) === trackId),
    remainedPaused: Boolean(afterSeek && (afterSeek.status === 'paused' || afterSeek.paused === true)),
    pausedSeekNativeFirstAudioMs: metric(diagnostics(afterSeek).firstAudioMs),
    pausedSeekNativeSeekFirstAudioMs: metric(diagnostics(afterSeek).seekFirstAudioMs),
    resumedPlayheadAdvanced,
    resumeNativeSeekFirstAudioMs: metric(diagnostics(afterResume).seekFirstAudioMs),
    nativeOperationId: safeId(diagnostics(afterSeek).operationId),
    nativeSeekOperationId: safeId(diagnostics(afterSeek).seekOperationId),
    failureCategory,
  };
}

async function main() {
  const runStartedAt = new Date().toISOString();
  let track = null;
  let setupFailure = null;
  const warmups = [];
  const starts = [];
  const seeks = [];
  let pausedSeek = null;
  try {
    await requireIsolatedProfile(automationFile);
    await client.connect(transport, { timeout: DEADLINE_MS });
    const response = await call('get_track', { trackId });
    track = response?.track ?? response;
    if (!track || (typeof track.localPath !== 'string' && track.localAvailability !== 'available')) {
      setupFailure = 'fixture_not_local';
      return;
    }
    await call('set_volume', { volume: 0 });
    for (let index = 0; index < 3; index++) warmups.push(await startAttempt('warmup', index + 1));
    for (let index = 0; index < START_ATTEMPTS; index++) starts.push(await startAttempt('local_start', index + 1));
    for (const fraction of SEEK_FRACTIONS) {
      for (let index = 0; index < SEEK_REPEATS_PER_FRACTION; index++) seeks.push(await seekAttempt(fraction, index + 1));
    }
    pausedSeek = await pausedSeekCheck();
  } catch (error) {
    setupFailure = error?.category ?? 'setup_or_mcp_error';
  } finally {
    await call('pause_playback', {}, Date.now() + 5_000).catch(() => {});
    const result = {
      schemaVersion: 1,
      workload: 'real_mcp_local_native_playback_qualification',
      runStartedAt,
      runFinishedAt: new Date().toISOString(),
      trackId: safeId(trackId),
      localTrackVerified: Boolean(track && (typeof track.localPath === 'string' || track.localAvailability === 'available')),
      volumeSetToZero: true,
      bounds: { warmups: 3, localStarts: START_ATTEMPTS, seeksPerFraction: SEEK_REPEATS_PER_FRACTION, seekFractions: SEEK_FRACTIONS, perOperationDeadlineMs: DEADLINE_MS },
      startupWallBoundary: 'Harness time from before MCP request through observing native snapshot diagnostics; includes MCP, loopback transport, IPC and polling latency. Bypasses the renderer Play click.',
      nativeBoundary: 'firstAudioMs and seekFirstAudioMs are copied from native snapshot diagnostics; these exclude loopback and MCP transport latency.',
      setupFailure,
      warmups,
      starts,
      seeks,
      pausedSeek,
      failureCount: [...warmups, ...starts, ...seeks, ...(pausedSeek ? [pausedSeek] : [])].filter((attempt) => attempt.failureCategory).length + (setupFailure ? 1 : 0),
      limitations: [
        'This driver uses MCP tool calls and does not click the UI or claim codec identity.',
        'The MCP playback tools do not accept caller operation IDs; harnessOperationId identifies each attempt, while native IDs are captured only when exposed in diagnostics.',
        'Output is allowlisted metrics and fixed failure categories; source URLs, local paths, credentials, and stderr are omitted.',
      ],
    };
    try {
      const destination = resolve(outputFile);
      await mkdir(dirname(destination), { recursive: true });
      await writeFile(destination, `${JSON.stringify(result, null, 2)}\n`, { mode: 0o600 });
      console.log(JSON.stringify({ outputFile: destination, localTrackVerified: result.localTrackVerified, setupFailure, starts: starts.length, seeks: seeks.length }));
      if (result.failureCount > 0) process.exitCode = 1;
    } catch {
      console.error('output_write_failed');
      process.exitCode = 1;
    }
    await client.close().catch(() => {});
  }
}

await main();
