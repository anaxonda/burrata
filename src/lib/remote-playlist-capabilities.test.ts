import { describe, expect, it } from "vitest";
import { playlistWriteCapabilities, playlistWritePlanAllowed } from "./remote-playlist-capabilities";
describe("playlist write capabilities", () => {
  const addOnly = { add: true, remove: false, reorder: false };
  const plan = { add: [{}], remove: [], move: [] };
  it("retains legacy broad permission only when the typed field is missing", () => {
    expect(playlistWriteCapabilities(true, undefined)).toEqual({ add: true, remove: true, reorder: true });
    for (const malformed of [null, true, {}, { ...addOnly, extra: true }, { ...addOnly, remove: "false" }]) {
      expect(playlistWriteCapabilities(true, malformed)).toEqual({ add: false, remove: false, reorder: false });
    }
  });
  it("permits additions without granting remove or reorder", () => {
    expect(playlistWritePlanAllowed(false, addOnly, plan)).toBe(true);
    expect(playlistWritePlanAllowed(true, addOnly, { ...plan, remove: [{}] })).toBe(false);
    expect(playlistWritePlanAllowed(true, addOnly, { ...plan, move: [{}] })).toBe(false);
  });
});
