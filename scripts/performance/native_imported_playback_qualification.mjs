import { safeBuffering } from './native_qualification_diagnostics.mjs';
import { createHash, randomUUID } from 'node:crypto';
import { lstat, readFile, realpath, writeFile } from 'node:fs/promises';
import { basename, dirname, join, resolve } from 'node:path';
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { createSustainedPlaybackCheck } from './sustained_playback.mjs';
import { prepareImportedCase, sourceIdentityStatus } from './imported_playback_case.mjs';
import { isVerifiedBandcampInput, qualifyPreparedStartup, runOneShotProtocol } from './one_shot_playback.mjs';

const [automationFile, trackId, outputFile, binaryFile, expectedDurationArgument, expectedProviderArgument, expectedRecordingIdArgument, modeArgument] = process.argv.slice(2);
const oneShot = modeArgument === '--one-shot-bandcamp';
const expectedDurationSeconds = Number(expectedDurationArgument);
const expectedProvider = expectedProviderArgument || null;
const expectedRecordingId = expectedRecordingIdArgument || null;
if (!automationFile || !trackId || !outputFile || !binaryFile || !Number.isFinite(expectedDurationSeconds) || expectedDurationSeconds <= 0
    || (modeArgument && !oneShot)
    || (oneShot && !isVerifiedBandcampInput(expectedProvider, expectedRecordingId, expectedDurationSeconds))
    || (!oneShot && expectedProvider && !['monochrome', 'youtube', 'soundcloud'].includes(expectedProvider))
    || (expectedRecordingId && !/^[a-zA-Z0-9._:-]{1,100}$/.test(expectedRecordingId))) {
  console.error('usage: native_imported_playback_qualification.mjs AUTOMATION_FILE TRACK_ID OUTPUT_FILE BINARY_FILE INDEPENDENTLY_VERIFIED_DURATION_SECONDS [monochrome|youtube|soundcloud] [EXPECTED_SOURCE_RECORDING_ID] [--one-shot-bandcamp]');
  process.exit(2);
}
const client = new Client({ name: 'burrata-native-imported-playback-qualification', version: '1.0.0' });
const transport = new StdioClientTransport({
  command: 'bash',
  args: ['scripts/dev/mcp-stdio.sh'],
  cwd: process.cwd(),
  env: { ...process.env, BURRATA_AUTOMATION_FILE: automationFile },
  stderr: 'pipe',
});
transport.stderr?.on('data', () => {});
const deadlineMs = 20_000;
let oneShotEntryId = null;
const sleep = (ms) => new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
const finite = (value) => typeof value === 'number' && Number.isFinite(value);
const number = (value) => finite(value) && value >= 0 ? value : null;
const id = (value) => typeof value === 'string' && /^[a-zA-Z0-9._:-]{1,100}$/.test(value) ? value : null;
// Validate against the actual selected recording, never the decoder's own estimate.
// Otherwise both timing and 95% seeks can appear to pass for a truncated stream.
const durationMatches = (value) => finite(value) && Math.abs(value - expectedDurationSeconds) <= 1;

async function requireIsolatedProfile(discoveryFile) {
  try {
    const canonicalFile = await realpath(resolve(discoveryFile));
    const profileRoot = resolve(dirname(canonicalFile), '../..');
    const profileName = basename(profileRoot);
    if (dirname(profileRoot) !== '/tmp' || !/^burrata-launch\.[A-Za-z0-9_-]+$/.test(profileName)
        || canonicalFile !== join(profileRoot, 'data', 'org.burrata.discovery', 'automation.json')) {
      throw new Error('isolated_profile_required');
    }
    const marker = join(profileRoot, '.burrata-isolated-profile');
    const info = await lstat(marker);
    if (info.isSymbolicLink() || !info.isFile()
        || await readFile(marker, 'utf8') !== 'burrata isolated memory run\n') {
      throw new Error('isolated_profile_required');
    }
  } catch {
    throw new Error('isolated_profile_required');
  }
}


