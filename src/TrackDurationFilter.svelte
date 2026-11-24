<script lang="ts">
  import { onMount, tick, type Snippet } from "svelte";
  import MenuIcon from "./MenuIcon.svelte";
  let { minDuration = $bindable(""), maxDuration = $bindable(""), minLabel = "Minimum track duration", maxLabel = "Maximum track duration", inline = false, includeDuration = true, extraFilters, extraFiltersActive = false, onchange }: {
    minDuration?: string; maxDuration?: string; minLabel?: string; maxLabel?: string; inline?: boolean; includeDuration?: boolean; extraFilters?: Snippet; extraFiltersActive?: boolean; onchange?: (minimum: string, maximum: string) => void;
  } = $props();
  let open = $state(false);
  const panelId = $props.id();
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
    if (!open || inline) return;
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
    const dismiss = (event: Event) => { if (!inline && open && !(event.target instanceof Node && panel?.contains(event.target))) open = false; };
    window.addEventListener("scroll", dismiss, true); window.addEventListener("resize", dismiss);
    return () => { window.removeEventListener("scroll", dismiss, true); window.removeEventListener("resize", dismiss); };
  });
  const active = $derived(Boolean(minDuration.trim() || maxDuration.trim()));
  function clear() { minDuration = ""; maxDuration = ""; onchange?.("", ""); }
</script>

<svelte:window onpointerdown={(event) => { if (open && root && event.target instanceof Node && !root.contains(event.target) && !panel?.contains(event.target)) open = false; }} onkeydown={(event) => { if (open && event.key === "Escape") { if (inline) event.preventDefault(); open = false; root.querySelector<HTMLButtonElement>(".filter-toggle")?.focus(); } }} />
<div class="duration-filter" bind:this={root}>
  <button type="button" class="filter-toggle dropdown-trigger" class:active={active || extraFiltersActive} aria-expanded={open} aria-controls={panelId} onclick={toggle}>Filters{#if active || extraFiltersActive}<span aria-label={extraFiltersActive ? "Additional filters active" : "Duration filter active"}> · {Number(active) + Number(extraFiltersActive)}</span>{/if}<MenuIcon name={open ? "arrow-up" : "arrow-down"} /></button>
  <div id={panelId} class="duration-inputs dropdown-menu" class:inline bind:this={panel} use:portal={open && !inline} style:left={`${popupLeft}px`} style:top={`${popupTop}px`} style:max-height={`${popupMaxHeight}px`} hidden={!open}>
    {#if extraFilters}<div class="extra-filters">{@render extraFilters()}</div>{/if}
    {#if includeDuration}
      <strong>Track duration</strong>
      <label>From<input aria-label={minLabel} inputmode="numeric" placeholder="mm:ss" bind:value={minDuration} oninput={(event) => onchange?.(event.currentTarget.value, maxDuration)} /></label>
      <label>To<input aria-label={maxLabel} inputmode="numeric" placeholder="mm:ss" bind:value={maxDuration} oninput={(event) => onchange?.(minDuration, event.currentTarget.value)} /></label>
      <button class="clear-duration" type="button" disabled={!active} onclick={clear}>Clear duration</button>
    {/if}
  </div>
</div>

<style>
  .extra-filters { grid-column: 1 / -1; display: grid; gap: 7px; }
  .extra-filters :global(label) { display: grid; gap: 4px; margin: 0; color: var(--muted); font-size: 11px; }
  .extra-filters :global(select) { width: 100%; min-width: 0; }
  .duration-inputs.inline{position:static;width:100%;max-width:none;margin-top:6px;box-shadow:none}
  .duration-filter{position:relative;flex:0 0 auto;align-self:end;min-width:0}
  .filter-toggle{display:inline-flex;align-items:center;gap:4px;margin:0;padding:0 8px;white-space:nowrap}
  .filter-toggle :global(svg){width:14px;height:14px;flex:none;color:var(--muted)}
  .filter-toggle.active{color:var(--accent);border-color:var(--accent)}
  .duration-inputs{position:fixed;left:0;top:0;z-index:1000;overflow:auto;overscroll-behavior:contain;display:grid;grid-template-columns:1fr 1fr;gap:8px;box-sizing:border-box;width:220px;max-width:calc(100vw - 32px);padding:8px}
  .duration-inputs[hidden]{display:none}.duration-inputs>strong{grid-column:1/-1;font-size:12px;font-weight:600}.duration-inputs>label{display:flex;flex-direction:column;align-items:stretch;gap:4px;min-width:0;margin:0;color:var(--muted,#a3a8ae);font-size:11px}.duration-inputs input{box-sizing:border-box;display:block;width:100%;min-width:0;height:34px;margin:0;padding:0 8px;border:1px solid var(--line,#454a50);border-radius:6px;background:var(--base,#17191c);color:var(--text,#eee);font:inherit;font-size:12px;line-height:18px}.duration-inputs>.clear-duration{grid-column:1/-1;justify-self:start;min-height:28px;padding:3px 0;border:0;background:none;color:var(--accent,#b8ec8c);font:inherit;font-size:11px;cursor:pointer}.clear-duration:disabled{opacity:.45;cursor:default}
</style>
