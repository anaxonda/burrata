import app from "../App.svelte?raw";
import rows from "../DownloadWatches.svelte?raw";
import manifest from "../../src-tauri/build.rs?raw";
import handlers from "../../src-tauri/src/lib.rs?raw";
import capability from "../../src-tauri/capabilities/main.json";
import {expect,it} from "vitest";
it("permits the desktop commands used by waiting-download UI through every Tauri registration layer",()=>{
  const used=[...new Set([...`${app}\n${rows}`.matchAll(/"(\w*download_watch\w*)"/g)].map(match=>match[1]))];
  expect(used.sort()).toEqual(["control_download_watch","list_download_watches","queue_download_watch"]);
  for(const command of used){expect(manifest).toContain(`"${command}"`);expect(handlers).toContain(`fn ${command}(`);expect(capability.permissions).toContain(`allow-${command.replaceAll("_","-")}`);}
});
