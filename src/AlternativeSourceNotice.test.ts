// @vitest-environment jsdom
// @ts-expect-error Svelte browser runtime
import {mount,tick,unmount} from "../node_modules/svelte/src/index-client.js";
import {describe,it,expect,vi} from "vitest";
// @ts-expect-error Svelte internal runtime
vi.mock("svelte", async () => await import("../node_modules/svelte/src/index-client.js"));
import AlternativeSourceNotice from "./AlternativeSourceNotice.svelte";
describe("AlternativeSourceNotice", () => {
  it("shows a concise label and an accessible explanation", async () => {
    const target=document.createElement("div");document.body.append(target);
    const component=mount(AlternativeSourceNotice,{target,props:{matching:{assessment:{relationship:"alternative",action:"accept",alternativeKind:"remix"},resolvedRecording:{title:"Song remix"}},requestedTitle:"Song",requestedArtist:"Artist"}}); await tick();
    expect(target.textContent).toContain("Other remix or edit");
    expect(target.querySelector('[aria-label="About this alternative"]')).not.toBeNull();
    expect(target.textContent).toContain("Your library details are unchanged");
    unmount(component);target.remove();
  });
});
