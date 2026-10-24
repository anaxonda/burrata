// @vitest-environment jsdom
// @ts-expect-error Browser runtime under jsdom
import { mount, tick, unmount } from "../node_modules/svelte/src/index-client.js";
import { describe,it,expect,vi } from "vitest";
// @ts-expect-error Browser runtime under jsdom
vi.mock("svelte",async()=>await import("../node_modules/svelte/src/index-client.js"));
import LanguagePicker from "./LanguagePicker.svelte";
async function settle(){await tick();for(let i=0;i<8;i++)await Promise.resolve();await tick();}
describe("LanguagePicker",()=>{
  it("searches standard language names and selects a language code",async()=>{
    const target=document.createElement("div");document.body.append(target);const change=vi.fn(async()=>{});const app=mount(LanguagePicker,{target,props:{value:"en",onChange:change}});
    try {
      target.querySelector<HTMLButtonElement>('[aria-label="Translation language"]')!.click();await settle();
      const input=target.querySelector<HTMLInputElement>('input')!;input.value="bulgar";input.dispatchEvent(new Event("input",{bubbles:true}));await settle();
      const choices=target.querySelectorAll<HTMLButtonElement>('[role="option"]');expect(choices).toHaveLength(1);expect(choices[0]!.textContent).toContain("Bulgarian");choices[0]!.click();await settle();expect(change).toHaveBeenCalledWith("bg");expect(target.querySelector('[role="listbox"]')).toBeNull();
    } finally {await unmount(app);target.remove();}
  });
});