async function call(name, args = {}, deadline = Date.now() + deadlineMs) {
  const timeout = Math.max(1, deadline - Date.now());
  const response = await client.callTool({ name, arguments: args }, { timeout });
  if (response.isError) {
    const error = new Error('tool_failed');
    error.category = 'tool_error';
    return { error, result: response.structuredContent?.error ?? response.content?.[0]?.text };
  }
  return { result: response.structuredContent?.result ?? response.structuredContent ?? null };
}

async function state(deadline = Date.now() + deadlineMs) {
  const { error, result } = await call('playback_state', { offset: 0, limit: 10 }, deadline);
  if (error || !result || typeof result.status !== 'string') throw new Error('invalid_state');
  return result;
}


function safeState(value) {
  const diagnostics = value?.diagnostics ?? {};
  const decoder = diagnostics.decoder;
  const quality = value?.quality;
  return {
    status: ['playing', 'paused', 'buffering', 'stopped', 'idle', 'failed', 'error', 'ended'].includes(value?.status) ? value.status : 'unknown',
    engine: value?.engine === 'native' ? 'native' : 'other',
    trackId: id(value?.trackId),
    positionSeconds: number(value?.position),
    durationSeconds: number(value?.duration),
    generation: Number.isSafeInteger(value?.generation) ? value.generation : null,
    seekSerial: Number.isSafeInteger(value?.seekSerial) ? value.seekSerial : null,
    outputReady: typeof diagnostics.outputReady === 'boolean' ? diagnostics.outputReady : null,
    pendingSeekGeneration: Number.isSafeInteger(diagnostics.pendingSeekGeneration) ? diagnostics.pendingSeekGeneration : null,
    sourceReadyMs: number(diagnostics.sourceReadyMs),
    lastBuffering: safeBuffering(value),
    firstAudioMs: number(diagnostics.firstAudioMs),
    seekFirstAudioMs: number(diagnostics.seekFirstAudioMs),
    selectedFormat: quality && typeof quality === 'object' ? {
      id: typeof quality.formatId === 'string' && /^[a-zA-Z0-9._-]{1,40}$/.test(quality.formatId) ? quality.formatId : null,
      hasVideo: typeof quality.formatHasVideo === 'boolean' ? quality.formatHasVideo : null,
      expectedContentLength: number(quality.expectedContentLength),
    } : null,
    decoder: decoder && typeof decoder === 'object' ? {
      kind: ['rodio', 'experimental-opus', 'ffmpeg'].includes(decoder.decoderKind) ? decoder.decoderKind : 'unknown',
      contentType: ['audio/webm', 'audio/mp4', 'audio/ogg', 'audio/mpeg', 'application/octet-stream'].includes(decoder.contentType) ? decoder.contentType : null,
      inputContentLength: number(decoder.inputContentLength),
      contentRangeTotal: number(decoder.contentRangeTotal),
      bytesRead: number(decoder.bytesRead),
      bytesReceived: number(decoder.bytesReceived),
      sampleRate: number(decoder.sampleRate),
      channels: number(decoder.channels),
      totalDurationSeconds: number(decoder.totalDurationSeconds),
    } : null,
    sourceProvider: ['monochrome', 'youtube', 'soundcloud', 'bandcamp'].includes(value?.candidate?.provider) ? value.candidate.provider : null,
    sourceRecordingId: id(value?.candidate?.id),
    entryId: id(value?.entryId),
  };
}

