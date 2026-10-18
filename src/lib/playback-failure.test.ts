import {describe,it,expect} from "vitest";
import {appendTrackFailure, MAX_RECENT_TRACK_FAILURES, safeTrackFailureText, trackFailureCategory, shouldSkipFailedTrack} from "./playback-failure";
describe("automatic queue failure classification",()=>{
  it("recognizes exhausted recording failures before title words",()=>{
    for(const reason of ["No playable audio match was found for Network — Connection (Cancel Mix)","The resolved audio source identity does not match this recording","This source has no supported audio stream","Private video","Video has been removed","HTTP 404 unavailable"])
      expect(shouldSkipFailedTrack(reason),reason).toBe(true);
  });
  it("retains global failures and unknown errors even when unavailable appears",()=>{
    for(const reason of ["Cancelled", "Network connection unavailable","HTTP 429 unavailable","HTTP status 503 unavailable","status code 502 unavailable","HTTP 500 unavailable","HTTP 504 unavailable","rate limit: unavailable","audio output device unavailable","20-second playback deadline","timeout: unavailable","temporarily unavailable","Unknown playback failure"])
      expect(shouldSkipFailedTrack(reason),reason).toBe(false);
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
