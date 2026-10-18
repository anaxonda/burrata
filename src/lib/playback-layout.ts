export type PlaybackLayoutRole =
  | "footer"
  | "now_playing"
  | "transport"
  | "scrubber"
  | "right"
  | "quality"
  | "error"
  | "queue_panel"
  | "queue_header"
  | "queue_title"
  | "identify"
  | "queue_close"
  | "panel_close";

export type PlaybackLayoutElement = {
  role: PlaybackLayoutRole;
  present: boolean;
  visible: boolean;
  x: number;
  y: number;
  width: number;
  height: number;
  clientWidth: number;
  scrollWidth: number;
};

export type PlaybackLayoutCapture = {
  viewport: { width: number; height: number };
  footerHeight: number;
  elements: PlaybackLayoutElement[];
  trackListsCounts: TrackListCount[];
};

type MeasurableElement = Pick<Element, "getBoundingClientRect" | "clientWidth" | "scrollWidth" | "getClientRects">;
type LayoutRoot = Pick<ParentNode, "querySelector" | "querySelectorAll">;

const TARGETS: ReadonlyArray<[PlaybackLayoutRole, string]> = [
  ["footer", "footer.player"],
  ["now_playing", "footer.player > .now-playing"],
  ["transport", "footer.player .player-transport"],
  ["scrubber", "footer.player .player-scrubber"],
  ["right", "footer.player .player-right"],
  ["quality", "footer.player .now-playing .playback-quality"],
  ["error", "footer.player .player-error[role='alert']"],
  ["queue_panel", "aside.playback-queue-panel"],
  ["queue_header", "aside.playback-queue-panel > header.queue-header"],
  ["queue_title", "aside.playback-queue-panel > header.queue-header h2"],
  ["identify", "aside.playback-queue-panel > header.queue-header button.identify"],
  ["queue_close", "aside.playback-queue-panel > header.queue-header button.close"],
  ["panel_close", ".player-panel-tabs button[aria-label='Close player panel']"],
];

const rounded = (value: number) => Math.round(value * 100) / 100;

function measureElement(
  root: LayoutRoot,
  role: PlaybackLayoutRole,
  selector: string,
  styleFor: (element: Element) => Pick<CSSStyleDeclaration, "display" | "visibility" | "opacity">,
): PlaybackLayoutElement {
  const element = root.querySelector(selector) as MeasurableElement | null;
  if (!element) {
    return { role, present: false, visible: false, x: 0, y: 0, width: 0, height: 0, clientWidth: 0, scrollWidth: 0 };
  }
  const rect = element.getBoundingClientRect();
  let effectiveOpacity = 1;
  let cssVisible = true;
  for (let ancestor: Element | null = element as Element; ancestor; ancestor = ancestor.parentElement) {
    const style = styleFor(ancestor);
    const opacity = Number(style.opacity || "1");
    if (style.display === "none" || style.visibility === "hidden" || style.visibility === "collapse" || opacity <= 0) {
      cssVisible = false;
      break;
    }
    effectiveOpacity *= opacity;
  }
  const visible = cssVisible && effectiveOpacity > 0 && element.getClientRects().length > 0 && rect.width > 0 && rect.height > 0;
  return {
    role,
    present: true,
    visible,
    x: rounded(rect.left),
    y: rounded(rect.top),
    width: rounded(rect.width),
    height: rounded(rect.height),
    clientWidth: Math.max(0, Math.round(element.clientWidth)),
    scrollWidth: Math.max(0, Math.round(element.scrollWidth)),
  };
}

/** Capture only allow-listed player controls and CSS geometry. No text, track data, or URLs are read. */
export function measurePlaybackLayout(
  root: LayoutRoot,
  viewport: { width: number; height: number },
  styleFor: (element: Element) => Pick<CSSStyleDeclaration, "display" | "visibility" | "opacity"> = (element) => window.getComputedStyle(element),
): PlaybackLayoutCapture {
  const elements = TARGETS.map(([role, selector]) => measureElement(root, role, selector, styleFor));
  const footer = elements.find((element) => element.role === "footer")!;
  return {
    viewport: { width: Math.max(0, Math.round(viewport.width)), height: Math.max(0, Math.round(viewport.height)) },
    footerHeight: footer.present ? footer.height : 0,
    elements,
    trackListsCounts: measureTrackListCounts(root, viewport, styleFor),
  };
}