async function startAttempt(type, index) {
  const startedAt = Date.now();
  const deadline = startedAt + deadlineMs;
  let before;
  let latest;
  let playheadObservedAt = null;
  let firstAudioObservedAt = null;
  let failureCategory = null;
  let playCallResult = null;
  const continued = createSustainedPlaybackCheck();
  let continuedPlayback = null;
  try {
    before = await state(deadline);
    if (before.engine !== 'native' || before.trackId !== trackId) throw new Error('wrong_engine_or_track');
    if (expectedProvider && before.candidate?.provider !== expectedProvider) throw new Error('wrong_provider');
    if (expectedRecordingId && before.candidate?.id !== expectedRecordingId) throw new Error('wrong_source_recording');
    const beforeGeneration = before.generation;
    const playPromise = call('play_tracks', { trackIds: [trackId] }, deadline)
      .then(({ error, result }) => {
        playCallResult = error ? 'tool_error' : result?.ok === false ? 'play_failed' : 'ok';
        return error;
      })
      .catch(() => { playCallResult = 'mcp_error'; return new Error('mcp_error'); });
    while (Date.now() < deadline) {
      latest = await state(deadline);
      const fresh = Number.isSafeInteger(latest.generation) && latest.generation !== beforeGeneration;
      const sourceStatus = sourceIdentityStatus(latest, expectedProvider, expectedRecordingId,
        before?.diagnostics?.operationId);
      const current = fresh && latest.engine === 'native' && latest.trackId === trackId
        && sourceStatus === 'matched';
      if (fresh && latest.engine === 'native' && latest.trackId === trackId
          && ['wrong_provider', 'wrong_source_recording'].includes(sourceStatus)) {
        failureCategory = sourceStatus;
        break;
      }
      if (current && firstAudioObservedAt === null && finite(latest.diagnostics?.firstAudioMs)) firstAudioObservedAt = Date.now();
      if (current && latest.status === 'playing' && finite(latest.position) && latest.position > 0.05 && playheadObservedAt === null) playheadObservedAt = Date.now();
      if (current && firstAudioObservedAt !== null && playheadObservedAt !== null) {
        continuedPlayback = continued.observe(latest, Date.now());
        if (continuedPlayback.status === 'failed') { failureCategory = continuedPlayback.failureCategory; break; }
        if (continuedPlayback.status === 'ready') break;
      } else if (continuedPlayback) { failureCategory = 'continued_playback_session_changed'; break; }
      if (current && ['failed', 'error'].includes(latest.status)) { failureCategory = 'startup_failed'; break; }
      await sleep(100);
    }
    if (!failureCategory && (!latest || latest.generation === beforeGeneration)) failureCategory = 'generation_not_advanced';
    if (!failureCategory && firstAudioObservedAt === null) failureCategory = 'first_audio_timeout';
    if (!failureCategory && playheadObservedAt === null) failureCategory = 'playhead_did_not_advance';
    if (!failureCategory && continuedPlayback?.status !== 'ready') failureCategory = 'continued_playback_timeout';
    if (!failureCategory && !durationMatches(latest?.duration)) failureCategory = 'recording_duration_mismatch';
    if (['mcp_error', 'tool_error', 'play_failed'].includes(playCallResult)) failureCategory ??= playCallResult;
    await Promise.race([playPromise, sleep(750)]);
  } catch (error) {
    failureCategory ??= error?.message === 'wrong_engine_or_track' ? 'wrong_engine_or_track' : error?.message === 'wrong_provider' ? 'wrong_provider' : 'mcp_error';
  }
  const record = {
    type,
    index,
    harnessOperationId: randomUUID(),
    wallFirstAudioObservedMs: firstAudioObservedAt === null ? null : firstAudioObservedAt - startedAt,
    wallPlayheadAdvanceMs: playheadObservedAt === null ? null : playheadObservedAt - startedAt,
    native: latest ? safeState(latest) : null,
    playCallResult,
    continuedPlayback,
    failureCategory,
  };
  console.log(JSON.stringify(record));
  return record;
}

