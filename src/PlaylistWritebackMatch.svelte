<script lang="ts">
  import SourceLink from "./SourceLink.svelte";
  import type { Track } from "./lib/types";
  type Candidate = { candidateToken: string; title: string; artist: string; version?: string | null; durationSeconds?: number | null; url: string };
  let { bindingId, localTrackId, provider, disabled = false, call, onSelect }: {
    bindingId: string; localTrackId: string; provider: string; disabled?: boolean;
    call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
    onSelect: (localTrackId: string, candidateToken: string, label: string) => void;
  } = $props();
  let loading = $state(false);
  let searched = $state(false);
  let candidates = $state<Candidate[]>([]);
  let error = $state("");
  let generation = 0;
  $effect(() => {
    bindingId; localTrackId; provider;
    generation += 1; loading = false; searched = false; candidates = []; error = "";
    return () => { generation += 1; };
  });
  async function search() {
    if (disabled || loading) return;
    const request = ++generation;
    const binding = bindingId, track = localTrackId, targetProvider = provider;
    loading = true; error = ""; candidates = []; searched = false;
    try {
      const result = await call<{bindingId:string;localTrackId:string;provider:string;candidates:Candidate[]}>(
        "search_remote_playlist_writeback_candidates", {bindingId:binding,localTrackId:track});
      if (request !== generation || binding !== bindingId || track !== localTrackId || targetProvider !== provider) return;
      if (result.bindingId !== binding || result.localTrackId !== track || result.provider !== targetProvider || !Array.isArray(result.candidates)) throw new Error("The returned matches belong to a different recording.");
      candidates = result.candidates; searched = true;
    } catch (failure) { if (request === generation) error = `Could not find matches: ${String(failure)}`; }
    finally { if (request === generation) loading = false; }
  }
  function duration(value: number | null | undefined) {
    if (value == null || !Number.isFinite(value) || value <= 0) return "Duration unknown";
    const seconds = Math.round(value);
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2,"0")}`;
  }
  function candidateSource(candidate: Candidate): Track | null {
    try {
      const url = new URL(candidate.url);
      let id: string | null = null;
      if (provider === "youtube" && url.protocol === "https:" && ["www.youtube.com", "youtube.com", "music.youtube.com"].includes(url.hostname)
          && url.pathname === "/watch" && !url.username && !url.password && !url.port && !url.hash) {
        const values = url.searchParams.getAll("v");
        if (values.length === 1 && url.searchParams.toString() === `v=${values[0]}` && /^[A-Za-z0-9_-]{11}$/.test(values[0])) id = values[0];
      } else if (provider === "spotify" && url.protocol === "https:" && url.hostname === "open.spotify.com"
          && !url.username && !url.password && !url.port && !url.hash && !url.search) {
        const match = url.pathname.match(/^\/track\/([A-Za-z0-9]{22})\/?$/);
        if (match) id = match[1];
      }
      if (!id) return null;
      const sourceUrl = provider === "youtube" ? `https://www.youtube.com/watch?v=${id}` : `https://open.spotify.com/track/${id}`;
      return {id:`candidate:${provider}:${id}`,provider,providerId:id,sourceUrl,title:candidate.title,artist:candidate.artist,
        favorite:false,provenance:{}};
    } catch { return null; }
  }
  function candidateLabel(candidate: Candidate): string {
    const evidence = provider === "youtube" ? `YouTube byline: ${candidate.artist || "unknown"} (artist not verified)` : candidate.artist || "Artist unknown";
    return `${candidate.title || "Title unknown"} · ${evidence}${candidate.version ? ` · ${candidate.version}` : ""} · ${duration(candidate.durationSeconds)}`;
  }
</script>

<div class="writeback-match">
  <button disabled={disabled || loading} onclick={() => void search()}>{loading ? "Finding recordings…" : "Find matching recording"}</button>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if searched && !candidates.length}<p role="status">No matching candidates were found. Keep this item unresolved or change the working playlist.</p>{/if}
  {#if candidates.length}
    <p>These are search suggestions, not verified matches. Check the exact title, credited artists, version and duration before choosing. Your local tags stay unchanged.</p>
    {#if provider === "youtube"}<p role="note">YouTube search bylines can name an uploader or channel rather than the recording artist. Verify the credits on the source page.</p>{/if}
    <ul aria-label="Matching recordings">
      {#each candidates as candidate (candidate.candidateToken)}
        {@const sourceTrack = candidateSource(candidate)}
        <li>
          <span><strong>{candidate.title || "Title unknown"}</strong> · {provider === "youtube" ? `YouTube byline: ${candidate.artist || "unknown"} (artist not verified)` : candidate.artist || "Artist unknown"}{candidate.version ? ` · ${candidate.version}` : ""} · {duration(candidate.durationSeconds)}</span>
          {#if !candidate.title?.trim()}<small role="note">Title missing; verify manually.</small>{/if}
          {#if !candidate.artist?.trim() || /^(unknown artist|unknown|n\/a)$/i.test(candidate.artist.trim())}<small role="note">Artist credit missing or weak; verify manually.</small>{/if}
          {#if candidate.durationSeconds == null || !Number.isFinite(candidate.durationSeconds) || candidate.durationSeconds <= 0}<small role="note">Duration is missing; verify manually.</small>{/if}
          <div class="candidate-actions">
            {#if sourceTrack}<SourceLink track={sourceTrack} compact={false} trailingArrow={true} />{/if}
            <button aria-label={`Use ${candidateLabel(candidate)}`} disabled={disabled || loading} onclick={() => onSelect(localTrackId,candidate.candidateToken,candidateLabel(candidate))}>Use this recording</button>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .writeback-match { display:grid; gap:6px; min-width:0; margin-top:6px; }
  button { padding:6px 9px; justify-self:start; font-size:11px; }
  p,small { margin:0; color:var(--muted); font-size:11px; }
  ul { display:grid; gap:6px; list-style:none; padding:0; margin:0; }
  li { display:grid; gap:5px; overflow-wrap:anywhere; }
  .candidate-actions { display:flex; flex-wrap:wrap; align-items:center; gap:6px; }
</style>
