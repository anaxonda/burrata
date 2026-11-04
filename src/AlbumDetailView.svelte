<script lang="ts">
  import type { ComponentProps, Snippet } from "svelte";
  import { onDestroy } from "svelte";
  import { call } from "./lib/api";
  import TrackList from "./TrackList.svelte";
  import type { AlbumDetailMetadata, AlbumDetailSelectOption, AlbumLoudnessAnalysisStart, AlbumLoudnessAnalysisStatus } from "./lib/album-detail-view";
  import type { LoudnessResourceIdentity } from "./lib/loudness-normalization";
  import type { Track } from "./lib/types";

  type SharedTrackListProps = Omit<ComponentProps<typeof TrackList>, "presentation">;
  let {
    metadata, albumTracks, resolveAlbumTracks, actionTrackCount = albumTracks.length, analysisScopeComplete = true, analysisUnavailableReason = "Album analysis requires the complete album track list.", analysisIdentity, loading = false, ownsScroll = true, onArtistClick,
    onPlayAlbum, onQueueAlbum, playAlbumLabel = "▶ Play album", queueAlbumLabel = "＋ Queue album", onDownloadAlbum, downloadAlbumLabel = "Download album", onSearchAlbumSoulseek, soulseekSearchAvailable = false,
    onAddAlbumToPlaylist, onSaveAlbum, saveAlbumLabel = "Save album", onSaveLoadedTracks, saveLoadedTracksLabel = "Save loaded tracks", saving = false,
    onAnalyzeLoudness, analysisTargetLabel = "this album", trackListProps,
    filterValue = "", onFilterChange, sortValue = "sourceOrder", sortOptions = [], onSortChange,
    sortDirection = "asc", onDirectionChange, availabilityValue = "all", availabilityOptions = [], onAvailabilityChange,
    minDuration = "", maxDuration = "", onDurationChange,
    headerActions, moreActions, notices, actionStatus = "", visibleTrackCount = 0, loadingMessage = "Loading tracks…", emptyMessage = "No tracks are available.",
  }: {
    metadata: AlbumDetailMetadata; albumTracks: Track[]; resolveAlbumTracks?: () => Promise<Track[]>; actionTrackCount?: number; analysisScopeComplete?: boolean; analysisUnavailableReason?: string; analysisIdentity: string; loading?: boolean; ownsScroll?: boolean;
    onArtistClick?: (artist: string) => void;
    onPlayAlbum: (tracks: Track[], resource: LoudnessResourceIdentity | null) => void | Promise<void>;
    onQueueAlbum: (tracks: Track[], resource: LoudnessResourceIdentity | null) => void | Promise<void>;
    playAlbumLabel?: string; queueAlbumLabel?: string; onDownloadAlbum?: (tracks: Track[]) => unknown | Promise<unknown>; downloadAlbumLabel?: string;
    onSearchAlbumSoulseek?: () => unknown | Promise<unknown>; soulseekSearchAvailable?: boolean;
    onAddAlbumToPlaylist?: (tracks: Track[], title: string) => void | Promise<void>;
    onSaveAlbum?: (tracks: Track[]) => void | Promise<void>; saveAlbumLabel?: string;
    onSaveLoadedTracks?: (tracks: Track[]) => void | Promise<void>; saveLoadedTracksLabel?: string; saving?: boolean;
    onAnalyzeLoudness?: (tracks: Track[], gapSeconds: number) => Promise<AlbumLoudnessAnalysisStart>;
    analysisTargetLabel?: string;
    trackListProps: SharedTrackListProps;
    filterValue?: string; onFilterChange?: (value: string) => void;
    sortValue?: string; sortOptions?: AlbumDetailSelectOption[]; onSortChange?: (value: string) => void;
    sortDirection?: "asc" | "desc"; onDirectionChange?: (value: "asc" | "desc") => void;
    availabilityValue?: string; availabilityOptions?: AlbumDetailSelectOption[]; onAvailabilityChange?: (value: string) => void;
    minDuration?: string; maxDuration?: string; onDurationChange?: (minimum: string, maximum: string) => void;
    headerActions?: Snippet; moreActions?: Snippet; notices?: Snippet; actionStatus?: string;
    visibleTrackCount?: number; loadingMessage?: string; emptyMessage?: string;
  } = $props();

  let downloadActions = $state<HTMLDetailsElement | null>(null);
  let moreMenu = $state<HTMLDetailsElement | null>(null);
  let loudnessAnalysisJobId = $state("");
  let loudnessAnalysisState = $state<"idle" | "running" | "completed" | "failed" | "cancelled">("idle");
  let loudnessAnalysisProgress = $state("");
  let loudnessAnalysisError = $state("");
  let loudnessAlbumResource = $state<LoudnessResourceIdentity | null>(null);
  let loudnessPlaybackTracks = $state<Track[]>([]);
  let loudnessAnalysisGeneration = 0;
  let activeAnalysisIdentity = "";
  let localActionStatus = $state("");
  let actionOwnerGeneration = 0;
  let destroyed = false;
  const visibleCount = $derived(visibleTrackCount || trackListProps.tracks.length);

  async function resolveActionTracks(): Promise<Track[]> {
    const owner = actionOwnerGeneration;
    const identity = analysisIdentity;
    const tracks = resolveAlbumTracks ? await resolveAlbumTracks() : albumTracks;
    if (owner !== actionOwnerGeneration || identity !== analysisIdentity) throw new Error("This album changed while loading its tracks. Try again.");
    return tracks;
  }

  function resetAnalysis(identity: string) {
    if (identity === activeAnalysisIdentity) return;
    activeAnalysisIdentity = identity;
    actionOwnerGeneration++;
    localActionStatus = "";
    if (loudnessAnalysisJobId) void call("cancel_loudness_analysis", { jobId: loudnessAnalysisJobId }).catch(() => {});
    loudnessAnalysisGeneration++;
    loudnessAnalysisJobId = ""; loudnessAnalysisState = "idle"; loudnessAnalysisProgress = "";
    loudnessAnalysisError = ""; loudnessAlbumResource = null; loudnessPlaybackTracks = [];
  }
  $effect(() => resetAnalysis(analysisIdentity));
  onDestroy(() => {
    destroyed = true;
    const jobId = loudnessAnalysisJobId;
    actionOwnerGeneration++;
    loudnessAnalysisGeneration++;
    if (jobId) void call("cancel_loudness_analysis", { jobId }).catch(() => {});
  });

  function closeMenus(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    const activeMenu = [downloadActions, moreMenu].find(menu => menu?.open && menu.contains(event.target as Node));
    if (!activeMenu) return;
    event.preventDefault();
    activeMenu.open = false;
    activeMenu.querySelector<HTMLElement>("summary")?.focus();
  }
  function openMenu(which: "download" | "more") {
    if (which === "download" && moreMenu) moreMenu.open = false;
    if (which === "more" && downloadActions) downloadActions.open = false;
    const active = which === "download" ? downloadActions : moreMenu;
    requestAnimationFrame(() => fitMenuToDetail(active));
  }
  function fitMenuToDetail(menu: HTMLDetailsElement | null) {
    if (!menu?.open) return;
    const panel = menu.querySelector<HTMLElement>(".menu-content");
    const detail = menu.closest<HTMLElement>(".album-detail");
    if (!panel || !detail) return;
    panel.style.left = "auto";
    panel.style.right = "0px";
    const panelRect = panel.getBoundingClientRect();
    const menuRect = menu.getBoundingClientRect();
    const detailRect = detail.getBoundingClientRect();
    const leftEdge = detailRect.left + 8;
    const rightEdge = detailRect.left + detail.clientWidth - 8;
    if (panelRect.left < leftEdge) {
      panel.style.left = `${leftEdge - menuRect.left}px`;
      panel.style.right = "auto";
    } else if (panelRect.right > rightEdge) {
      panel.style.right = `${menuRect.right - rightEdge}px`;
    }
  }
  function refitOpenMenus() {
    fitMenuToDetail(downloadActions);
    fitMenuToDetail(moreMenu);
  }

  async function analyzeAlbumLoudness() {
    if (!onAnalyzeLoudness || !analysisScopeComplete || loading || actionTrackCount < 2 || loudnessAnalysisState === "running") return;
    const generation = ++loudnessAnalysisGeneration;
    loudnessAnalysisState = "running"; loudnessAnalysisError = "";
    loudnessAnalysisProgress = "Preparing selected local tracks…";
    loudnessAlbumResource = null; loudnessPlaybackTracks = [];
    try {
      const actionTracks = await resolveActionTracks();
      if (generation !== loudnessAnalysisGeneration) return;
      if (!analysisScopeComplete) throw new Error(analysisUnavailableReason);
      if (actionTracks.length < 2) throw new Error("At least two complete album tracks are required for analysis.");
      const started = await onAnalyzeLoudness(actionTracks, 0);
      if (generation !== loudnessAnalysisGeneration) {
        void call("cancel_loudness_analysis", { jobId: started.jobId }).catch(() => {});
        return;
      }
      loudnessAnalysisJobId = started.jobId;
      loudnessPlaybackTracks = started.playbackTracks;
      while (generation === loudnessAnalysisGeneration) {
        const status = await call<AlbumLoudnessAnalysisStatus>("get_loudness_analysis_status", { jobId: started.jobId });
        if (generation !== loudnessAnalysisGeneration) return;
        loudnessAnalysisProgress = `${status.completed} of ${status.total} tracks analyzed`;
        if (status.status === "completed") {
          if (!status.resource || status.resource.scope !== "album") throw new Error("Album analysis completed without its exact programme identity.");
          loudnessAlbumResource = status.resource;
          loudnessAnalysisState = "completed";
          loudnessAnalysisProgress = `Album programme analyzed with no added inter-track silence. Play or queue ${analysisTargetLabel} to use its measured album gain.`;
          loudnessAnalysisJobId = "";
          return;
        }
        if (status.status === "failed") throw new Error(status.error || "Album loudness analysis failed.");
        if (status.status === "cancelled") {
          loudnessAnalysisState = "cancelled"; loudnessAnalysisProgress = "Album analysis cancelled."; loudnessAnalysisJobId = ""; return;
        }
        await new Promise(resolve => setTimeout(resolve, 700));
      }
    } catch (failure) {
      if (generation === loudnessAnalysisGeneration) {
        loudnessAnalysisState = "failed";
        loudnessAnalysisError = failure instanceof Error ? failure.message : String(failure);
        loudnessAnalysisProgress = ""; loudnessAnalysisJobId = "";
      }
    }
  }
  async function cancelAlbumLoudnessAnalysis() {
    const jobId = loudnessAnalysisJobId;
    loudnessAnalysisGeneration++;
    loudnessAnalysisJobId = ""; loudnessAnalysisState = "cancelled";
    loudnessAnalysisProgress = "Album analysis cancelled.";
    if (jobId) await call("cancel_loudness_analysis", { jobId }).catch(() => {});
  }
  async function playAlbum() {
    localActionStatus = "";
    try {
      const actionTracks = await resolveActionTracks();
      const selectedTracks = loudnessAlbumResource ? loudnessPlaybackTracks : actionTracks;
      await onPlayAlbum(selectedTracks, loudnessAlbumResource);
    } catch (failure) { if (!destroyed) localActionStatus = `Could not play album: ${failure instanceof Error ? failure.message : String(failure)}`; }
  }
  async function queueAlbum() {
    localActionStatus = "";
    try {
      const actionTracks = await resolveActionTracks();
      const selectedTracks = loudnessAlbumResource ? loudnessPlaybackTracks : actionTracks;
      await onQueueAlbum(selectedTracks, loudnessAlbumResource);
    } catch (failure) { if (!destroyed) localActionStatus = `Could not queue album: ${failure instanceof Error ? failure.message : String(failure)}`; }
  }
  function closeDownloadMenu() { if (downloadActions) downloadActions.open = false; }
  async function downloadAlbum() {
    if (!onDownloadAlbum || loading || actionTrackCount < 1) return;
    localActionStatus = "";
    try {
      const actionTracks = await resolveActionTracks();
      const receipt = await onDownloadAlbum(actionTracks);
      const result = receipt && typeof receipt === "object" ? receipt as { status?: string; queuedJobs?: unknown[]; failed?: Array<{reason?:string}>; skipped?: unknown[] } : null;
      if (result?.status === "failed" || result?.status === "busy" || result?.status === "empty") throw new Error(result.failed?.[0]?.reason || "No album tracks could be prepared.");
      localActionStatus = result ? `Album request finished: ${result.queuedJobs?.length ?? 0} queued${result.skipped?.length ? `, ${result.skipped.length} need review` : ""}.` : "Album download request sent.";
      closeDownloadMenu();
    } catch (failure) { if (!destroyed) localActionStatus = `Album download failed: ${failure instanceof Error ? failure.message : String(failure)}`; }
  }
  async function searchSoulseek() {
    if (!onSearchAlbumSoulseek || !soulseekSearchAvailable) return;
    try { await onSearchAlbumSoulseek(); localActionStatus = "Opened Soulseek album search."; closeDownloadMenu(); }
    catch (failure) { if (!destroyed) localActionStatus = `Soulseek album search failed: ${failure instanceof Error ? failure.message : String(failure)}`; }
  }
  async function runAlbumCallback(label: string, callback: (tracks: Track[]) => unknown | Promise<unknown>) {
    localActionStatus = "";
    try { await callback(await resolveActionTracks()); }
    catch (failure) { if (!destroyed) localActionStatus = `${label}: ${failure instanceof Error ? failure.message : String(failure)}`; }
  }
