import { describe, expect, it } from "vitest";
import { LatestRequest } from "./latest-request";
describe("rapid preview switching", () => {
  it("keeps the latest preview when the previous network response finishes last", async () => {
    const player = new LatestRequest();
    let finishOld!: (url: string) => void;
    const old = player.resolve(
      player.next(),
      new Promise<string>((resolve) => (finishOld = resolve)),
    );
    const current = player.resolve(
      player.next(),
      Promise.resolve("new-preview.mp3"),
    );
    expect(await current).toBe("new-preview.mp3");
    finishOld("old-preview.mp3");
    expect(await old).toBeUndefined();
  });
  it("suppresses an obsolete source failure while preserving actionable current errors", async () => {
    const player = new LatestRequest();
    let failOld!: (reason: Error) => void;
    const old = player.resolve(
      player.next(),
      new Promise<string>((_, reject) => (failOld = reject)),
    );
    const token = player.next();
    failOld(new Error("Old source offline"));
    expect(await old).toBeUndefined();
    await expect(
      player.resolve(
        token,
        Promise.reject(new Error("Current source offline")),
      ),
    ).rejects.toThrow("Current source offline");
  });
  it("cleans up an obsolete result using its own session identity", async () => {
    const player = new LatestRequest();
    let finishOld!: (session: string) => void;
    const stopped: string[] = [];
    const old = player.resolve(
      player.next(),
      new Promise<string>((resolve) => (finishOld = resolve)),
      (session) => stopped.push(session),
    );
    player.next();
    finishOld("old-session");
    expect(await old).toBeUndefined();
    expect(stopped).toEqual(["old-session"]);
  });
});
