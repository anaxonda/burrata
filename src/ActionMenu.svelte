<script lang="ts">
  import { onMount, type Snippet } from "svelte";
  import { positionContextMenu, type MenuAnchorRect } from "./lib/menu-geometry";

  let { label, title, subtitle, closeLabel = "Close actions", anchor = null, anchorRect = null, playerHeight = 90, role = "dialog", class: className = "", children, badge, onClose }: {
    label: string; title?: string; subtitle?: string; closeLabel?: string;
    anchor?: { x: number; y: number } | null; anchorRect?: MenuAnchorRect | null; playerHeight?: number;
    role?: "dialog" | "menu"; class?: string; children: Snippet; badge?: Snippet; onClose: () => void;
  } = $props();
  let dialog: HTMLDialogElement;
  let viewportWidth = $state(1024), viewportHeight = $state(768);
  let menuWidth = $state(320), menuHeight = $state(240);
  const anchored = $derived(Boolean(anchor || anchorRect));
  const position = $derived.by(() => {
    const width = Math.min(menuWidth || 320, Math.max(0, viewportWidth - 16));
    const height = menuHeight || 240;
    if (anchorRect) return positionContextMenu(anchorRect, { width, height }, { width: viewportWidth, height: viewportHeight }, playerHeight);
    const bottom = Math.max(8, viewportHeight - Math.max(0, playerHeight) - 8);
    return { left: Math.max(8, Math.min(anchor?.x ?? 8, viewportWidth - width - 8)), top: Math.max(8, Math.min(anchor?.y ?? 8, bottom - height)), maxHeight: Math.max(0, bottom - 8) };
  });
  function focusable() {
    return [...dialog.querySelectorAll<HTMLElement>("button:not(:disabled), a[href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])")]
      .filter((element) => !element.matches(":disabled") && element.tabIndex >= 0 && !element.hidden && !element.closest("[hidden], [aria-hidden='true']") && element.getClientRects().length > 0);
  }
  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") { event.preventDefault(); onClose(); }
    else if (event.key === "Tab") {
      const items = focusable(), first = items[0], last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first && last) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last && first) { event.preventDefault(); first.focus(); }
    } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
      const items = focusable().filter((element) => !element.closest("header"));
      if (items.length) {
        const current = items.indexOf(document.activeElement as HTMLElement);
        const index = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : event.key === "ArrowDown" ? (current + 1) % items.length : (current <= 0 ? items.length - 1 : current - 1);
        event.preventDefault(); items[index].focus();
      }
    }
    event.stopPropagation();
  }
  onMount(() => {
    const trigger = document.activeElement as HTMLElement | null;
    let restoreFocus = true;
    const dismissForLayout = () => { restoreFocus = false; onClose(); };
    const dismissForScroll = (event: Event) => { if (!(event.target instanceof Node) || !dialog.contains(event.target)) dismissForLayout(); };
    window.addEventListener("resize", dismissForLayout);
    window.addEventListener("scroll", dismissForScroll, true);
    dialog.showModal();
    (dialog.querySelector<HTMLElement>(".primary-action:not(:disabled)") ?? dialog.querySelector<HTMLElement>("button:not(:disabled), a[href]"))?.focus();
    return () => {
      window.removeEventListener("resize", dismissForLayout);
      window.removeEventListener("scroll", dismissForScroll, true);
      if (restoreFocus && trigger?.isConnected && (dialog.contains(document.activeElement) || document.activeElement === document.body)) trigger.focus();
    };
  });
</script>

