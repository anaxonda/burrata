<script lang="ts">
  import { languages, languageName } from "./lib/languages";
  let { value = "en", onChange }: { value?: string; onChange: (language: string) => Promise<unknown> | void } = $props();
  let expanded = $state(false), query = $state(""), busy = $state(false), error = $state("");
  const results = $derived(languages.filter(item => `${item.name} ${item.code}`.toLowerCase().includes(query.trim().toLowerCase())));
  async function select(code: string) { busy = true; error = ""; try { await onChange(code); expanded = false; query = ""; } catch(failure) { error = String(failure); } finally { busy = false; } }
</script>
<div class="language-picker">
  <button class="language-trigger" type="button" aria-label="Translation language" aria-expanded={expanded} onclick={() => expanded = !expanded}><span>Translation language</span><strong>{languageName(value)}</strong><span aria-hidden="true">⌄</span></button>
  {#if expanded}
    <div class="language-choices">
      <input type="search" aria-label="Search translation languages" placeholder="Search languages…" bind:value={query} />
      <div class="language-list" role="listbox" aria-label="Translation languages" aria-busy={busy}>
        {#each results as language (language.code)}<button type="button" role="option" aria-selected={value === language.code || languageName(value) === language.name} disabled={busy} onclick={() => void select(language.code)}><span>{language.name}</span><small>{language.code}</small>{#if languageName(value) === language.name}<span aria-hidden="true">✓</span>{/if}</button>{/each}
        {#if !results.length}<p>No languages found.</p>{/if}
      </div>
    </div>
  {/if}
  {#if error}<p role="alert">{error}</p>{/if}
</div>
<style>
  .language-picker{min-width:0}.language-trigger{display:flex;align-items:center;gap:10px;width:100%;text-align:left}.language-trigger strong{margin-left:auto;font-weight:500;color:var(--text)}
  button,input{box-sizing:border-box;border:1px solid var(--line,#454a50);border-radius:7px;background:var(--raised,#ffffff08);color:var(--text,#e5ebe7);font:inherit;padding:8px 10px}button{cursor:pointer}button:focus-visible,input:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
  .language-choices{margin-top:8px;padding:8px;border:1px solid var(--line,#454a50);border-radius:9px}.language-choices input{width:100%;margin-bottom:6px}.language-list{max-height:210px;overflow:auto;display:grid;gap:2px}.language-list button{border:0;display:flex;align-items:center;gap:8px;text-align:left;background:transparent}.language-list button:hover,.language-list button[aria-selected="true"]{background:color-mix(in srgb,var(--accent) 12%,transparent)}.language-list small{margin-left:auto;color:var(--muted)}p{font-size:.85rem;color:var(--muted)}
</style>
