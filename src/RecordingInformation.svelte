<script lang="ts">
  import { territoryLabel } from "./lib/territory-label";
  import { untrack } from "svelte";
  import { call } from "./lib/api";
  import type { Track } from "./lib/types";
  import type { ArtistRef } from "./lib/artist-types";
  import type { RecordingInformationResult, RecordingCatalogInfo, RecordingReleaseInfo } from "./lib/recording-information";
  let {track, loadRecording, onRelease, onArtist, compact = false}: {
    track: Track; loadRecording: (track: Track) => Promise<RecordingInformationResult>;
    onRelease?: (release: RecordingReleaseInfo, artist: ArtistRef | null) => void;
    onArtist?: (artist: ArtistRef) => void; compact?: boolean;
  } = $props();
  let result = $state<RecordingInformationResult | null>(null);
  let selection = $state<RecordingCatalogInfo | null>(null);
  let loading = $state(false);
  let failed = $state(false);
  let importStatus = $state("");
  let importedOverride = $state<Record<string,unknown> | null>(null);
  let playbackOverride = $state<string | null>(null);
  const evidence = $derived((track.provenance ?? {}) as Record<string,unknown>);
  const imported = $derived(importedOverride ?? (evidence.importMetadata ?? {}) as Record<string,unknown>);
  const canEnrich = $derived([track.sourceUrl, imported.albumUrl].some(url=>typeof url==="string" && /^https:\/\/[^/]+\.bandcamp\.com\/(album|track)\//.test(url)));
  const pageLinks = $derived([...[{label:"Album page",url:imported.albumUrl},{label:"Artist page",url:imported.artistUrl},{label:"Channel page",url:imported.channelUrl}],...(Array.isArray(imported.artistUrls)?imported.artistUrls.map(url=>({label:"Artist page",url})):[])].filter((item,index,items)=>typeof item.url==="string" && /^https:\/\//.test(item.url) && items.findIndex(other=>other.url===item.url)===index));
  async function openSourcePage(event: MouseEvent, url: string) {
    event.preventDefault();
    try {await call("open_external_url",{url});} catch(error) {importStatus=String(error);}
  }
  async function enrichImport() {
    importStatus="Reading Bandcamp information…";
    try {const value=await call<{metadata:Record<string,unknown>}>("enrich_import_track",{trackId:track.id});importedOverride=value.metadata;importStatus="Missing information updated.";}catch(error){importStatus=String(error);}
  }
  async function changePlaybackMode(mode:string) {
    try {await call("set_import_playback_mode",{trackId:track.id,mode});playbackOverride=mode;importStatus="Playback preference saved for the next play.";}catch(error){importStatus=String(error);}
  }
  const identity = $derived(`${track.id}\n${track.artist}\n${track.title}\n${track.isrc ?? ""}`);
  const recording = $derived(result?.recording ?? selection);
  $effect(() => {
    identity;
    const current = untrack(() => track);
    result = null; selection = null; loading = true; failed = false; importedOverride=null; playbackOverride=null; importStatus="";
    let cancelled = false;
    void loadRecording(current).then(value => {if (!cancelled) result = value;}).catch(() => {if (!cancelled) failed = true;}).finally(() => {if (!cancelled) loading = false;});
    return () => {cancelled = true;};
  });
</script>
<section class="recording-information" class:compact aria-label="Recording catalog information">
  <h2>Song information</h2>
  {#if pageLinks.length || Object.keys(imported).length}<details><summary>Imported source information</summary>
    <nav aria-label="Original source pages">{#each pageLinks as item}<a href={String(item.url)} target="_blank" rel="noopener noreferrer" onclick={event=>void openSourcePage(event,String(item.url))}>{item.label} ↗</a>{/each}</nav>
    {#if imported.albumArtist}<p>Album artist: {String(imported.albumArtist)}</p>{/if}
    {#if imported.location}<p>{String(imported.location)}</p>{/if}
    {#if imported.description}<p>{String(imported.description)}</p>{/if}
    {#if imported.credits}<h3>Credits</h3><p>{String(imported.credits)}</p>{/if}
    {#if imported.artistBio}<details><summary>Artist biography</summary><p>{String(imported.artistBio)}</p></details>{/if}
    {#if imported.lyrics}<details><summary>Source lyrics</summary><p style="white-space:pre-wrap">{String(imported.lyrics)}</p></details>{/if}
    {#if Array.isArray(imported.tags) && imported.tags.length}<p>Source tags: {imported.tags.join(" · ")}</p>{/if}
    {#if imported.plays != null || imported.listeners != null}<p>{imported.plays != null ? `${imported.plays} plays` : ""}{imported.listeners != null ? ` · ${imported.listeners} listeners` : ""}</p>{/if}
    {#if imported.videoTitle}<p>Video: {String(imported.videoTitle)}{imported.channelName ? ` · Channel: ${imported.channelName}` : ""}</p>{/if}
    {#if imported.availability==="unavailable"}<p>This recording was unavailable on the source page.</p>{/if}
    {#if canEnrich}<button onclick={()=>void enrichImport()}>Fill missing Bandcamp information</button>{/if}
  </details>{/if}
  {#if !track.playbackRange && ["youtube","soundcloud","bandcamp"].includes(track.provider)}
    <div class="playback-preference">
      <label>Audio source<select aria-label="Playback source preference" value={playbackOverride ?? (evidence.playbackMode as string ?? "source")} onchange={event=>void changePlaybackMode(event.currentTarget.value)}><option value="source">Keep this exact upload</option><option value="lookup">Allow matching sources</option></select></label>
      <p>Applies next time you play. Keep the exact upload for mixes, sets, and live performances.</p>
    </div>
  {/if}
  {#if importStatus}<p role="status">{importStatus}</p>{/if}
  {#if recording}
    {#if !result?.recording}<p class="notice">Catalog suggestion · saved track information is unchanged.</p>{/if}
    <strong>{recording.title}{recording.version ? ` (${recording.version})` : ""}</strong>
    {#if recording.recordingArtist && onArtist}<button class="artist" onclick={() => onArtist?.(recording!.recordingArtist!.artist)}>{recording.recordingArtist.name}</button>{:else if recording.artist}<p>{recording.artist}</p>{/if}
    {#if recording.firstReleaseDate}<p>Catalog release date {recording.firstReleaseDate}</p>{/if}
    {#if recording.genres.length}<p>{recording.genres.join(" · ")}</p>{/if}
    {#if recording.releases.length}<h3>Appears on</h3><ul>{#each recording.releases as release (release.id)}<li><button class="release" disabled={!onRelease} onclick={() => onRelease?.(release, recording?.recordingArtist?.artist ?? null)}><strong>{release.title}</strong>{#if release.date || release.country}<small>{[release.date, territoryLabel(release.country)].filter(Boolean).join(" · ")}</small>{/if}</button></li>{/each}</ul>{/if}
    <small class="attribution">MusicBrainz catalog information</small>
  {:else if loading}<p role="status">Looking up song and album information…</p>
  {:else if failed}<p>Catalog information is temporarily unavailable.</p>
  {:else if !result?.candidates.length}<p>No catalog information found yet.</p>{/if}
  {#if !result?.recording && result?.candidates.length}<details><summary>{result.resolution === "ambiguous_isrc" ? "Choose a catalog recording" : "Possible catalog recordings"} ({result.candidates.length})</summary>{#each result.candidates as candidate (candidate.recording.id)}<button class="candidate" onclick={() => selection = candidate.recording}>{candidate.recording.title}{candidate.recording.version ? ` (${candidate.recording.version})` : ""}<small>{candidate.recording.artist ?? "Artist unknown"}</small></button>{/each}</details>{/if}
</section>
<style>
  .recording-information{margin-top:26px}nav{display:flex;flex-wrap:wrap;gap:12px;margin:12px 0}nav a{color:var(--accent);font-size:13px}.playback-preference{margin:14px 0}.playback-preference label{display:grid;gap:6px;margin:0;color:var(--muted);font-size:11px}.playback-preference select{width:100%;min-width:0;max-width:100%;font-size:12px}.playback-preference p{margin:6px 0 0;font-size:11px;line-height:1.4}h2{font-size:24px;margin:0 0 18px}h3{font-size:15px;margin:22px 0 10px}p{color:var(--muted);font-size:14px;line-height:1.5}.artist{display:block;margin-top:8px;background:transparent;border:0;padding:0;color:var(--text);text-align:left;text-decoration:underline}ul{list-style:none;padding:0;margin:0;display:grid;gap:7px;max-height:300px;overflow:auto}.release,.candidate{width:100%;display:grid;gap:5px;text-align:left;padding:9px 10px;border:1px solid var(--line);border-radius:7px;background:var(--raised);color:var(--text);overflow-wrap:anywhere}.release:disabled{opacity:1}.candidate{margin-top:8px}.release small,.candidate small,.attribution{color:var(--muted)}.attribution{display:block;margin-top:14px;font-size:11px}summary{cursor:pointer;color:var(--accent);margin-top:16px}.notice{font-size:12px}.compact h2{font-size:15px}.compact p,.compact button{font-size:12px}button:focus-visible,summary:focus-visible{outline:2px solid var(--accent);outline-offset:3px}
</style>