export type TrackListRole = "search" | "library" | "history" | "artist" | "remote";
export type TrackListCount = {
  role: TrackListRole;
  visibleRows: number;
  playing: number;
  paused: number;
  buffering: number;
};

const TRACK_LIST_TARGETS: ReadonlyArray<{ role: TrackListRole; selector: string; rowSelector: string; table: boolean }> = [
  { role: "search", selector: ".search-panel .source-results ol.shared-track-list", rowSelector: ":scope > li:not(.virtual-spacer)", table: false },
  { role: "library", selector: ".track-area .track-scroll.desktop-track-list", rowSelector: "table > tbody > tr:not(.spacer)", table: true },
  { role: "history", selector: ".history-page ol.shared-track-list", rowSelector: ":scope > li:not(.virtual-spacer)", table: false },
  { role: "artist", selector: ".artist-page-host ol.shared-track-list", rowSelector: ":scope > li:not(.virtual-spacer)", table: false },
  { role: "remote", selector: ".remote-playlist-detail ol.shared-track-list", rowSelector: ":scope > li:not(.virtual-spacer)", table: false },
];

const emptyTrackListCount = (role: TrackListRole): TrackListCount => ({ role, visibleRows: 0, playing: 0, paused: 0, buffering: 0 });

function intersects(a: DOMRect, b: { left: number; top: number; right: number; bottom: number }): boolean {
  return a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
}

function isTrackRowVisible(
  row: Element,
  clip: Element,
  viewport: { width: number; height: number },
  styleFor: (element: Element) => Pick<CSSStyleDeclaration, "display" | "visibility" | "opacity">,
): boolean {
  const rect = row.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return false;
  let left = Math.max(rect.left, 0), top = Math.max(rect.top, 0);
  let right = Math.min(rect.right, viewport.width), bottom = Math.min(rect.bottom, viewport.height);
  for (let ancestor: Element | null = row; ancestor; ancestor = ancestor.parentElement) {
    const style = styleFor(ancestor);
    if (style.display === "none" || style.visibility === "hidden" || style.visibility === "collapse" || Number(style.opacity || "1") <= 0) return false;
    if (ancestor !== row) {
      const overflow = style as Partial<CSSStyleDeclaration>;
      const bounds = ancestor.getBoundingClientRect();
      if (/^(hidden|clip|auto|scroll)$/.test(overflow.overflowX ?? overflow.overflow ?? "")) {
        left = Math.max(left, bounds.left); right = Math.min(right, bounds.right);
      }
      if (/^(hidden|clip|auto|scroll)$/.test(overflow.overflowY ?? overflow.overflow ?? "")) {
        top = Math.max(top, bounds.top); bottom = Math.min(bottom, bounds.bottom);
      }
    }
  }
  if (row.getClientRects().length === 0) return false;
  const clipRect = clip.getBoundingClientRect();
  return left < right && top < bottom && intersects(rect, clipRect);
}

/** Count visible track states in fixed UI regions. Reads only row classes/ARIA state and geometry; never reads labels or track fields. */
export function measureTrackListCounts(
  root: LayoutRoot,
  viewport: { width: number; height: number },
  styleFor: (element: Element) => Pick<CSSStyleDeclaration, "display" | "visibility" | "opacity"> = (element) => window.getComputedStyle(element),
): TrackListCount[] {
  return TRACK_LIST_TARGETS.map(({ role, selector, rowSelector, table }) => {
    const result = emptyTrackListCount(role);
    for (const container of root.querySelectorAll(selector)) {
      const clip = table ? container : container.closest(".track-list-viewport") ?? container;
      for (const row of container.querySelectorAll(rowSelector)) {
        if (!isTrackRowVisible(row, clip, viewport, styleFor)) continue;
        result.visibleRows += 1;
        if (row.matches(".current, [aria-current='true']")) result.playing += 1;
        if (row.matches(".paused")) result.paused += 1;
        if (row.matches(".buffering") || row.querySelector("[aria-busy='true']")) result.buffering += 1;
      }
    }
    for (const key of ["visibleRows", "playing", "paused", "buffering"] as const) {
      result[key] = Math.min(10_000, result[key]);
    }
    return result;
  });
}
