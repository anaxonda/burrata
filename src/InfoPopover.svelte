<script lang="ts">
  import type { Snippet } from "svelte";
  let { label, children, placement = "bottom" }: { label: string; children: Snippet; placement?: "top" | "bottom" } = $props();
  let element: HTMLDetailsElement;
  let above = $state(false), availableHeight = $state(300), horizontalShift = $state(0), availableWidth = $state(300);
  function reposition() {
    if (!element?.open) return;
    const anchor = element.getBoundingClientRect();
    const panel = element.querySelector<HTMLElement>(".info-content");
    const footer = document.querySelector("#burrata-playback-controls")?.getBoundingClientRect();
    let left = 0, right = window.innerWidth, top = 0;
    let bottom = footer && !element.closest("footer") ? Math.min(window.innerHeight, footer.top) : window.innerHeight;
    for (let parent = element.parentElement; parent; parent = parent.parentElement) {
      const style = getComputedStyle(parent);
      const rect = parent.getBoundingClientRect();
      const clips = (value: string) => /^(auto|scroll|hidden|clip)$/.test(value);
      if (rect.width > 0 && clips(style.overflowX)) { left = Math.max(left, rect.left + parent.clientLeft); right = Math.min(right, rect.left + parent.clientLeft + (parent.clientWidth || rect.width)); }
      if (rect.height > 0 && clips(style.overflowY)) { top = Math.max(top, rect.top + parent.clientTop); bottom = Math.min(bottom, rect.top + parent.clientTop + (parent.clientHeight || rect.height)); }
    }
    availableWidth = Math.max(0, Math.min(300, window.innerWidth * .75, right - left - 16));
    const naturalLeft = anchor.right - availableWidth;
    horizontalShift = Math.max(left + 8 - naturalLeft, Math.min(0, right - 8 - anchor.right));
    const below = bottom - anchor.bottom - 8, over = anchor.top - top - 8;
    above = placement === "top" || below < (panel?.scrollHeight ?? 0) && over > below;
    availableHeight = Math.max(0, Math.min(300, above ? over : below));
  }
  function dismiss(event: MouseEvent) {
    if (element?.open && event.target instanceof Node && !element.contains(event.target)) element.open = false;
  }
</script>

<svelte:document onscrollcapture={reposition} />
<svelte:window onresize={reposition} onclick={dismiss} onkeydown={(event) => {
  if (event.key === "Escape" && element?.open) {
    element.open = false;
    element.querySelector("summary")?.focus();
  }
}} />
<details class="info-popover" class:above={above || placement === "top"} bind:this={element} ontoggle={reposition}>
  <summary aria-label={label} title={label}>i</summary>
  <div class="info-content" style:width={`${availableWidth}px`} style:max-height={`${availableHeight}px`} style:transform={`translateX(${horizontalShift}px)`} role="note">{@render children()}</div>
</details>

<style>
  .info-popover { position:relative; display:inline-block; flex:none; margin:0; padding:0; border:0; }
  .above .info-content { top:auto; bottom:28px; }
  summary { display:inline-flex; align-items:center; justify-content:center; width:22px; height:22px; border:1px solid var(--line); border-radius:50%; color:var(--muted); cursor:pointer; font-size:12px; list-style:none; }
  summary::-webkit-details-marker { display:none; }
  summary:hover, summary:focus-visible, details[open] summary { color:var(--accent); border-color:var(--accent); }
  .info-content { position:absolute; z-index:100; right:0; top:28px; width:min(300px,75vw); box-sizing:border-box; overflow:auto; padding:12px 14px; border:1px solid var(--line); border-radius:10px; background:var(--surface,#18201b); color:var(--text); box-shadow:0 8px 24px #0005; font-size:12px; line-height:1.5; font-weight:400; text-align:left; }
</style>
