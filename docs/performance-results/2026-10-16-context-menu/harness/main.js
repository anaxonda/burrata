import { mount } from "svelte";
import TrackMenu from "../../../../src/TrackMenu.svelte";
import PlaylistMenu from "../../../../src/PlaylistMenu.svelte";
import "../../../../src/style.css";

const app = document.querySelector("#app");
app.className = "app-shell";
const noop = () => {};
const track = {
  id: "visual-t1",
  provider: "spotify",
  providerId: "s1",
  sourceUrl: "https://open.spotify.com/track/s1",
  artist: "The Example Band",
  title: "A Long Example Track Title for Rendering",
  favorite: false,
  localPath: null,
  provenance: {},
};
const params = new URLSearchParams(location.search);
const playlist = params.get("menu") === "playlist";
const centered = params.has("centered");
const playerHeight = Number(params.get("playerHeight") || 90);
const component = playlist ? PlaylistMenu : TrackMenu;
const sharedProps = {
  anchor: centered ? null : { x: innerWidth - 16, y: innerHeight - 16 },
  playerHeight,
  onClose: noop,
};
const props = playlist
  ? {
      ...sharedProps,
      name: "A Long Collection Name for Rendering",
      kind: "playlist",
      onOpen: noop,
      onDelete: noop,
      onDownload: noop,
      onOpenSource: noop,
      onRename: noop,
      onExport: noop,
      onCopy: noop,
      onRefresh: noop,
    }
  : {
      ...sharedProps,
      track,
      onNext: noop,
      onQueue: noop,
      onPlaylist: noop,
      onFavorite: noop,
      onDownload: noop,
      onDetails: noop,
      onSelect: noop,
      onRadio: noop,
      onRemove: noop,
      onMoreInfo: noop,
      onOpenMpv: noop,
    };

mount(component, { target: app, props });

requestAnimationFrame(() =>
  requestAnimationFrame(() => {
    const dialog = document.querySelector("dialog");
    const card = dialog.getBoundingClientRect();
    const rows = [...dialog.querySelectorAll(":scope > button, :scope > .source-row")].map((row) => {
      const box = row.getBoundingClientRect();
      const label = row.cloneNode(true);
      label.querySelectorAll('[aria-hidden="true"]').forEach((icon) => icon.remove());
      return {
        label: label.textContent.trim().replace(/\s+/g, " "),
        height: Math.round(box.height),
        top: Math.round(box.top),
        bottom: Math.round(box.bottom),
      };
    });
    document.title = `METRICS:${JSON.stringify({
      viewport: { width: innerWidth, height: innerHeight },
      playerHeight,
      playerTop: innerHeight - playerHeight,
      centered,
      card: {
        x: Math.round(card.x),
        y: Math.round(card.y),
        width: Math.round(card.width),
        height: Math.round(card.height),
        scrollHeight: dialog.scrollHeight,
        clientHeight: dialog.clientHeight,
        bottom: Math.round(card.bottom),
        right: Math.round(card.right),
        overlapsPlayer: card.bottom > innerHeight - playerHeight,
        playerGap: Math.round(innerHeight - playerHeight - card.bottom),
      },
      activeElement: document.activeElement?.textContent?.trim().replace(/\s+/g, " "),
      rows,
    })}`;
  }),
);