</script>

<svelte:window onkeydown={closeMenus} onresize={refitOpenMenus} />

<section class="album-detail" class:owns-scroll={ownsScroll} aria-label={`${metadata.title} album`}>
  <header class="album-hero">
    {#if metadata.artworkUrl}<img class="cover" src={metadata.artworkUrl} alt="" />{:else}<div class="cover placeholder" aria-hidden="true">♫</div>{/if}
    <div class="hero-copy">
      {#if metadata.sourceLabel}<p class="eyebrow">{metadata.sourceLabel}</p>{/if}
      <h1>{metadata.title}</h1>
      {#if metadata.artistNames?.length}<p class="artists">{#each metadata.artistNames as artist, index}<span>{#if index > 0}<span aria-hidden="true"> · </span>{/if}{#if onArtistClick}<button class="artist-credit" onclick={() => onArtistClick?.(artist)}>{artist}</button>{:else}{artist}{/if}</span>{/each}</p>{:else if metadata.artist}<p class="artists">{metadata.artist}</p>{/if}
      {#if metadata.year || metadata.scopeLabel}<p class="album-meta release-meta">{metadata.year ?? ""}{#if metadata.scopeLabel}<span>{metadata.year ? " · " : ""}{metadata.scopeLabel}</span>{/if}</p>{/if}
      {#if metadata.sourceUrl}<a class="album-source" href={metadata.sourceUrl} target="_blank" rel="noreferrer">{metadata.sourceLabel ? `Open ${metadata.sourceLabel} source ↗` : "Open source ↗"}</a>{/if}
      {@render headerActions?.()}
      <div class="hero-actions">
        <button class="primary" disabled={actionTrackCount < 1 || loading} onclick={() => void playAlbum()}>{playAlbumLabel}</button>
        <button disabled={actionTrackCount < 1 || loading} onclick={() => void queueAlbum()}>{queueAlbumLabel}</button>
        {#if onDownloadAlbum || onSearchAlbumSoulseek}<details class="album-menu album-actions-menu download-menu" bind:this={downloadActions} ontoggle={() => { if (downloadActions?.open) openMenu("download"); }}>
          <summary aria-label={`Download album options for ${metadata.title}`}>{downloadAlbumLabel} ▾</summary>
          <div class="menu-content" role="group" aria-label={`Download options for ${metadata.title}`}>
            {#if onDownloadAlbum}<button class="download-album-action" type="button" disabled={actionTrackCount < 1 || loading} onclick={() => void downloadAlbum()}>{downloadAlbumLabel}</button>{/if}
            {#if onSearchAlbumSoulseek}<button type="button" aria-label={`Search album ${metadata.title} on Soulseek`} disabled={!soulseekSearchAvailable} title={!soulseekSearchAvailable ? "Enable and configure Soulseek in Settings → Connections to search albums." : undefined} onclick={() => void searchSoulseek()}>Search album on Soulseek</button>{#if !soulseekSearchAvailable}<small>Enable and configure Soulseek in Settings → Connections.</small>{/if}{/if}
          </div>
        </details>{/if}
        {#if onAddAlbumToPlaylist}<button class="add-album-playlist" disabled={actionTrackCount < 1 || loading} onclick={() => void runAlbumCallback("Could not add album to playlist", items => onAddAlbumToPlaylist?.(items, metadata.title))}>Add to playlist</button>{/if}
        {#if onSaveAlbum}<button disabled={actionTrackCount < 1 || loading || saving} onclick={() => void runAlbumCallback("Could not save album", items => onSaveAlbum?.(items))}>{saveAlbumLabel}</button>{/if}
        {#if onSaveLoadedTracks}<button class="save-album" disabled={actionTrackCount < 1 || loading || saving} onclick={() => void runAlbumCallback("Could not save tracks", items => onSaveLoadedTracks?.(items))}>{saving ? "Saving to Library…" : saveLoadedTracksLabel}</button>{/if}
        {#if onAnalyzeLoudness || moreActions}<details class="album-menu album-actions-menu more-menu" bind:this={moreMenu} ontoggle={() => { if (moreMenu?.open) openMenu("more"); }}>
          <summary aria-label={`More album actions for ${metadata.title}`} title="More album actions">⋯</summary>
          <div class="menu-content more-content" role="group" aria-label={`More album actions for ${metadata.title}`}>
            {#if onAnalyzeLoudness}<section class="album-loudness-analysis" aria-label="Album loudness analysis"><strong>Album loudness</strong>
              <p>Requires local files for every track. Download or import the tracks first. Analysis measures the release without added inter-track silence.</p>
              {#if !analysisScopeComplete}<p role="note">{analysisUnavailableReason}</p>{/if}
              <button disabled={!analysisScopeComplete || actionTrackCount < 2 || loading || loudnessAnalysisState === "running"} title={!analysisScopeComplete ? analysisUnavailableReason : undefined} onclick={() => void analyzeAlbumLoudness()}>{loudnessAnalysisState === "running" ? "Analyzing album…" : loudnessAlbumResource ? "Reanalyze album loudness" : "Analyze album loudness"}</button>
              {#if loudnessAnalysisState === "running"}<button onclick={() => void cancelAlbumLoudnessAnalysis()}>Cancel analysis</button>{/if}
              {#if loudnessAnalysisProgress}<p role="status">{loudnessAnalysisProgress}</p>{/if}
              {#if loudnessAnalysisError}<p role="alert">{loudnessAnalysisError}</p>{/if}
            </section>
            {/if}
            {@render moreActions?.()}
          </div>
        </details>{/if}
      </div>
      {#if actionStatus || localActionStatus}<p class="album-action-status" role="status">{localActionStatus || actionStatus}</p>{/if}
      {#if metadata.trackCountLabel}<p class="album-track-scope">{metadata.trackCountLabel}</p>{/if}
    </div>
  </header>
  <div class="track-heading"><h2>Tracks <span>{visibleCount}</span></h2><div class="track-controls">
    <input aria-label="Filter tracks" placeholder="Filter tracks" value={filterValue} oninput={event => onFilterChange?.(event.currentTarget.value)} />
    {#if sortOptions.length}<select aria-label="Sort tracks" value={sortValue} onchange={event => onSortChange?.(event.currentTarget.value)}>{#each sortOptions as option (option.value)}<option value={option.value} disabled={option.disabled}>{option.label}</option>{/each}</select>{/if}
    {#if onDirectionChange}<select aria-label="Sort direction" value={sortDirection} onchange={event => onDirectionChange?.(event.currentTarget.value as "asc" | "desc")}><option value="asc">Ascending</option><option value="desc">Descending</option></select>{/if}
    {#if availabilityOptions.length}<select aria-label="Track availability" value={availabilityValue} onchange={event => onAvailabilityChange?.(event.currentTarget.value)}>{#each availabilityOptions as option (option.value)}<option value={option.value} disabled={option.disabled}>{option.label}</option>{/each}</select>{/if}
    {#if onDurationChange}<label>From <input aria-label="Minimum track duration" placeholder="mm:ss" value={minDuration} oninput={event => onDurationChange?.(event.currentTarget.value, maxDuration)} /></label><label>To <input aria-label="Maximum track duration" placeholder="mm:ss" value={maxDuration} oninput={event => onDurationChange?.(minDuration, event.currentTarget.value)} /></label>{/if}
  </div></div>
  {@render notices?.()}
  {#if trackListProps.tracks.length}<TrackList {...trackListProps} presentation="playlist" />{:else}<p class="empty-message" role={loading ? "status" : undefined}>{loading ? loadingMessage : emptyMessage}</p>{/if}
</section>

<style>
  .album-detail{display:flex;flex-direction:column;gap:1rem;min-width:0;min-height:0;padding:1rem;color:var(--text,#eee);background:var(--panel,#17191c);border-radius:12px;box-sizing:border-box}.album-detail.owns-scroll{height:100%;overflow:auto;flex:1}.album-detail:not(.owns-scroll){height:auto;overflow:visible}.album-detail button,.album-detail input,.album-detail select{font:inherit;color:inherit;background:var(--surface,#202328);border:1px solid var(--border,#454a50);border-radius:7px;padding:.5rem .65rem}.album-detail button{cursor:pointer}.album-detail button:disabled{opacity:.5;cursor:default}.album-hero{display:flex;align-items:flex-end;gap:1.15rem;padding:.6rem 0 1rem;border-bottom:1px solid var(--border,#383d43)}.cover{width:clamp(120px,20vw,190px);height:clamp(120px,20vw,190px);flex:none;object-fit:cover;border-radius:9px;box-shadow:0 8px 24px #0005}.placeholder{display:grid;place-items:center;background:linear-gradient(145deg,#2b4039,#20242a);color:var(--accent,#67c3a5);font-size:4rem}.hero-copy{min-width:0;flex:1}.eyebrow{margin:0;color:var(--muted,#a3a8ae);font-size:.75rem;text-transform:uppercase;letter-spacing:.1em}.hero-copy h1{margin:.35rem 0;font-size:clamp(1.6rem,4vw,2.65rem);line-height:1.1}.artists{overflow-wrap:anywhere}.artists,.album-meta{margin:.35rem 0;color:var(--muted,#a3a8ae)}.artist-credit{max-width:100%;border:0!important;background:transparent!important;padding:0!important;text-align:left;white-space:normal;overflow-wrap:anywhere;text-decoration:underline}.album-source{color:var(--muted,#a3a8ae);font-size:.8rem}.hero-actions{display:flex;gap:.45rem;margin-top:.85rem;flex-wrap:wrap}.album-detail .primary{background:var(--accent,#4c78c8)!important;border-color:var(--accent,#4c78c8)!important;color:var(--base,#101412)!important}.album-menu{position:relative}.album-menu summary{box-sizing:border-box;min-height:34px;padding:.4rem .6rem;cursor:pointer;list-style:none;white-space:nowrap;background:var(--surface,#202328);border:1px solid var(--border,#454a50);border-radius:7px;font-size:.8rem;line-height:1.2}.album-menu summary::-webkit-details-marker{display:none}.menu-content{position:absolute;z-index:5;top:calc(100% + 4px);right:0;display:grid;gap:.4rem;min-width:210px;max-width:min(330px,calc(100vw - 2rem));padding:.5rem;background:var(--panel,#17191c);border:1px solid var(--border,#454a50);border-radius:8px;box-shadow:0 8px 22px #0008}.menu-content button{text-align:left}.menu-content small{max-width:220px;color:var(--muted,#a3a8ae);font-size:.75rem}.more-content strong{font-size:.85rem}.more-content p{margin:0;color:var(--muted,#a3a8ae);font-size:.78rem;line-height:1.4}.album-loudness-analysis{display:grid;gap:.4rem}.more-menu summary{min-width:34px;text-align:center;font-size:1.2rem}.album-track-scope,.album-action-status{margin:.35rem 0 0;color:var(--muted,#a3a8ae);font-size:.82rem}.track-heading{display:flex;justify-content:space-between;align-items:flex-start;flex-wrap:wrap;gap:.75rem}.track-heading h2{margin:0;font-size:1.1rem}.track-heading h2 span{color:var(--muted,#a3a8ae);font-size:.8rem;font-weight:400}.track-controls{display:flex;flex-wrap:wrap;align-items:center;gap:.4rem;min-width:0}.track-controls input,.track-controls select{height:34px;min-height:34px;box-sizing:border-box;padding:.3rem .5rem;font-size:.8rem}.track-controls input{min-width:6.75rem}.track-controls select{color-scheme:dark}.track-controls label{display:flex;align-items:center;gap:.25rem;color:var(--muted,#a3a8ae);font-size:.8rem}.album-detail button:focus-visible,.album-detail input:focus-visible,.album-detail select:focus-visible,.album-menu summary:focus-visible{outline:2px solid var(--accent,#b8ec8c);outline-offset:2px}.track-controls input{color:var(--text,#eee)}
  @media(max-width:620px){.album-detail{padding:.7rem}.album-hero{align-items:center;gap:.75rem}.cover{width:105px;height:105px}.hero-actions button{padding:.4rem .5rem}.track-heading{align-items:flex-start;flex-direction:column}.track-controls{width:100%}.track-controls input{flex:1;min-width:0}.menu-content{max-width:calc(100vw - 1.4rem)}}
</style>
