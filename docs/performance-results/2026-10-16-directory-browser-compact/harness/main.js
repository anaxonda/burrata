import { mount } from "svelte";
import DirectoryBrowser from "../../../../src/DirectoryBrowser.svelte";
import "../../../../src/style.css";
import "../../../../src/desktop-layout.css";

const host = document.querySelector("#app");
host.className = "app-shell";
host.style.cssText = "--library-sidebar-width:246px;--player-height:90px;";
host.innerHTML = `
  <aside class="sidebar"><h2>burrata</h2><nav>Folders · dj</nav></aside>
  <main><header><div class="navigation-controls"><button>←</button><button>→</button></div><div><h1>Folders</h1></div><div class="header-actions"></div></header><div id="browser-host"></div></main>
  <footer class="player"><div class="now-playing"><strong>Example · Playing</strong></div><span class="transport">▶</span></footer>`;
const root = { id: "fixture-root", name: "Music", path: "/music" };
const files = Array.from({ length: 15 }, (_, index) => {
  const track = { id: `track-${index}`, provider: "local", providerId: `asset-${index}`, sourceUrl: `local:asset-${index}`, title: `Example Track ${String(index + 1).padStart(2, "0")}`, artist: "Example Artist", album: "Synthetic Album", durationSeconds: 180 + index, favorite: false, localPath: `/music/Example Track ${index + 1}.flac`, assetId: `asset-${index}`, provenance: {} };
  return { path: track.localPath, name: `Example Track ${String(index + 1).padStart(2, "0")}.flac`, assetId: track.assetId, track };
});
async function call(command, args = {}) {
  if (command === "list_directory_browser_roots") return [root];
  if (command === "browse_directory_browser") return { rootId: root.id, path: root.path, relativePath: args.relativePath ?? "", breadcrumbs: [{ name: "Music", relativePath: "" }, { name: "dj", relativePath: "dj" }], directories: [], files, total: files.length, offset: args.offset ?? 0, limit: args.limit ?? 200 };
  throw new Error(`Unexpected fixture call: ${command}`);
}
mount(DirectoryBrowser, { target: document.querySelector("#browser-host"), props: { call, location: { rootId: root.id, relativePath: "dj" }, onAddSelectedToPlaylist: () => {} } });
const params = new URLSearchParams(location.search);
setTimeout(() => {
if (params.has("duration")) document.querySelector(".directory-duration-filters")?.setAttribute("open", "");
if (params.has("metrics")) requestAnimationFrame(() => requestAnimationFrame(() => {
  const card = document.querySelector(".directory-browser");
  const first = document.querySelector(".shared-track-list li:not(.virtual-spacer)");
  const controls = [...document.querySelectorAll(".directory-view-tools > input,.directory-view-tools > label select,.directory-view-tools > button,.directory-view-tools > details > summary,.directory-view-tools > span")].map(el => {
    const r = el.getBoundingClientRect(); return { name: el.getAttribute("aria-label") || el.textContent.trim(), x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height), right: Math.round(r.right), bottom: Math.round(r.bottom) };
  });
  const firstRect = first?.getBoundingClientRect();
  const info = { viewport: { width: innerWidth, height: innerHeight }, firstTrackTop: firstRect ? Math.round(firstRect.top) : null, card: { x: Math.round(card.getBoundingClientRect().x), y: Math.round(card.getBoundingClientRect().y), width: Math.round(card.getBoundingClientRect().width), right: Math.round(card.getBoundingClientRect().right), scrollWidth: card.scrollWidth, clientWidth: card.clientWidth }, document: { width: document.documentElement.scrollWidth, bodyWidth: document.body.scrollWidth, horizontalOverflow: document.documentElement.scrollWidth > innerWidth || document.body.scrollWidth > innerWidth }, controls, selectionToolbar: (() => { const el = document.querySelector(".playlist-selection-actions"); const r=el.getBoundingClientRect();return {y:Math.round(r.y),height:Math.round(r.height)};})(), firstRowLabel: document.querySelector(".shared-track-list")?.getAttribute("aria-label"), oldHeadingPresent: Boolean(document.querySelector(".directory-list-heading")), fileCount: document.querySelector(".directory-file-count")?.textContent.trim() ?? null, durationPopover: (() => { const el=document.querySelector(".directory-duration-filters > div"); const r=el.getBoundingClientRect(); return {open:document.querySelector(".directory-duration-filters").open,x:Math.round(r.x),y:Math.round(r.y),right:Math.round(r.right),bottom:Math.round(r.bottom),width:Math.round(r.width),height:Math.round(r.height),overflowRight:r.right>innerWidth}; })() };
  document.title = `METRICS:${JSON.stringify(info)}`;
}));
}, 50);