async function seekAttempt(fraction, index) {
  const startedAt = Date.now();
  const deadline = startedAt + deadlineMs;
  let latest;
  let targetSeconds = null;
  let callMs = null;
  let seekCallFailed = false;
  let wallAudioObservedMs = null;
  let wallAdvanceObservedMs = null;
  const continued = createSustainedPlaybackCheck();
  let continuedPlayback = null;
  let failureCategory = null;
  try {
    const before = await state(deadline);
    const duration = expectedDurationSeconds;
    if (before.engine !== 'native' || before.trackId !== trackId || before.status !== 'playing' || !Number.isFinite(duration) || duration <= 0) {
      failureCategory = 'seek_requires_native_playing_track';
    } else if (oneShot && (before.entryId !== oneShotEntryId || !oneShotEntryId)) {
      failureCategory = 'occurrence_changed';
    } else if (!durationMatches(before.duration)) {
      failureCategory = 'recording_duration_mismatch';
    } else if (sourceIdentityStatus(before, expectedProvider, expectedRecordingId) !== 'matched') {
      const sourceStatus = sourceIdentityStatus(before, expectedProvider, expectedRecordingId);
      failureCategory = sourceStatus === 'pending' ? 'source_identity_not_ready' : sourceStatus;
    } else {
      targetSeconds = Math.min(Math.max(0, duration - 0.5), Math.max(0, duration * fraction));
      if (duration - targetSeconds < 5.25) throw new Error('insufficient_remaining_audio');
      const serial = Number.isSafeInteger(before.seekSerial) ? before.seekSerial : 0;
      const seekStartedAt = Date.now();
      const seekPromise = call('seek_playback', { position: targetSeconds }, deadline)
        .then(({ error, result }) => { callMs = Date.now() - seekStartedAt; seekCallFailed = Boolean(error || result?.ok === false); return error; })
        .catch(() => { seekCallFailed = true; callMs = Date.now() - seekStartedAt; return new Error('mcp_error'); });
      while (Date.now() < deadline) {
        latest = await state(deadline);
        const fresh = Number.isSafeInteger(latest.seekSerial) && latest.seekSerial > serial;
        const sourceStatus = sourceIdentityStatus(latest, expectedProvider, expectedRecordingId);
        const correct = latest.trackId === trackId && latest.engine === 'native' && sourceStatus === 'matched';
        if (oneShot && (latest.entryId !== before.entryId || typeof before.entryId !== 'string' || !before.entryId)) {
          failureCategory = 'occurrence_changed'; break;
        }
        if (latest.trackId === trackId && latest.engine === 'native'
            && ['wrong_provider', 'wrong_source_recording'].includes(sourceStatus)) {
          failureCategory = sourceStatus;
          break;
        }
        if (fresh && correct && wallAudioObservedMs === null && finite(latest.diagnostics?.seekFirstAudioMs)) wallAudioObservedMs = Date.now() - seekStartedAt;
        const atTarget = correct && finite(latest.position) && Math.abs(latest.position - targetSeconds) <= 2;
        if (fresh && correct && atTarget && latest.status === 'playing' && latest.position > targetSeconds + 0.05 && wallAdvanceObservedMs === null) wallAdvanceObservedMs = Date.now() - seekStartedAt;
        if (fresh && correct && wallAudioObservedMs !== null && wallAdvanceObservedMs !== null) {
          continuedPlayback = continued.observe(latest, Date.now(), 'seekFirstAudioMs');
          if (continuedPlayback.status === 'failed') { failureCategory = continuedPlayback.failureCategory; break; }
          if (continuedPlayback.status === 'ready') break;
        } else if (continuedPlayback) { failureCategory = 'continued_playback_session_changed'; break; }
        if (correct && ['failed', 'error'].includes(latest.status)) { failureCategory = 'seek_failed'; break; }
        await sleep(100);
      }
      await Promise.race([seekPromise, sleep(750)]);
      if (oneShot && seekCallFailed) failureCategory ??= 'seek_tool_error';
      if (!failureCategory && (!latest || latest.seekSerial <= serial)) failureCategory = 'seek_serial_timeout';
      if (!failureCategory && wallAudioObservedMs === null) failureCategory = 'seek_first_audio_timeout';
      if (!failureCategory && wallAdvanceObservedMs === null) failureCategory = 'seek_playhead_did_not_advance';
      if (!failureCategory && continuedPlayback?.status !== 'ready') failureCategory = 'continued_playback_timeout';
    }
  } catch (error) {
    failureCategory ??= error?.message === 'insufficient_remaining_audio' ? 'insufficient_remaining_audio' : 'mcp_error';
  }
  const record = {
    type: 'playing_seek', fraction, index, harnessOperationId: randomUUID(),
    targetSeconds: number(targetSeconds), wallCallMs: number(callMs),
    wallSeekFirstAudioObservedMs: number(wallAudioObservedMs),
    wallSeekPlayheadAdvanceMs: number(wallAdvanceObservedMs),
    continuedPlayback,
    native: latest ? safeState(latest) : null,
    failureCategory,
  };
  console.log(JSON.stringify(record));
  return record;
}

