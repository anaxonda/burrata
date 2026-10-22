// One MCP playback selection per manifest case. No library/tag/import APIs are called.
// Run only after review against an app profile where changing the queue is acceptable:
// BURRATA_AUTOMATION_FILE=/path/to/automation.json node scripts/performance/saved_track_one_attempt_smoke.mjs \
//   .cache/final-playback-regression-inputs/manifest.json .cache/final-playback-regression-inputs/run.json
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { readFile, writeFile, rename, unlink } from 'node:fs/promises';
import { resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { matchesAcceptedEntry, acceptedEntryOwnershipChanged, playbackSourceIdentity } from './saved_track_playback_fence.mjs';

const [manifestFile, outputFile, ...selectedCaseIds] = process.argv.slice(2);
const automationFile = process.env.BURRATA_AUTOMATION_FILE;
if (!manifestFile || !outputFile || !automationFile) {
  console.error('usage: BURRATA_AUTOMATION_FILE=... node saved_track_one_attempt_smoke.mjs MANIFEST OUTPUT [CASE_ID ...]');
  process.exit(2);
}

const manifest = JSON.parse(await readFile(manifestFile, 'utf8'));
if (manifest?.schemaVersion !== 1 || !Array.isArray(manifest.cases)) throw new Error('invalid_manifest');
const cases = selectedCaseIds.length
  ? manifest.cases.filter((item) => selectedCaseIds.includes(item.caseId))
  : manifest.cases;
if (!cases.length || cases.length !== (selectedCaseIds.length || manifest.cases.length)) throw new Error('case_not_found');
if (cases.some((item) => item.attempts !== 1 || typeof item.trackId !== 'string')) throw new Error('invalid_case');

const client = new Client({ name: 'burrata-saved-track-one-attempt-smoke', version: '1.0.0' });
const transport = new StdioClientTransport({
  command: 'bash', args: ['scripts/dev/mcp-stdio.sh'], cwd: process.cwd(),
  env: { ...process.env, BURRATA_AUTOMATION_FILE: automationFile }, stderr: 'pipe',
});
transport.stderr?.on('data', () => {});
const resolvedOutputFile = resolve(outputFile);
const sleep = (ms) => new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
const deadlineMs = manifest.attemptPolicy?.attemptDeadlineSeconds * 1000 || 45_000;
const progressSeconds = manifest.attemptPolicy?.minimumPlaybackProgressSeconds || 5;
const safeId = (value) => typeof value === 'string' && /^[A-Za-z0-9._:-]{1,120}$/.test(value) ? value : null;
const safeNumber = (value) => typeof value === 'number' && Number.isFinite(value) ? value : null;

async function call(name, args = {}, timeout = 10_000) {
  const response = await client.callTool({ name, arguments: args }, { timeout });
  if (response.isError) return { error: true, result: null };
  return { error: false, result: response.structuredContent?.result ?? response.structuredContent ?? null };
}

async function playbackState(timeout) {
  const response = await call('playback_state', { offset: 0, limit: 10 }, timeout);
  const value = response.result;
  if (response.error || !value || typeof value.status !== 'string') throw taggedError('state_unavailable');
  return value;
}

let currentCaseContext = null;
function safeErrorClass(error) {
  const known = ['tool_error', 'state_unavailable', 'timeout', 'receipt_unavailable'];
  if (known.includes(error?.safeClass)) return error.safeClass;
  if (error?.name === 'TimeoutError' || ['TIMEOUT', 'REQUEST_TIMEOUT'].includes(error?.code)) return 'timeout';
  if (error?.name === 'AbortError' || error?.code === 'ABORT_ERR') return 'cancelled';
  if (error?.code === 'CONNECTION_CLOSED') return 'transport_closed';
  return 'unexpected_error';
}
function taggedError(safeClass) {
  const error = new Error(safeClass);
  error.safeClass = safeClass;
  return error;
}

function identityFields(value) {
  const track = value?.track && typeof value.track === 'object' ? value.track : {};
  return {
    trackId: safeId(value?.trackId),
    title: typeof track.title === 'string' ? track.title.slice(0, 200) : null,
    artist: typeof track.artist === 'string' ? track.artist.slice(0, 400) : null,
    album: typeof track.album === 'string' ? track.album.slice(0, 200) : null,
    version: typeof track.version === 'string' ? track.version.slice(0, 200) : null,
    durationSeconds: safeNumber(track.durationSeconds ?? value?.duration),
    isrc: typeof track.isrc === 'string' && /^[A-Za-z0-9]{12}$/.test(track.isrc) ? track.isrc : null,
  };
}

function snapshot(value) {
  return {
    status: ['playing', 'paused', 'buffering', 'stopped', 'idle', 'failed', 'error', 'ended'].includes(value.status) ? value.status : 'unknown',
    engine: ['native', 'webview'].includes(value.engine) ? value.engine : 'other',
    identity: identityFields(value),
    positionSeconds: safeNumber(value.position),
    durationSeconds: safeNumber(value.duration),
    generation: Number.isSafeInteger(value.generation) ? value.generation : null,
    entryId: safeId(value.entryId),
    source: playbackSourceIdentity(value),
    mediaErrorPresent: Boolean(value.mediaError || value.lastMediaFailure || value.lastStartupFailure),
  };
}

async function runCase(item) {
  currentCaseContext = { stage: 'matching_mode_status', selectionSubmission: 'not_submitted' };
  const modeStatus = await call('get_app_status', {}, 10_000);
  if (modeStatus.error) throw taggedError('tool_error');
  const actualMode = modeStatus.result?.settings?.matchingLevel
    ?? modeStatus.result?.matchingLevel
    ?? modeStatus.result?.settings?.matching_level;
  if (actualMode?.toLowerCase?.() !== item.matchingMode.toLowerCase()) {
    return { caseId: item.caseId, trackId: item.trackId, attempts: 0,
      status: actualMode ? 'unsupported_matching_mode_mismatch' : 'unsupported_matching_mode_unverifiable',
      intendedMatchingMode: item.matchingMode, observedMatchingMode: actualMode ?? null,
      selectionSubmission: 'not_submitted', sourceIdEqualityGate: false };
  }
  currentCaseContext.stage = 'preselection_playback_state';
  const before = await playbackState(10_000);
  const startedAt = Date.now();
  const operationId = randomUUID();
  let selected;
  let receiptRecovered = false;
  currentCaseContext.stage = 'queue_submission';
  currentCaseContext.selectionSubmission = 'unknown';
  try {
    selected = await call('play_tracks_queued', { trackIds: [item.trackId], operationId }, 50_000);
    currentCaseContext.selectionSubmission = 'submitted';
  } catch {
    // Recover the named operation receipt once; never resubmit playback on timeout.
    currentCaseContext.stage = 'queue_receipt_recovery';
    selected = await call('get_playback_operation', { operationId }, 10_000).catch(() => ({ error: true, result: null }));
    receiptRecovered = !selected.error && Boolean(selected.result);
    if (receiptRecovered) currentCaseContext.selectionSubmission = 'submitted';
  }
  const receipt = selected.result?.receipt;
  const acceptedStatuses = ['accepted', 'playing', 'paused'];
  if (selected.error || !acceptedStatuses.includes(selected.result?.status)
      || !Array.isArray(receipt?.trackIds) || receipt.trackIds.length !== 1
      || receipt.trackIds[0] !== item.trackId || !safeId(receipt?.queueEntryIds?.[0])) {
    return { caseId: item.caseId, trackId: item.trackId, status: 'selection_failed', attempts: 1,
      operationId, receiptRecovered, matchingModeControl: 'unavailable through playback MCP; unchanged app preference applies',
      failureStage: currentCaseContext.stage,
      errorClass: selected.error ? 'tool_error' : 'selection_rejected_or_invalid_receipt',
      selectionSubmission: currentCaseContext.selectionSubmission,
      sourceIdEqualityGate: false, before: snapshot(before) };
  }
  const expectedEntryId = Array.isArray(receipt.queueEntryIds) ? safeId(receipt.queueEntryIds[0]) : null;
  const expectedGeneration = Number.isSafeInteger(receipt.acceptedGeneration) ? receipt.acceptedGeneration : null;
  const deadline = startedAt + deadlineMs;
  let firstPlayingAt = null;
  let firstPlayingPosition = null;
  let latest = null;
  let failure = null;
  let acceptedEntrySeen = false;
  currentCaseContext.stage = 'playback_state_poll';
  while (Date.now() < deadline) {
    latest = await playbackState(Math.max(1, deadline - Date.now()));
    const current = snapshot(latest);
    const ownsAcceptedEntry = matchesAcceptedEntry(latest, item.trackId, expectedEntryId);
    // The pre-acceptance snapshot may still show the previous track. Wait for the
    // receipt's exact occurrence before attributing status or progress to this case.
    if (ownsAcceptedEntry) acceptedEntrySeen = true;
    else if (acceptedEntrySeen && acceptedEntryOwnershipChanged(latest, item.trackId, expectedEntryId)) {
      failure = 'accepted_entry_ownership_changed';
      break;
    }
    if (current.status === 'playing' && ownsAcceptedEntry && current.positionSeconds !== null) {
      firstPlayingAt ??= Date.now();
      firstPlayingPosition ??= current.positionSeconds;
      const wallMs = Date.now() - firstPlayingAt;
      const advancedSeconds = current.positionSeconds - firstPlayingPosition;
      if (wallMs >= progressSeconds * 1000 && advancedSeconds >= progressSeconds - 0.25) break;
    } else if (ownsAcceptedEntry && ['failed', 'error', 'ended'].includes(current.status)) {
      failure = 'playback_failed';
      break;
    }
    await sleep(Math.min(400, Math.max(1, deadline - Date.now())));
  }
  const after = latest ? snapshot(latest) : null;
  const advancedSeconds = firstPlayingPosition !== null && after?.positionSeconds !== null
    ? after.positionSeconds - firstPlayingPosition : null;
  const completed = !failure && firstPlayingAt !== null && advancedSeconds >= progressSeconds - 0.25
    && Date.now() - firstPlayingAt >= progressSeconds * 1000;
  return {
    caseId: item.caseId,
    trackId: item.trackId,
    operationId,
    receiptRecovered,
    acceptedEntryId: expectedEntryId,
    acceptedGeneration: expectedGeneration,
    intendedMatchingMode: item.matchingMode,
    matchingModeControl: 'effective mode verified through read-only app status before selection',
    attempts: 1,
    selectionSubmission: 'submitted',
    status: completed ? 'five_second_progress_observed' : failure ?? 'deadline_without_five_second_progress',
    wallMs: Date.now() - startedAt,
    progressSeconds: advancedSeconds,
    sourceIdEqualityGate: false,
    previousSourceObservations: item.knownSourceObservations,
    before: snapshot(before),
    after,
  };
}

const results = [];
const report = {
  schemaVersion: 1,
  workload: 'one_attempt_per_saved_track',
  runStatus: 'in_progress',
  startedAt: new Date().toISOString(),
  attemptPolicy: manifest.attemptPolicy,
  results,
  missingCases: manifest.missing,
  limitations: [
    'Each valid case submits one play_tracks_queued operation with a caller UUID; a timeout recovers that operation receipt once and never resubmits playback.',
    'The effective mode is read from get_app_status before each case; there is no per-operation matching-mode parameter or setter.',
    'A changed source ID is recorded and is not by itself a failure; the runtime matcher may establish that two sources represent the same recording.',
    'Five seconds of playback-position progress is not physical-audibility or acoustic identity verification.',
    'The playback queue/current playback state is changed by selection; use only with a profile where that temporary runtime state is acceptable.',
  ],
};
async function checkpoint() {
  const temporaryFile = `${resolvedOutputFile}.tmp-${randomUUID()}`;
  try {
    await writeFile(temporaryFile, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600, flag: 'wx' });
    await rename(temporaryFile, resolvedOutputFile);
  } catch (error) {
    await unlink(temporaryFile).catch(() => {});
    throw error;
  }
}
let outputReserved = false;
try {
  // Reserve once; each subsequent completed case atomically replaces this file.
  await writeFile(resolvedOutputFile, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600, flag: 'wx' });
  outputReserved = true;
  await client.connect(transport, { timeout: 15_000 });
  for (const item of cases) {
    try { results.push(await runCase(item)); }
    catch (error) {
      const context = currentCaseContext ?? { stage: 'case_setup', selectionSubmission: 'not_submitted' };
      results.push({ caseId: item.caseId, trackId: item.trackId,
        attempts: context.selectionSubmission === 'not_submitted' ? 0 : 1,
        status: 'harness_or_state_error', failureStage: context.stage,
        errorClass: safeErrorClass(error), selectionSubmission: context.selectionSubmission,
        sourceIdEqualityGate: false });
    }
    await checkpoint();
  }
  report.runStatus = 'complete';
  report.completedAt = new Date().toISOString();
  await checkpoint();
} catch (error) {
  report.runStatus = 'interrupted_or_harness_error';
  report.failureStage = currentCaseContext?.stage ?? 'mcp_connect_or_checkpoint';
  report.errorClass = safeErrorClass(error);
  if (outputReserved) await checkpoint().catch(() => {});
  process.exitCode = 1;
} finally {
  await client.close().catch(() => {});
}
