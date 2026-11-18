import { onDestroy } from "svelte";
import { seekPreviewTime } from "./seek-preview";

export function seekInteraction(
  identity: () => unknown,
  disabled: () => boolean,
  duration: () => number,
  seek: (position: number) => void,
) {
  let target = $state<number | null>(null);
  let hover = $state<number | null>(null);
  let dragging = false;
  let owner: { identity: unknown } | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let lastIdentity = identity();

  function clearPending() {
    clearTimeout(timer);
    owner = null;
    target = null;
    hover = null;
  }

  function isCurrent() {
    return owner !== null && owner.identity === identity() && !disabled();
  }

  $effect(() => {
    const currentIdentity = identity();
    const isDisabled = disabled();
    if (lastIdentity !== currentIdentity || isDisabled) clearPending();
    lastIdentity = currentIdentity;
  });

  function commit() {
    const position = isCurrent() && target !== null && Number.isFinite(target)
      ? Math.max(0, Math.min(duration() || 1, target))
      : null;
    dragging = false;
    clearPending();
    if (position !== null) seek(position);
  }

  onDestroy(() => clearTimeout(timer));

  return {
    get target() { return target; },
    get preview() { return target ?? hover; },
    pointerMove(event: PointerEvent & { currentTarget: HTMLInputElement }) {
      if (disabled() || (dragging && !isCurrent())) return;
      const { left, width } = event.currentTarget.getBoundingClientRect();
      hover = seekPreviewTime(event.clientX, left, width, duration());
    },
    pointerLeave() { if (!dragging) hover = null; },
    pointerDown(event: PointerEvent & { currentTarget: HTMLInputElement }) {
      if (disabled()) return;
      clearPending();
      owner = { identity: identity() };
      dragging = true;
      event.currentTarget.setPointerCapture(event.pointerId);
    },
    input(event: Event & { currentTarget: HTMLInputElement }) {
      if (!dragging && !disabled()) owner = { identity: identity() };
      if (!isCurrent()) return;
      target = event.currentTarget.valueAsNumber;
      clearTimeout(timer);
      if (!dragging) timer = setTimeout(commit, 150);
    },
    commit,
    lostPointerCapture() { if (dragging) commit(); },
    cancel() {
      dragging = false;
      clearPending();
    },
  };
}