async function main() {
  const startedAt = new Date().toISOString();
  const binaryHash = createHash('sha256').update(await readFile(binaryFile)).digest('hex');
  const sourceHashes = {
    qualificationRunner: createHash('sha256').update(await readFile(new URL(import.meta.url))).digest('hex'),
    qualificationDiagnostics: createHash('sha256').update(await readFile(new URL('./native_qualification_diagnostics.mjs', import.meta.url))).digest('hex'),
  };
  for (const sourcePath of ['src-tauri/src/playback.rs', 'src-tauri/src/native_output.rs']) {
    sourceHashes[sourcePath] = createHash('sha256').update(await readFile(resolve(sourcePath))).digest('hex');
  }
  let before;
  let initial = null;
  let setupSnapshot = null;
  const warmups = [];
  const starts = [];
  const seeks = [];
  let pausedSeek = null;
  let setupFailure = null;
  let volumeSetToZero = false;
  let cleanupFailure = null;
  let pausedFinally = null;
  try {
    await requireIsolatedProfile(automationFile);
    await client.connect(transport, { timeout: deadlineMs });
    initial = await state();
    if (initial.engine !== 'native') throw new Error('wrong_engine_or_track');
    const preparationDeadline = Date.now() + deadlineMs;
    try {
      before = await prepareImportedCase({ call, state, sleep,
        onMuted: () => { volumeSetToZero = true; } }, initial, {
        trackId, expectedProvider, expectedRecordingId, expectedDurationSeconds,
      }, preparationDeadline);
    } catch (error) {
      setupSnapshot = error?.snapshot ?? null;
      throw error;
    }
    if (oneShot) {
      oneShotEntryId = before.entryId;
      const result = await runOneShotProtocol(async () => {
        const attempt = await qualifyPreparedStartup(before, { state, sleep, deadline: preparationDeadline,
          trackId, expectedProvider, expectedRecordingId, expectedDurationSeconds });
        const { snapshot, ...safe } = attempt;
        return { ...safe, native: snapshot ? safeState(snapshot) : null };
      }, seekAttempt);
      starts.push(result.start);
      seeks.push(...result.seeks);
    } else {
    for (let index = 0; index < 3; index++) {
      const attempt = await startAttempt('warmup', index + 1);
      warmups.push(attempt);
      if (attempt.failureCategory) throw new Error('startup_sample_failed');
      await call('pause_playback', {}, Date.now() + 5_000);
    }
    for (let index = 0; index < 20; index++) {
      const attempt = await startAttempt('native_start', index + 1);
      starts.push(attempt);
      if (attempt.failureCategory) throw new Error('startup_sample_failed');
      await call('pause_playback', {}, Date.now() + 5_000);
    }
    const setup = await startAttempt('seek_setup', 1);
    if (setup.failureCategory) throw new Error('seek_setup_failed');
    for (const fraction of [0.01, 0.5, 0.95]) {
      for (let index = 0; index < 20; index++) {
        const attempt = await seekAttempt(fraction, index + 1);
        seeks.push(attempt);
        if (attempt.failureCategory) throw new Error('seek_sample_failed');
      }
    }
    await call('pause_playback', {}, Date.now() + 5_000);
    const paused = await state(Date.now() + 5_000);
    const pausedSerial = Number.isSafeInteger(paused.seekSerial) ? paused.seekSerial : 0;
    const duration = expectedDurationSeconds;
    if (paused.trackId === trackId && paused.status === 'paused' && finite(duration) && duration > 0) {
      const target = Math.min(duration * 0.33, duration - 0.5);
      const seekBefore = safeState(paused);
      const seek = await call('seek_playback', { position: target }, Date.now() + deadlineMs);
      let after = null;
      const deadline = Date.now() + deadlineMs;
      while (Date.now() < deadline) {
        after = await state(deadline);
        if (after.seekSerial > pausedSerial && after.status === 'paused' && Math.abs(after.position - target) <= 2) break;
        await sleep(100);
      }
      const valid = after && after.seekSerial > pausedSerial && after.status === 'paused' && Math.abs(after.position - target) <= 2 && !finite(after.diagnostics?.seekFirstAudioMs);
      await call('resume_playback', {}, Date.now() + 5_000);
      const resumedAt = Date.now();
      let resumed = null;
      const continuedResume = createSustainedPlaybackCheck();
      let resumedProgress = null;
      while (Date.now() - resumedAt < deadlineMs) {
        resumed = await state(resumedAt + deadlineMs);
        if (resumed.trackId !== trackId || resumed.generation !== after?.generation
            || resumed.seekSerial !== after?.seekSerial
            || sourceIdentityStatus(resumed, expectedProvider, expectedRecordingId) !== 'matched') {
          resumedProgress = { status: 'failed', failureCategory: 'continued_playback_session_changed' }; break;
        }
        resumedProgress = continuedResume.observe(resumed, Date.now());
        if (resumedProgress.status === 'ready' || resumedProgress.status === 'failed') break;
        await sleep(100);
      }
      pausedSeek = {
        type: 'paused_seek', harnessOperationId: randomUUID(), targetSeconds: target,
        remainedPaused: Boolean(valid), resumeLatencyMeasured: false,
        resumedPlayheadAdvanced: resumedProgress?.status === 'ready', continuedPlayback: resumedProgress,
        transition: { before: seekBefore, after: after ? safeState(after) : null, afterResume: resumed ? safeState(resumed) : null },
        internalsAvailability: {
          outputReady: typeof paused.diagnostics?.outputReady === 'boolean' && typeof after?.diagnostics?.outputReady === 'boolean' ? 'exposed' : 'not_exposed_by_playback_snapshot',
          pendingSeekGeneration: Number.isSafeInteger(paused.diagnostics?.pendingSeekGeneration) && Number.isSafeInteger(after?.diagnostics?.pendingSeekGeneration) ? 'exposed' : 'not_exposed_by_playback_snapshot',
        },
        failureCategory: valid && resumedProgress?.status === 'ready' ? null : resumedProgress?.failureCategory ?? 'paused_seek_or_resume_failed',
      };
      if (seek.error) pausedSeek.failureCategory = 'paused_seek_tool_error';
    } else {
      pausedSeek = { type: 'paused_seek', failureCategory: 'paused_seek_setup_failed' };
    }
    }
  } catch (error) {
    const category = error?.category ?? error?.message;
    setupFailure = ['unexpected_isolated_queue', 'wrong_engine_or_track', 'wrong_provider', 'wrong_source_recording', 'case_selection_failed', 'case_prepare_timeout', 'recording_duration_mismatch', 'volume_setup_failed', 'isolated_profile_required'].includes(category) ? category : error?.message === 'startup_sample_failed' ? 'startup_sample_failed' : error?.message === 'seek_sample_failed' ? 'seek_sample_failed' : error?.message === 'seek_setup_failed' ? 'seek_setup_failed' : 'setup_or_mcp_error';
  } finally {
    await call('pause_playback', {}, Date.now() + 5_000).then(({error, result}) => {
      if (oneShot && (error || result?.ok === false)) cleanupFailure = 'final_pause_failed';
    }).catch(() => { if (oneShot) cleanupFailure = 'final_pause_failed'; });
    if (oneShot && initial) {
      try { pausedFinally = (await state(Date.now() + 5_000)).status === 'paused'; } catch { pausedFinally = false; }
      if (!pausedFinally) cleanupFailure ??= 'final_pause_failed';
    }
    if (!oneShot && initial?.trackId === trackId && finite(initial.position) && initial.position > 0) {
      await call('seek_playback', { position: initial.position }, Date.now() + 5_000).catch(() => {});
      await call('pause_playback', {}, Date.now() + 5_000).catch(() => {});
    }
    if (finite(initial?.volume)) await call('set_volume', { volume: initial.volume }, Date.now() + 5_000).catch(() => {});
    volumeSetToZero = volumeSetToZero && true;
    const result = {
      schemaVersion: 3,
      workload: 'real_mcp_imported_native_playback_qualification',
      runStartedAt: startedAt,
      runFinishedAt: new Date().toISOString(),
      binarySha256: binaryHash,
      sourceSnapshotSha256: sourceHashes,
      sourceBinding: {
        status: 'unverified_binary_to_source_binding',
        explanation: 'Binary and current source hashes are recorded independently; no build manifest proves which source snapshot produced this binary.',
      },
      trackId: id(trackId),
      independentlyVerifiedDurationSeconds: expectedDurationSeconds,
      expectedProvider,
      expectedRecordingId,
      initialState: before ? safeState(before) : setupSnapshot ? safeState(setupSnapshot) : null,
      acousticContentVerified: false,
      durationToleranceSeconds: 1,
      engine: before?.engine === 'native' ? 'native' : 'other',
      initialVolume: number(initial?.volume),
      volumeSetToZero,
      initialTrackPositionSeconds: number(initial?.position),
      initialQueueCount: Array.isArray(initial?.queue) ? initial.queue.length : null,
      mode: oneShot ? 'one_shot_bandcamp' : 'matrix',
      bounds: { warmups: oneShot ? 0 : 3, starts: oneShot ? 1 : 20, seekRepeatsPerFraction: oneShot ? 1 : 20, seekFractions: oneShot ? [0.95, 0.01] : [0.01, 0.5, 0.95], continuedPlaybackMs: 5000, maximumObservationGapMs: 2000, perOperationDeadlineMs: deadlineMs },
      warmups, starts, seeks, pausedSeek, setupFailure, cleanupFailure, pausedFinally,
      failureCount: [...warmups, ...starts, ...seeks, ...(pausedSeek ? [pausedSeek] : [])].filter((item) => item.failureCategory).length + (setupFailure ? 1 : 0) + (cleanupFailure ? 1 : 0),
      limitations: [
        'Paused seek/resume uses the stable current native session firstAudioMs plus five seconds of consumed position; it does not measure resume latency or paused-seek first-audio latency.',
        'Every start and playing seek requires five seconds of stable native position advancement after mixer-consumed first audio, sampled with no gap exceeding two seconds.',
        'Native firstAudioMs indicates mixer-consumed audio frames; it does not measure physical loudspeaker audibility.',
        'Matching the expected source recording ID is metadata identity validation, not acoustic content verification; snapshots do not prove target landing or acoustic identity.',
        'Binary and source snapshot hashes are recorded independently; source-to-binary binding remains unverified without a build manifest.',
        'Output readiness and pending seek generation are emitted only if exposed in playback diagnostics; null with not_exposed_by_playback_snapshot means unavailable, not false.',
        'Startup and seek timing starts at MCP calls, excluding the renderer click path.',
        'The MCP playback tools do not accept caller operation IDs; harnessOperationId identifies each attempt and generation/seekSerial validate freshness.',
        'The existing isolated single-track queue is checked, the requested case is selected through MCP after setting volume to zero, and the original volume is restored afterward. URLs, local paths, credentials, and raw stderr are omitted.',
      ],
    };
    try { await writeFile(resolve(outputFile), `${JSON.stringify(result, null, 2)}\n`, { mode: 0o600, flag: 'wx' }); } catch { console.error('output_write_failed'); process.exitCode = 1; }
    await client.close().catch(() => {});
    console.log(JSON.stringify({ finished: true, starts: starts.length, seeks: seeks.length, pausedSeekFailure: pausedSeek?.failureCategory ?? null, setupFailure }));
    if (result.failureCount > 0) process.exitCode = 1;
  }
}

await main();
