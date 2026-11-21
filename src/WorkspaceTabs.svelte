<script lang="ts">
  import { tick } from "svelte";

  let { tabs, activeId, busy, onSelect, onClose, onNew }: {
    tabs: { id: number; label: string }[];
    activeId: number;
    busy: boolean;
    onSelect: (id: number) => void;
    onClose: (id: number) => void;
    onNew: () => void;
  } = $props();
  let tablist: HTMLDivElement;

  async function moveFocus(event: KeyboardEvent, index: number) {
    if (busy) return;
    const last = tabs.length - 1;
    const next = event.key === "ArrowRight" ? (index + 1) % tabs.length
      : event.key === "ArrowLeft" ? (index + last) % tabs.length
      : event.key === "Home" ? 0 : event.key === "End" ? last : -1;
    if (next < 0) return;
    event.preventDefault();
    const tab = tabs[next];
    if (!tab) return;
    onSelect(tab.id);
    await tick();
    tablist.querySelector<HTMLElement>(`[data-workspace-tab="${tab.id}"]`)?.focus();
  }
</script>

<div class="workspace-tabs">
  <div class="tabs" role="tablist" aria-label="Workspace tabs" bind:this={tablist}>
    {#each tabs as tab, index (tab.id)}
      <div class="tab" class:active={tab.id === activeId} role="presentation">
        <button class="tab-label" type="button" role="tab" aria-selected={tab.id === activeId}
          aria-disabled={busy} tabindex={tab.id === activeId ? 0 : -1} title={tab.label} data-workspace-tab={tab.id}
          onclick={() => onSelect(tab.id)} onkeydown={(event) => void moveFocus(event, index)}>{tab.label}</button>
        <button class="close-tab" type="button" aria-label={`Close ${tab.label} tab`} title="Close tab"
          disabled={busy} onclick={() => onClose(tab.id)}>×</button>
      </div>
    {/each}
  </div>
  <button class="new-tab" type="button" aria-label="New tab" title="New tab" disabled={busy} onclick={onNew}>＋</button>
</div>

<style>
  .workspace-tabs { display:flex; align-items:center; gap:5px; min-width:0; flex:none; padding:4px 9px; background:var(--surface); border-bottom:1px solid var(--line); }
  .tabs { display:flex; gap:4px; min-width:0; flex:1; overflow-x:auto; scrollbar-width:thin; }
  .tab { display:flex; align-items:center; gap:2px; min-width:64px; max-width:200px; flex:0 0 auto; border:1px solid transparent; border-radius:6px; }
  .tab.active { background:var(--raised); border-color:var(--line); }
  .tab button,.new-tab { min-height:28px; padding:3px 7px; line-height:1.2; font-size:.8rem; border:0; background:transparent; color:var(--text); }
  .tab button:hover,.new-tab:hover { background:var(--raised); }
  .tab-label { flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; text-align:left; }
  .tab .close-tab { font-size:1rem; flex:none; color:var(--muted); }
  .new-tab { flex:none; font-size:1rem; }
</style>
