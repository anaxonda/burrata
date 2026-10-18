import { describe, expect, it, vi } from "vitest";
import { ProviderReadCancellation } from "./provider-read-cancellation";

describe("ProviderReadCancellation", () => {
  it("cancels only read IDs owned by the caller and retires them after completion", async () => {
    let resolveFirst!: (value: unknown) => void;
    let resolveSecond!: (value: unknown) => void;
    const firstResponse = new Promise(resolve => { resolveFirst = resolve; });
    const secondResponse = new Promise(resolve => { resolveSecond = resolve; });
    const call = vi.fn((command: string, args?: Record<string, unknown>) => {
      if (command === "cancel_remote_read") return Promise.resolve(true);
      return args?.query === "first" ? firstResponse : secondResponse;
    });
    const firstOwner = new ProviderReadCancellation(call);
    const secondOwner = new ProviderReadCancellation(call);
    const first = firstOwner.read("search_source", { query: "first" });
    const second = secondOwner.read("search_source", { query: "second" });
    await Promise.resolve();
    const firstId = call.mock.calls[0]?.[1]?.requestId;
    const secondId = call.mock.calls[1]?.[1]?.requestId;
    expect(firstId).toEqual(expect.any(String));
    expect(secondId).toEqual(expect.any(String));

    firstOwner.cancelAll();
    expect(call).toHaveBeenCalledWith("cancel_remote_read", { requestId: firstId });
    expect(call).not.toHaveBeenCalledWith("cancel_remote_read", { requestId: secondId });
    resolveFirst({ ok: true });
    resolveSecond({ ok: true });
    await Promise.all([first, second]);
    firstOwner.cancelAll();
    expect(call.mock.calls.filter(([command]) => command === "cancel_remote_read")).toHaveLength(1);
  });
});
