<script lang="ts">
  import { findSettings, settingsCategories, type SettingsCategory } from "./lib/settings-search";

  let { active, desktop = true, soulseekConfigured = false, onchange }: {
    active: SettingsCategory;
    desktop?: boolean;
    soulseekConfigured?: boolean;
    onchange: (category: SettingsCategory, target?: string) => void;
  } = $props();
  let query = $state("");
  let searchInput: HTMLInputElement;
  const current = $derived(active === "integrations" ? "connections" : active);
  const results = $derived(findSettings(query, desktop)
    .filter(setting => soulseekConfigured || setting.target !== "soulseek-policy-settings"));

  function openSetting(setting: (typeof results)[number]) {
    query = "";
    onchange(setting.category, setting.target);
  }

  function changeCategory(category: SettingsCategory) {
    query = "";
    onchange(category);
  }
</script>

<form class="settings-search" role="search" aria-label="Settings" onsubmit={event => {
  event.preventDefault();
  if (results[0]) openSetting(results[0]);
}}>
  <label>
    Search settings
    <input type="search" bind:this={searchInput} bind:value={query} placeholder="Find a setting" autocomplete="off"
      onkeydown={event => {
        if (event.key === "Escape" && query) {
          event.preventDefault();
          event.stopPropagation();
          query = "";
        }
      }} />
  </label>
  {#if query}
    <button class="clear-search" type="button" onclick={() => { query = ""; searchInput.focus(); }}>Clear</button>
  {/if}
</form>
{#if query.trim()}
  {#if results.length}
    <p class="sr-only" role="status">{results.length} {results.length === 1 ? "setting" : "settings"} found.</p>
    <ul class="settings-search-results" aria-label="Matching settings">
      {#each results as setting (setting.target)}
        <li><button type="button" onclick={() => openSetting(setting)}>
          <span>{setting.label}</span>
          <small>{settingsCategories.find(category => category.id === setting.category)?.label}</small>
        </button></li>
      {/each}
    </ul>
  {:else}
    <p class="settings-search-empty" role="status">No matching settings.</p>
  {/if}
{/if}

<nav class="settings-nav section-switcher" aria-label="Settings categories">
  <label class="category-picker">Settings category
    <select value={current} onchange={event => changeCategory(event.currentTarget.value as SettingsCategory)}>
      {#each settingsCategories as category}<option value={category.id}>{category.label}</option>{/each}
    </select>
  </label>
  {#each settingsCategories as category}
    <button type="button" aria-current={current === category.id ? "page" : undefined}
      onclick={() => changeCategory(category.id)}>{category.label}</button>
  {/each}
</nav>

<style>
  .settings-search { display: flex; align-items: end; gap: 8px; margin: 0 0 12px; max-width: 34rem; }
  .settings-search label { flex: 1; min-width: 0; margin: 0; }
  .settings-search input { min-height: 38px; width: 100%; }
  .clear-search { min-height: 38px; }
  .settings-search-results {
    display: grid;
    gap: 2px;
    max-width: 34rem;
    max-height: 14rem;
    overflow: auto;
    list-style: none;
    margin: -4px 0 12px;
    padding: 4px;
    border: 1px solid var(--line);
    border-radius: 9px;
    background: var(--surface);
  }
  .settings-search-results button {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: space-between;
    gap: 4px 12px;
    width: 100%;
    text-align: left;
    border-color: transparent;
  }
  .settings-search-results small, .settings-search-empty { color: var(--muted); }
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
