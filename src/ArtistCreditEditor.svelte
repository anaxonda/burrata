<script lang="ts">
  import { untrack } from "svelte";
  export interface ArtistCreditDraft {
    name: string;
    role: string;
  }

  let {
    credits = [],
    onSave,
    onCancel,
    onReset,
  }: {
    credits?: ArtistCreditDraft[];
    onSave: (credits: ArtistCreditDraft[]) => void | Promise<void>;
    onCancel?: () => void;
    onReset?: () => void | Promise<void>;
  } = $props();

  const MAX_CREDITS = 32;
  let rows = $state<ArtistCreditDraft[]>(untrack(() => credits.map(credit => ({ name: credit.name, role: credit.role }))));
  let saving = $state(false);
  let error = $state("");
  let status = $state("");

  const invalid = $derived(rows.some(row => !row.name.trim() || !row.role.trim()));

  function addCredit() {
    if (rows.length >= MAX_CREDITS) return;
    rows = [...rows, { name: "", role: "artist" }];
  }

  function removeCredit(index: number) {
    rows = rows.filter((_, rowIndex) => rowIndex !== index);
  }

  async function save() {
    if (saving || invalid || rows.length > MAX_CREDITS) return;
    saving = true;
    error = "";
    try {
      await onSave(rows.map(row => ({ name: row.name.trim(), role: row.role.trim() })));
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      saving = false;
    }
  }

  async function reset() {
    if (saving || !onReset) return;
    saving = true;
    error = "";
    status = "";
    try {
      await onReset();
      status = "Manual correction cleared.";
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      saving = false;
    }
  }
</script>

<section class="artist-credit-editor" aria-label="Correct artist credits">
  <header>
    <div>
      <h2>Artist credits</h2>
      <p>Use one row per credited artist. These names are used in the Library for every saved copy linked to this recording. Names are kept exactly as entered; punctuation is not split.</p>
    </div>
    {#if onCancel}<button type="button" class="secondary" onclick={onCancel}>Cancel</button>{/if}
  </header>

  <div class="credit-list">
    {#each rows as row, index}
      <fieldset class="credit-row">
        <legend>Credit {index + 1}</legend>
        <label>
          <span>Artist name</span>
          <input aria-label={`Artist name ${index + 1}`} autocomplete="off" maxlength="200" bind:value={row.name} />
        </label>
        <label>
          <span>Role</span>
          <input aria-label={`Artist role ${index + 1}`} autocomplete="off" maxlength="80" bind:value={row.role} />
        </label>
        <button type="button" class="remove" aria-label={`Remove credit ${index + 1}`} disabled={rows.length <= 1} onclick={() => removeCredit(index)}>Remove</button>
      </fieldset>
    {/each}
  </div>

  <div class="actions">
    <button type="button" class="secondary" disabled={rows.length >= MAX_CREDITS} onclick={addCredit}>Add artist</button>
    <div class="save-actions">
      {#if onReset}<button type="button" class="secondary" disabled={saving} onclick={() => void reset()}>Clear manual correction</button>{/if}
      <button type="button" class="primary" disabled={saving || invalid || !rows.length} onclick={() => void save()}>{saving ? "Saving…" : "Save artist credits"}</button>
    </div>
  </div>
  {#if invalid}<p class="validation" role="status">Each credit needs an artist name and role.</p>{/if}
  {#if rows.length >= MAX_CREDITS}<p class="hint" role="status">A track can have up to {MAX_CREDITS} artist credits.</p>{/if}
  {#if status}<p class="success" role="status">{status}</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</section>

<style>
  .artist-credit-editor{display:grid;gap:12px;min-width:0;padding:14px;border:1px solid var(--line);border-radius:10px;background:var(--surface);color:var(--text);font-size:13px}
  header{display:flex;align-items:flex-start;justify-content:space-between;gap:12px}
  header>div{min-width:0}
  h2{margin:0 0 4px;font-size:15px}
  p{margin:0;color:var(--muted);font-size:12px;line-height:1.45}
  .credit-list{display:grid;gap:8px;min-width:0;container-type:inline-size;container-name:artist-credits}
  .credit-row{display:grid;grid-template-columns:minmax(0,1fr) minmax(120px,.6fr) auto;align-items:end;gap:8px;min-width:0;margin:0;padding:9px;border:1px solid var(--line);border-radius:8px}
  legend{padding:0 4px;color:var(--muted);font-size:11px}
  label{display:grid;gap:4px;min-width:0;font-size:11px;color:var(--muted)}
  input{box-sizing:border-box;width:100%;min-width:0;padding:8px;border:1px solid var(--line);border-radius:6px;background:var(--base);color:var(--text);font:inherit;font-size:13px}
  input:focus-visible,button:focus-visible{outline:2px solid var(--accent);outline-offset:2px}
  button{min-height:34px;padding:6px 10px;border:1px solid var(--line);border-radius:7px;background:var(--surface);color:var(--text);font:inherit;cursor:pointer}
  button:disabled{opacity:.5;cursor:not-allowed}
  button.primary{background:var(--accent);color:var(--on-accent,#111)}
  .actions{display:flex;justify-content:space-between;gap:8px;flex-wrap:wrap}
  .save-actions{display:flex;gap:8px;flex-wrap:wrap}
  .validation,.error{color:var(--danger,#c64b4b)}
  .success{color:var(--success,#4a9)}
  .hint{font-size:11px}
  @container artist-credits (max-width:520px){.credit-row{grid-template-columns:minmax(0,1fr) auto}.credit-row label:first-of-type{grid-column:1/-1}.credit-row label:nth-of-type(2){grid-column:1;min-width:0}.credit-row .remove{grid-column:2;justify-self:end}}
  @media(max-width:520px){.actions button{flex:1}.save-actions{width:100%}}
</style>
