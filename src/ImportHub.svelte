<script lang="ts">
  export type ImportChoice = "playlist-file" | "paste-playlist" | "import-url" | "local-audio" | "restore-backup";

  let { onChoose, busyChoice = null }: { onChoose: (choice: ImportChoice) => void; busyChoice?: ImportChoice | null } = $props();

  const choices: { id: ImportChoice; title: string; detail: string }[] = [
    { id: "playlist-file", title: "Choose a playlist file", detail: "Bring in a playlist saved on this device." },
    { id: "paste-playlist", title: "Paste a tracklist", detail: "Add songs from text copied from anywhere." },
    { id: "import-url", title: "Import a playlist or album link", detail: "Bring in songs from a link or chart." },
    { id: "local-audio", title: "Add music files or a folder", detail: "Add audio stored on this device." },
  ];
</script>

<section class="import-hub" aria-labelledby="import-hub-title">
  <header>
    <h2 id="import-hub-title">Import</h2>
    <p>Choose what you want to bring into Burrata.</p>
  </header>
  <div class="choices">
    {#each choices as choice (choice.id)}
      <button type="button" class="choice" disabled={busyChoice !== null} aria-busy={busyChoice === choice.id} onclick={() => onChoose(choice.id)}>
        <span class="choice-copy"><strong>{choice.title}</strong><small>{choice.detail}</small></span>
        <span class="arrow" aria-hidden="true">›</span>
      </button>
    {/each}
  </div>
</section>

<style>
  .import-hub{display:grid;gap:14px;max-width:760px;color:var(--text)}
  header{display:block;margin:0;padding:0;border:0}
  h2,p{margin:0}h2{font-size:1.25rem}header p{margin-top:4px;color:var(--muted);font-size:.88rem}
  .choices{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,260px),1fr));gap:9px;min-width:0}
  .choice{display:flex;align-items:center;justify-content:space-between;gap:12px;min-height:76px;padding:12px;border:1px solid var(--line);border-radius:9px;background:var(--surface);color:var(--text);text-align:left;font:inherit;cursor:pointer}
  .choice:hover:not(:disabled),.choice:focus-visible{border-color:var(--accent);background:var(--surface-2)}.choice:disabled{opacity:.55;cursor:default}
  .choice-copy{display:grid;gap:5px;min-width:0;overflow-wrap:anywhere}.choice-copy strong{font-size:.9rem}.choice-copy small{color:var(--muted);font-size:.79rem;line-height:1.4}.arrow{flex:0 0 auto;color:var(--accent);font-size:1.5rem}
</style>
