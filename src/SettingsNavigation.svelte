<script lang="ts">
  type Category = "general" | "playback" | "library" | "downloads" | "connections" | "integrations" | "advanced";
  let { active, onchange }: { active: Category; onchange: (category: Category) => void } = $props();
  const categories: { id: Category; label: string }[] = [
    { id: "general", label: "General" },
    { id: "playback", label: "Playback" },
    { id: "library", label: "Library" },
    { id: "downloads", label: "Downloads" },
    { id: "connections", label: "Connections" },
    { id: "integrations", label: "Integrations" },
    { id: "advanced", label: "Troubleshooting" },
  ];
</script>

<nav class="settings-nav" aria-label="Settings categories">
  {#each categories as category}
    <button type="button" class:active={active === category.id} aria-current={active === category.id ? "page" : undefined}
      onclick={() => onchange(category.id)}>{category.label}</button>
  {/each}
</nav>

<style>
  .settings-nav { display:flex; flex-wrap:wrap; gap:6px; border-bottom:1px solid var(--line); padding-bottom:12px; margin-bottom:18px; }
  button { border:1px solid transparent; border-radius:8px; background:transparent; color:var(--muted); padding:8px 12px; font:inherit; cursor:pointer; }
  button:hover { color:var(--text); background:var(--surface-2); }
  button.active { color:var(--text); border-color:var(--line); background:var(--surface-2); }
</style>
