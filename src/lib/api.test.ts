import { afterEach, describe, expect, it, vi } from "vitest";
import { call, desktop, listen, playable } from "./api";
afterEach(() => vi.unstubAllGlobals());
describe("supported Tauri global bridge", () => {
  it("fails clearly in a browser without attempting native operations", async () => {
    vi.stubGlobal("window", {});
    expect(desktop()).toBe(false);
    await expect(call("list_assets")).rejects.toThrow("desktop application");
    await expect(listen("download-progress", () => {})).rejects.toThrow(
      "Desktop event connection",
    );
  });
  it("preserves structured command arguments and return values", async () => {
    const invoke = vi.fn().mockResolvedValue([{ id: "a" }]);
    vi.stubGlobal("window", { __TAURI__: { core: { invoke } } });
    const args = {
      providerId: "beatport",
      url: "https://www.beatport.com/genre/house/5/top-100",
      refresh: true,
    };
    expect(await call("get_chart", args)).toEqual([{ id: "a" }]);
    expect(invoke).toHaveBeenCalledWith("get_chart", args);
  });
  it("returns event cleanup and forwards progress payloads", async () => {
    const cleanup = vi.fn(),
      handler = vi.fn();
    const bridge = vi.fn(
      async (_event: string, callback: (e: unknown) => void) => {
        callback({
          event: "download-progress",
          id: 1,
          payload: { state: "completed" },
        });
        return cleanup;
      },
    );
    vi.stubGlobal("window", { __TAURI__: { event: { listen: bridge } } });
    const dispose = await listen("download-progress", handler);
    dispose();
    expect(handler).toHaveBeenCalledWith({
      event: "download-progress",
      id: 1,
      payload: { state: "completed" },
    });
    expect(cleanup).toHaveBeenCalledOnce();
  });
  it("converts local paths through the supported asset API and keeps remote previews intact", () => {
    const convertFileSrc = vi
      .fn()
      .mockReturnValue("asset://localhost/music.mp3");
    vi.stubGlobal("window", { __TAURI__: { core: { convertFileSrc } } });
    expect(playable("/music.mp3")).toBe("asset://localhost/music.mp3");
    expect(convertFileSrc).toHaveBeenCalledWith("/music.mp3");
    expect(playable("https://example.com/preview.mp3")).toBe(
      "https://example.com/preview.mp3",
    );
    expect(convertFileSrc).toHaveBeenCalledOnce();
  });
});
