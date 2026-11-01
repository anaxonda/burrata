import {expect,it} from "vitest";
import {loudnessMeasurementSourceLabel} from "./loudness-normalization";
it("labels measured audio and trusted tag sources",()=>{
  expect(loudnessMeasurementSourceLabel("ffmpeg")).toBe("Audio measurement");
  expect(loudnessMeasurementSourceLabel("replayGainTrack")).toBe("ReplayGain track tag");
  expect(loudnessMeasurementSourceLabel("opusR128Album")).toBe("Opus album tag");
  expect(loudnessMeasurementSourceLabel("unknown")).toBe("Audio measurement");
});