<svelte:window bind:innerWidth={viewportWidth} bind:innerHeight={viewportHeight} onpointerdown={(event) => { if (dialog?.open && !dialog.contains(event.target as Node)) onClose(); }} />
<dialog bind:this={dialog} bind:clientWidth={menuWidth} bind:clientHeight={menuHeight} class={`action-menu ${className}`} class:context-menu={anchored} {role} aria-label={label} aria-modal="true" style:left={anchored ? `${position.left}px` : undefined} style:top={anchored ? `${position.top}px` : undefined} style:--menu-player-height={`${Math.max(0, playerHeight)}px`} onclose={onClose} oncancel={(event) => { event.preventDefault(); onClose(); }} onclick={(event) => { if (event.target === dialog) onClose(); }} onkeydown={handleKeydown}>
  {#if title}<header><div class="action-menu-heading"><strong>{title}</strong>{#if subtitle}<small>{subtitle}</small>{/if}{#if badge}{@render badge()}{/if}</div><button class="action-menu-close" aria-label={closeLabel} onclick={onClose}>×</button></header>{/if}
  {@render children()}
</dialog>

<style>
  :global(.action-menu) { position:fixed;z-index:100;box-sizing:border-box;inset:8px 0 calc(var(--menu-player-height,var(--player-height,90px)) + 8px);margin:auto;width:min(320px,calc(100vw - 16px));max-height:calc(100dvh - var(--menu-player-height,var(--player-height,90px)) - 16px);overflow:auto;padding:8px;border:1px solid var(--context-menu-border,var(--line));border-radius:10px;background:var(--context-menu-surface,var(--surface));color:var(--context-menu-text,var(--text));box-shadow:0 20px 80px #0008; }
  :global(.action-menu.context-menu) { inset:auto;margin:0; }
  :global(.action-menu::backdrop) { background:#0006; }
  :global(.action-menu.context-menu::backdrop) { background:transparent; }
  header { display:flex;align-items:center;gap:8px;padding:2px 4px 6px;border-bottom:1px solid var(--context-menu-border,var(--line));margin-bottom:4px;color:var(--context-menu-muted,var(--muted));font-size:11px; }
  .action-menu-heading { flex:1;min-width:0; }
  strong,small { display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap; }
  small { margin-top:4px;color:var(--context-menu-muted,var(--muted));font-size:11px; }
  .action-menu-close { display:grid;place-items:center;flex:0 0 28px;width:28px;height:28px;min-width:28px;padding:0;border:0;border-radius:5px;background:transparent;color:inherit;font-size:22px;touch-action:manipulation; }
  :global(.action-menu>button),:global(.action-menu .source-row) { display:flex;align-items:center;gap:12px;box-sizing:border-box;width:100%;min-height:36px;padding:5px 9px;border:0;border-radius:5px;background:transparent;color:var(--context-menu-text,var(--text));font:inherit;font-size:13px;line-height:1.3;text-align:left;white-space:normal;overflow-wrap:anywhere;touch-action:manipulation; }
  :global(.action-menu>button>span) { min-width:0; }
  :global(.action-menu>button:hover),:global(.action-menu .source-row:hover),.action-menu-close:hover { background:var(--context-menu-hover,var(--raised)); }
  :global(.action-menu>button.menu-section),:global(.action-menu .source-row.menu-section) { margin-top:5px;padding-top:7px;border-top:1px solid var(--context-menu-border,var(--line));border-top-left-radius:0;border-top-right-radius:0; }
  :global(.action-menu>button.primary-action) { margin:3px 0;background:var(--context-menu-primary,color-mix(in srgb,var(--accent) 18%,var(--surface)));font-weight:650; }
  :global(.action-menu>button.primary-action:hover) { background:var(--context-menu-hover,var(--raised)); }
  :global(.action-menu>button.destructive) { color:var(--danger,#e79a93); }
  :global(.action-menu button:disabled) { opacity:.55; }
  :global(.action-menu svg[aria-hidden="true"]) { display:block;flex:0 0 20px;color:var(--context-menu-muted,var(--muted)); }
  :global(.action-menu>button.primary-action svg[aria-hidden="true"]) { color:var(--accent); }
  :global(.action-menu .source-row),:global(.action-menu .source-row.menu-section) { padding:0;flex-direction:column;align-items:stretch;gap:0; }
  :global(.action-menu .source-row .source-link) { display:flex;align-items:center;gap:12px;width:100%;min-height:35px;padding:5px 9px;border:0;border-radius:5px;background:transparent;color:var(--context-menu-text,var(--text));font:inherit;font-size:13px;text-align:left; }
  :global(.action-menu .source-row .source-link:hover) { background:transparent; }
  :global(.action-menu button:focus-visible),:global(.action-menu a:focus-visible) { outline:2px solid var(--accent);outline-offset:1px; }
  @media (max-width:600px) { :global(.action-menu>button),:global(.action-menu .source-row .source-link) { min-height:42px; } }
  @media (min-width:601px) {
    :global(.action-menu) { padding:5px;border-radius:6px;box-shadow:0 8px 28px #0007; }
    :global(.action-menu>button) { min-height:32px;gap:10px;padding:4px 8px;border-radius:3px; }
    :global(.action-menu>button.primary-action) { margin:1px 0;font-weight:600; }
    :global(.action-menu svg[aria-hidden="true"]) { width:18px;height:18px;flex-basis:18px; }
    :global(.action-menu>button.menu-section),:global(.action-menu .source-row.menu-section) { margin-top:4px; }
    :global(.action-menu .source-row .source-link) { min-height:31px;gap:10px;padding:4px 8px; }
  }
</style>
