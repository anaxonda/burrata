import {describe,expect,it} from "vitest";
import {alternativeSourceNotice} from "./alternative-source-notice";
const matching = (kind?:string) => ({assessment:{relationship:"alternative",action:"accept",alternativeKind:kind},resolvedRecording:{title:"Song (Live)",artist:"Source-reported artist",sourceId:"youtube:123"}});
describe("alternative source notice", () => {
  it.each([["release","Different release"],["performance","Other performance"],["remix","Other remix or edit"],["otherArtist","Other artist"]])("labels %s", (kind,label) => {
    expect(alternativeSourceNotice(matching(kind),"Song","Artist")).toEqual({label,performer:null,requested:"Song · Artist",playing:"Song (Live)"});
  });
  it("uses only an explicitly verified performer, never uploader or source-reported artist", () => {
    const input = matching("otherArtist");
    expect(alternativeSourceNotice({...input,resolvedRecording:{...input.resolvedRecording,verifiedPerformer:"Singer",uploader:"Channel"}},"Song")?.performer).toBe("Singer");
    expect(alternativeSourceNotice(input)?.performer).toBeNull();
  });
  it("retains a generic label for older accepted alternatives and hides unaccepted records", () => {
    expect(alternativeSourceNotice(matching())?.label).toBe("Alternative recording");
    expect(alternativeSourceNotice({assessment:{relationship:"alternative",action:"inspect"},resolvedRecording:{title:"Song"}})).toBeNull();
    expect(alternativeSourceNotice(null)).toBeNull();
  });
});
