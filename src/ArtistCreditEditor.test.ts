// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("svelte", async () => {
  // @ts-expect-error Match the browser runtime used by the direct import above.
  return await import("../node_modules/svelte/src/index-client.js");
});
import ArtistCreditEditor from "./ArtistCreditEditor.svelte";

let cleanup: (() => void) | undefined;
afterEach(() => { cleanup?.(); cleanup = undefined; document.body.innerHTML = ""; vi.restoreAllMocks(); });
async function settle() { await Promise.resolve(); await tick(); }
function render(props: Record<string, unknown>) {
  const target = document.createElement("div"); document.body.append(target);
  const instance = mount(ArtistCreditEditor, { target, props: props as never });
  cleanup = () => { void unmount(instance); };
  return target;
}

describe("ArtistCreditEditor", () => {
  it("keeps a combined punctuation name atomic and returns separate editable rows", async () => {
    const onSave = vi.fn();
    const target = render({
      credits: [{ name: "&friends and Samuel Cosmic", role: "artist" }],
      onSave,
    });

    expect(target.querySelectorAll(".credit-row")).toHaveLength(1);
    expect((target.querySelector('[aria-label="Artist name 1"]') as HTMLInputElement).value).toBe("&friends and Samuel Cosmic");

    (target.querySelector('[aria-label="Artist name 1"]') as HTMLInputElement).value = "&friends";
    target.querySelector('[aria-label="Artist name 1"]')!.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector('[aria-label="Artist role 1"]')!.dispatchEvent(new Event("input", { bubbles: true }));
    target.querySelector<HTMLButtonElement>(".secondary")!.click();
    await settle();

    const secondName = target.querySelector<HTMLInputElement>('[aria-label="Artist name 2"]')!;
    secondName.value = "Samuel Cosmic";
    secondName.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    target.querySelector<HTMLButtonElement>(".primary")!.click();
    await settle();
    expect(onSave).toHaveBeenCalledWith([
      { name: "&friends", role: "artist" },
      { name: "Samuel Cosmic", role: "artist" },
    ]);
  });

  it("requires nonempty names and roles and caps the list at 32", async () => {
    const target = render({ credits: [{ name: "  ", role: "artist" }], onSave: vi.fn() });
    expect(target.querySelector<HTMLButtonElement>(".primary")!.disabled).toBe(true);
    expect(target.querySelector('[role="status"]')?.textContent).toContain("needs an artist name and role");

    const input = target.querySelector<HTMLInputElement>('[aria-label="Artist name 1"]')!;
    input.value = "Valid Artist";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    for (let count = 1; count < 32; count++) target.querySelector<HTMLButtonElement>(".secondary")!.click();
    await settle();
    expect(target.querySelectorAll(".credit-row")).toHaveLength(32);
    expect(target.querySelector<HTMLButtonElement>(".secondary")!.disabled).toBe(true);
    expect(target.querySelector(".hint")?.textContent).toContain("up to 32");
  });

  it("retains edits and reports callback errors without closing", async () => {
    const onSave = vi.fn().mockRejectedValue(new Error("Could not save"));
    const onCancel = vi.fn();
    const target = render({ credits: [{ name: "One", role: "artist" }], onSave, onCancel });
    target.querySelector<HTMLButtonElement>(".primary")!.click();
    await settle();
    expect(target.querySelector('[role="alert"]')?.textContent).toBe("Could not save");
    expect(target.querySelector('[aria-label="Artist name 1"]')).not.toBeNull();
    target.querySelector<HTMLButtonElement>("header button")!.click();
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("offers a non-destructive reset callback and reports reset failures", async () => {
    const onReset = vi.fn().mockRejectedValueOnce(new Error("Reset unavailable")).mockResolvedValueOnce(undefined);
    const target = render({ credits: [{ name: "Artist", role: "artist" }], onSave: vi.fn(), onReset });
    const clear = [...target.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.includes("Clear manual correction"))!;
    expect(clear).not.toBeUndefined();
    clear.click(); await settle();
    expect(target.querySelector('[role="alert"]')?.textContent).toBe("Reset unavailable");
    clear.click(); await settle();
    expect(onReset).toHaveBeenCalledTimes(2);
    expect(target.querySelector('[role="status"]')?.textContent).toBe("Manual correction cleared.");
  });
});
