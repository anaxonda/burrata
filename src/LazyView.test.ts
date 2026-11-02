// @vitest-environment jsdom
// @ts-expect-error Use Svelte's browser runtime for direct component mounting.
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { afterEach, describe, expect, it, vi } from "vitest";
import LazyView from "./LazyView.svelte";
import LazyViewHarness from "./LazyViewHarness.svelte";

let component: ReturnType<typeof mount> | undefined;
afterEach(() => {
  if (component) unmount(component);
  component = undefined;
  document.body.innerHTML = "";
});

describe("LazyView", () => {
  async function until(predicate: () => boolean) {
    for (let pass = 0; pass < 20 && !predicate(); pass++) {
      await Promise.resolve();
      await new Promise(resolve => setTimeout(resolve, 0));
      await tick();
    }
  }
  it("shows loading and renders the resolved view with props", async () => {
    let resolve!: (component: typeof import("./LazyView.fixture.svelte").default) => void;
    const loader = vi.fn(() => new Promise<typeof import("./LazyView.fixture.svelte").default>(done => { resolve = done; }));
    component = mount(LazyView, { target: document.body, props: { loader, componentProps: { title: "Loaded view" }, label: "settings" } });

    await until(() => typeof resolve === "function");
    expect(document.body.textContent).toContain("Loading settings");
    resolve((await import("./LazyView.fixture.svelte")).default);
    await until(() => document.body.textContent?.includes("Loaded view") === true);
    expect(document.body.textContent).toContain("Loaded view");
  });

  it("offers retry after a failed load", async () => {
    const loader = vi.fn()
      .mockRejectedValueOnce(new Error("chunk unavailable"))
      .mockImplementationOnce(() => import("./LazyView.fixture.svelte").then(module => module.default));
    component = mount(LazyView, { target: document.body, props: { loader, componentProps: { title: "Retry loaded" }, label: "backup" } });
    await until(() => Boolean(document.body.querySelector(".lazy-view-error button")));

    const retry = document.body.querySelector<HTMLButtonElement>("button");
    expect(retry?.textContent).toBe("Retry");
    retry?.click();
    await until(() => document.body.textContent?.includes("Retry loaded") === true);
    expect(document.body.textContent).toContain("Retry loaded");
    expect(loader).toHaveBeenCalledTimes(2);
  });

  it("keeps the loaded child instance when parent props and an inline loader change", async () => {
    component = mount(LazyViewHarness, { target: document.body });
    await until(() => document.body.textContent?.includes("Before update") === true);
    document.body.querySelectorAll<HTMLButtonElement>("button")[1]?.click();
    await tick();
    expect(document.body.textContent).toContain("Child clicks: 1");

    document.body.querySelectorAll<HTMLButtonElement>("button")[0]?.click();
    await tick();
    await tick();
    expect(document.body.textContent).toContain("After update");
    expect(document.body.textContent).toContain("Child clicks: 1");
    expect(document.querySelector('[data-testid="load-count"]')?.textContent).toBe("1");
  });
});
