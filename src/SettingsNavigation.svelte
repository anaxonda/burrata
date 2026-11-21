<script lang="ts">
  type Category = "general" | "playback" | "library" | "downloads" | "connections" | "integrations" | "advanced";
  let { active, onchange }: { active: Category; onchange: (category: Category) => void } = $props();
  const categories: { id: Category; label: string }[] = [
    { id: "general", label: "General" },
    { id: "playback", label: "Playback" },
    { id: "library", label: "Library" },
    { id: "downloads", label: "Downloads" },
    { id: "connections", label: "Connections" },
    { id: "advanced", label: "Troubleshooting" },
  ];
  const current = $derived(active === "integrations" ? "connections" : active);
</script>

<nav class="settings-nav section-switcher" aria-label="Settings categories">
  <label class="category-picker">Settings category
    <select value={current} onchange={event => onchange(event.currentTarget.value as Category)}>
      {#each categories as category}<option value={category.id}>{category.label}</option>{/each}
    </select>
  </label>
  {#each categories as category}
    <button type="button" aria-current={current === category.id ? "page" : undefined}
      onclick={() => onchange(category.id)}>{category.label}</button>
  {/each}
</nav>

<style>
  .settings-nav {
    border-bottom: 1px solid var(--line);
    padding-bottom: 12px;
    margin-bottom: 18px;
  }
  .category-picker { display: none; }
  @container settings-pane (max-width: 36rem) {
    .settings-nav > button { display: none; }
    .category-picker { display: grid; gap: 6px; margin: 0; width: 100%; font-size: 12px; }
    .category-picker select { min-height: 40px; }
  }
</style>
