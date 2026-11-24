<script lang="ts">
  import MenuIcon from "./MenuIcon.svelte";

  let { value, entity = false, servers, onchange }: {
    value: string;
    entity?: boolean;
    servers: { jellyfin: boolean; subsonic: boolean };
    onchange: (value: string) => void;
  } = $props();

  const options = $derived([
    { id: "saved", label: "Local library" },
    ...(servers.jellyfin ? [{ id: "jellyfin", label: "Jellyfin" }] : []),
    ...(servers.subsonic ? [{ id: "subsonic", label: "Subsonic" }] : []),
  ]);
  const selected = $derived(value === "all" || value === "" ? options.map(option => option.id)
    : value === "__saved__" ? ["saved"] : value.split(",").filter(id => options.some(option => option.id === id)));
  const caption = $derived(selected.length === options.length ? "All sources"
    : selected.length === 0 ? "No sources"
    : options.filter(option => selected.includes(option.id)).map(option => option.label).join(" + "));
  let host: HTMLDivElement;
  let trigger: HTMLButtonElement;
  let open = $state(false);
  const id = $props.id();

  function toggle(source: string, checked: boolean) {
    const next = options.filter(option => option.id === source ? checked : selected.includes(option.id)).map(option => option.id);
    onchange(next.length === options.length ? (entity ? "" : "all")
      : next.length === 0 ? "none"
      : entity && next.length === 1 && next[0] === "saved" ? "__saved__" : next.join(","));
  }

  function onKeydown(event: KeyboardEvent) {
    if (!open || event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    open = false;
    trigger.focus();
  }
</script>

<svelte:window onclick={event => {
  if (open && event.target instanceof Node && !host.contains(event.target)) open = false;
}} onkeydown={onKeydown} />
<div class="library-source-filter" bind:this={host} role="group" aria-label="Library sources">
  <span id={`${id}-caption`}>Sources</span>
  <button class="source-trigger dropdown-trigger" type="button" bind:this={trigger} aria-expanded={open} aria-controls={open ? id : undefined}
    aria-label={`Library sources, ${caption}`} title={caption} onclick={() => { open = !open; }}>
    <span>{caption}</span><MenuIcon name={open ? "arrow-up" : "arrow-down"} />
  </button>
  {#if open}
    <div class="source-menu dropdown-menu" id={id} role="group" aria-labelledby={`${id}-caption`}>
      {#each options as option (option.id)}
        <label class="dropdown-option">
          <input type="checkbox" checked={selected.includes(option.id)} onchange={event => toggle(option.id, event.currentTarget.checked)} />
          {option.label}
        </label>
      {/each}
    </div>
  {/if}
</div>

<style>
  .library-source-filter { position: relative; font-size: .8rem; }
  .source-trigger {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-width: 125px;
    max-width: min(240px, 54vw);
    padding: 5px 9px;
  }
  .source-trigger span { overflow: hidden; text-overflow: ellipsis; }
  .source-trigger :global(svg) { flex: none; width: 14px; height: 14px; color: var(--muted); }
  .source-menu {
    position: absolute;
    z-index: 8;
    top: calc(100% + 5px);
    right: 0;
    width: max(178px, 100%);
  }
  .source-menu label { flex-direction: row; margin: 0; }
  .source-menu input { flex: none; width: 15px; min-width: 0; height: 15px; margin: 0; accent-color: var(--accent); }
</style>
