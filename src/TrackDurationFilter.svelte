<script lang="ts">
  import { onMount, tick } from "svelte";
  let { minDuration = $bindable(""), maxDuration = $bindable(""), minLabel = "Minimum track duration", maxLabel = "Maximum track duration", onchange }: {
    minDuration?: string; maxDuration?: string; minLabel?: string; maxLabel?: string; onchange?: () => void;
  } = $props();
  let open = $state(false);
  let root: HTMLDivElement;
  let panel: HTMLDivElement;
  let popupLeft = $state(0), popupTop = $state(0), popupMaxHeight = $state(600);
  function portal(node: HTMLDivElement, _visible: boolean) {
    const parent = node.parentElement!;
    return {
      update(visible: boolean) { (visible ? document.body : parent).appendChild(node); },
      destroy() { node.remove(); },
    };
  }
  async function toggle() {
    open = !open;
    if (!open) return;
    await tick();
    if (!open || !root || !panel) return;
    const anchor = root.getBoundingClientRect();
    const section = root.closest("section")?.getBoundingClientRect();
    const left = Math.max(8, section && section.width > 0 ? section.left + 8 : 8);
    const right = Math.min(window.innerWidth - 8, section && section.width > 0 ? section.right - 8 : window.innerWidth - 8);
    const footer = document.querySelector("footer.player")?.getBoundingClientRect();
    const bottom = Math.min(window.innerHeight - 8, footer && footer.top > 0 ? footer.top - 8 : window.innerHeight - 8);
    const theme = getComputedStyle(root);
    for (const name of ["--line", "--surface", "--base", "--text", "--muted", "--accent"]) {
      const value = theme.getPropertyValue(name); if (value) panel.style.setProperty(name, value);
    }
    popupMaxHeight = Math.max(40, bottom - 8);
    const measured = panel.getBoundingClientRect();
    const width = measured.width || 220, height = Math.min(measured.height || 150, popupMaxHeight);
    popupLeft = Math.max(left, Math.min(anchor.left, right - width));
    const below = anchor.bottom + 6;
    popupTop = Math.max(8, Math.min(below + height <= bottom ? below : anchor.top - height - 6, bottom - height));
  }
  onMount(() => {
    const dismiss = (event: Event) => { if (open && !(event.target instanceof Node && panel?.contains(event.target))) open = false; };
    window.addEventListener("scroll", dismiss, true); window.addEventListener("resize", dismiss);
    return () => { window.removeEventListener("scroll", dismiss, true); window.removeEventListener("resize", dismiss); };
  });
  const active = $derived(Boolean(minDuration.trim() || maxDuration.trim()));
  function clear() { minDuration = ""; maxDuration = ""; onchange?.(); }
</script>

<svelte:window onpointerdown={(event) => { if (open && root && event.target instanceof Node && !root.contains(event.target) && !panel?.contains(event.target)) open = false; }} onkeydown={(event) => { if (open && event.key === "Escape") { open = false; root.querySelector<HTMLButtonElement>(".filter-toggle")?.focus(); } }} />
<div class="duration-filter" bind:this={root}>
  <button type="button" class="filter-toggle" class:active aria-expanded={open} onclick={toggle}>Filters{#if active}<span aria-label="Duration filter active"> · 1</span>{/if}<span aria-hidden="true"> ▾</span></button>
  <div class="duration-inputs" bind:this={panel} use:portal={open} style:left={`${popupLeft}px`} style:top={`${popupTop}px`} style:max-height={`${popupMaxHeight}px`} hidden={!open}>
    <strong>Track duration</strong>
    <label>From<input aria-label={minLabel} inputmode="numeric" placeholder="mm:ss" bind:value={minDuration} oninput={() => onchange?.()} /></label>
    <label>To<input aria-label={maxLabel} inputmode="numeric" placeholder="mm:ss" bind:value={maxDuration} oninput={() => onchange?.()} /></label>
    <button class="clear-duration" type="button" disabled={!active} onclick={clear}>Clear duration</button>
  </div>
</div>

<style>
  .duration-filter{position:relative;flex:0 0 auto;align-self:end;min-width:0}.duration-filter>.filter-toggle{box-sizing:border-box;display:inline-flex;align-items:center;justify-content:center;gap:3px;height:34px;min-height:34px;margin:0;padding:0 10px;border:1px solid var(--line,#454a50);border-radius:7px;background:var(--surface,#202328);color:var(--muted,#a3a8ae);font:inherit;font-size:12px;line-height:18px;white-space:nowrap;cursor:pointer}.duration-filter>.filter-toggle.active{color:var(--accent,#b8ec8c);border-color:var(--accent,#b8ec8c)}.duration-inputs{position:fixed;left:0;top:0;z-index:1000;overflow:auto;display:grid;grid-template-columns:1fr 1fr;gap:9px;box-sizing:border-box;width:220px;max-width:calc(100vw - 32px);padding:12px;border:1px solid var(--line,#454a50);border-radius:8px;background:var(--surface,#202328);box-shadow:0 8px 24px #0007;color:var(--text,#eee)}.duration-inputs[hidden]{display:none}.duration-inputs>strong{grid-column:1/-1;font-size:12px;font-weight:600}.duration-inputs>label{display:flex;flex-direction:column;align-items:stretch;gap:4px;min-width:0;margin:0;color:var(--muted,#a3a8ae);font-size:11px}.duration-inputs input{box-sizing:border-box;display:block;width:100%;min-width:0;height:34px;margin:0;padding:0 8px;border:1px solid var(--line,#454a50);border-radius:6px;background:var(--base,#17191c);color:var(--text,#eee);font:inherit;font-size:12px;line-height:18px}.duration-inputs>.clear-duration{grid-column:1/-1;justify-self:start;min-height:28px;padding:3px 0;border:0;background:none;color:var(--accent,#b8ec8c);font:inherit;font-size:11px;cursor:pointer}.clear-duration:disabled{opacity:.45;cursor:default}
</style>
