// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
// Match the client-runtime mock used by mounted Svelte component tests in this repo.
// @ts-expect-error Svelte internal client entry has no public declaration.
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import VirtualResultsTestHost from "./VirtualResultsTestHost.svelte";

let cleanup: (() => void) | undefined;
afterEach(() => { cleanup?.(); cleanup = undefined; document.body.innerHTML = ""; });

describe("VirtualResults alternate scroll roots", () => {
  it("bounds jobs, then scrolls to the same job identity and keeps its action usable", async () => {
    const target = document.createElement("div");
    document.body.append(target);
    const instance = mount(VirtualResultsTestHost, { target, props: { count: 500 } });
    cleanup = () => unmount(instance);
    await tick();
    const queue = target.querySelector<HTMLElement>(".queue")!;
    const host = target.querySelector<HTMLElement>(".virtual-results")!;
    Object.defineProperty(queue, "clientHeight", { configurable: true, value: 100 });
    Object.defineProperty(queue, "clientWidth", { configurable: true, value: 500 });
    Object.defineProperty(host, "clientWidth", { configurable: true, value: 500 });
    queue.getBoundingClientRect = () => ({ top: 0, bottom: 100, left: 0, right: 500, width: 500, height: 100, x: 0, y: 0, toJSON() {} });
    host.getBoundingClientRect = () => ({ top: -queue.scrollTop, bottom: 20_000 - queue.scrollTop, left: 0, right: 500, width: 500, height: 20_000, x: 0, y: -queue.scrollTop, toJSON() {} });
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(target.querySelectorAll("[data-row]").length).toBeLessThan(40);
    expect(target.querySelector("[data-row='0']")).not.toBeNull();

    queue.scrollTop = 10_000;
    queue.dispatchEvent(new Event("scroll"));
    await new Promise(resolve => setTimeout(resolve, 30));
    await tick();
    const laterJob = target.querySelector<HTMLElement>("[data-row='250']");
    expect(laterJob?.getAttribute("data-job-id")).toBe("job-250");
    const action = laterJob?.querySelector<HTMLButtonElement>("button");
    action?.click();
    await tick();
    expect(target.querySelector("[data-selected]")?.textContent).toBe("job-250");
  });
});
