import { mount } from "svelte";
import TrackList from "../../../../src/TrackList.svelte";
import "../../../../src/style.css";

const app = document.querySelector("#app");
app.className = "app-shell";
app.style.gridTemplateRows = innerWidth <= 767 ? "0 minmax(0, 1fr) 90px" : "minmax(0, 1fr) 90px";
app.style.setProperty("--player-height", "90px");
const noop = () => {};
const track = {
  id: "visual-tracklist-1",
  provider: "spotify",
  providerId: "s1",
  sourceUrl: "https://open.spotify.com/track/s1",
  artist: "The Example Band",
  title: "A Long Example Track Title for Rendering",
  album: "Visual Fixture",
  favorite: false,
  localPath: null,
  provenance: {},
};
app.innerHTML = '<aside aria-hidden="true"></aside><main><section class="tracklist-fixture"></section></main><footer aria-hidden="true"></footer>';
app.querySelector("footer").style.gridColumn = "1 / -1";
const target = app.querySelector(".tracklist-fixture");
target.style.cssText = "padding:180px 24px 0;";
mount(TrackList, {
  target,
  props: {
    tracks: [track],
    onPlay: noop,
    onDownload: noop,
    onFavorite: noop,
    onNext: noop,
    onQueue: noop,
    onPlaylist: noop,
    onArtist: noop,
    onInfo: noop,
    onContext: noop,
  },
});

setTimeout(() => {
const details = document.querySelector("details.more");
details.querySelector("summary").click();
const menu = details.querySelector(".more-menu");
requestAnimationFrame(() => requestAnimationFrame(() => {
    const box = menu.getBoundingClientRect();
    const rows = [...menu.querySelectorAll("button, a")].map((row) => {
      const rect = row.getBoundingClientRect();
      const label = row.cloneNode(true);
      label.querySelectorAll('[aria-hidden="true"]').forEach((icon) => icon.remove());
      return { label: label.textContent.trim().replace(/\s+/g, " "), height: Math.round(rect.height), top: Math.round(rect.top), bottom: Math.round(rect.bottom) };
    });
document.title = `METRICS:${JSON.stringify({
      viewport: { width: innerWidth, height: innerHeight },
      menu: { x: Math.round(box.x), y: Math.round(box.y), width: Math.round(box.width), height: Math.round(box.height), bottom: Math.round(box.bottom), right: Math.round(box.right), scrollHeight: menu.scrollHeight },
      footerTop: document.querySelector("footer").getBoundingClientRect().top,
      documentScrollWidth: document.documentElement.scrollWidth,
      rows,
})}`;
}));
}, 500);
