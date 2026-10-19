import {describe,it,expect} from "vitest";
import failureContracts from "./playback-failure-conformance.json";
import {appendTrackFailure, FIXED_RESOLVER_FAILURES, MAX_RECENT_TRACK_FAILURES, safeTrackFailureText, trackFailureCategory, shouldSkipFailedTrack, classifyPlaybackFailure, shouldSkipRecording} from "./playback-failure";
describe("automatic queue failure classification",()=>{
  it("recognizes exhausted recording failures before title words",()=>{
    for(const reason of ["No playable audio match was found for Network — Connection (Cancel Mix)","The resolved audio source identity does not match this recording","This source has no supported audio stream","Private video","Video has been removed"])
      expect(shouldSkipFailedTrack(reason),reason).toBe(true);
  });
  it("retains global failures and unknown errors even when unavailable appears",()=>{
    for(const reason of ["HTTP 403 unavailable", "HTTP 404 unavailable", "HTTP 410 unavailable", "Cancelled", "Network connection unavailable","HTTP 429 unavailable","HTTP status 503 unavailable","status code 502 unavailable","HTTP 500 unavailable","HTTP 504 unavailable","rate limit: unavailable","audio output device unavailable","20-second playback deadline","timeout: unavailable","temporarily unavailable","Unknown playback failure"])
      expect(shouldSkipFailedTrack(reason),reason).toBe(false);
  });
  it("classifies only the exact fixed resolver outcomes and skips each terminal recording failure",()=>{
    const contracts = [
      [FIXED_RESOLVER_FAILURES.noAudioResults, "no_audio_results"],
      [FIXED_RESOLVER_FAILURES.recordingIdentityUnverified, "recording_identity_unverified"],
      [FIXED_RESOLVER_FAILURES.missingDurationIdentityUnverified, "recording_identity_unverified"],
    ] as const;
    for (const [reason, category] of contracts) {
      expect(shouldSkipFailedTrack(reason), reason).toBe(true);
      expect(trackFailureCategory(reason), reason).toBe(category);
    }
    for (const nearMiss of [
      `${FIXED_RESOLVER_FAILURES.noAudioResults} extra`,
      FIXED_RESOLVER_FAILURES.noAudioResults.toLowerCase(),
      "No results: network connection unavailable",
    ]) {
      expect(trackFailureCategory(nearMiss), nearMiss).toBe("failed_with_reason");
      expect(shouldSkipFailedTrack(nearMiss), nearMiss).toBe(false);
    }
  });
});


describe("bounded queue failure history", () => {
  const track = {id:"same-recording",provider:"spotify",providerId:"same",title:"Exact recording",artist:"Artist",sourceUrl:"https://source.invalid/?token=secret",favorite:false,provenance:{}};
  it("retains the most recent 64 occurrences and independent retries", () => {
    let history: import("./types").PlaybackTrackFailure[] = [];
    for (let i=0;i<70;i++) history=appendTrackFailure(history,track,{queueRevision:"queue-a",generation:i,operationId:`op-${i}`,entryId:`entry-${i}`,outcome:"skipped"},"No playable audio match was found");
    expect(history).toHaveLength(MAX_RECENT_TRACK_FAILURES);
    expect(history[0]?.entryId).toBe("entry-6");
    expect(history[history.length-1]?.entryId).toBe("entry-69");
    history=appendTrackFailure(history,track,{queueRevision:"queue-a",generation:71,operationId:"retry",entryId:"entry-69",outcome:"queue_exhausted"},"HTTP 404 unavailable");
    expect(history.slice(-2).map(item=>item.entryId)).toEqual(["entry-69","entry-69"]);
    expect(history[history.length-1]?.category).toBe("failed_with_reason");
    expect(history[history.length-1]?.outcome).toBe("queue_exhausted");
  });
  it("redacts sources, paths, and credentials before storing the allowlisted DTO", () => {
    const reason='HTTP 404 https://private.invalid/?token=url-secret /home/private/music.flac C:\\private\\music.flac file:///tmp/secret token=token-secret cookie=cookie-secret api_key=api-secret Authorization: Bearer bearer-secret';
    const [failure]=appendTrackFailure([], {...track,title:reason}, {queueRevision:"queue-a",generation:1,operationId:"https://private.invalid/op",entryId:"entry-1",outcome:"skipped"},reason);
    const encoded=JSON.stringify(failure);
    for (const secret of ["private.invalid","url-secret","music.flac","token-secret","cookie-secret","api-secret","bearer-secret","sourceUrl"]) expect(encoded).not.toContain(secret);
    expect(failure?.operationId).toBeNull();
    expect(safeTrackFailureText("x".repeat(400))).toHaveLength(250);
  });
  it("categorizes only exact resolver outcomes and never infers no-match from a title", () => {
    expect(trackFailureCategory("No playable audio match was found for Network — Cancel Mix")).toBe("no_playable_match");
    expect(trackFailureCategory("Video unavailable: No playable audio match was found")).toBe("failed_with_reason");
    expect(trackFailureCategory("This source has no supported audio stream")).toBe("unsupported_audio_stream");
    expect(trackFailureCategory("The resolved audio source identity does not match this recording")).toBe("source_identity_mismatch");
  });
});


describe("typed playback failure contract", () => {
  it("matches the shared native policy fixtures", () => {
    for (const item of failureContracts) {
      expect(classifyPlaybackFailure(item.reason), item.reason).toEqual(item.failure);
      expect(shouldSkipFailedTrack(item.reason), item.reason).toBe(item.skip);
    }
  });
  it("uses typed detail before a legacy reason and never skips shared or source failures", () => {
    const source = classifyPlaybackFailure("HTTP 403 expired URL");
    expect(shouldSkipFailedTrack("Private video", source)).toBe(false);
    expect(shouldSkipRecording({...source, scope:"recording"})).toBe(false);
    const exhausted = classifyPlaybackFailure(FIXED_RESOLVER_FAILURES.noAudioResults);
    expect(shouldSkipFailedTrack("Unknown playback failure", exhausted)).toBe(true);
    for (const scope of ["global", "operation", "source", "unknown"] as const) expect(shouldSkipRecording({...exhausted, scope})).toBe(false);
    expect(shouldSkipRecording({...exhausted, retryable:true})).toBe(false);
  });
});
