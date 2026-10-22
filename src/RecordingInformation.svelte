<script lang="ts">
  import { untrack } from "svelte";
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
  const identity = $derived(`${track.id}\n${track.artist}\n${track.title}\n${track.isrc ?? ""}`);
  const recording = $derived(result?.recording ?? selection);
  $effect(() => {
    identity;
    const current = untrack(() => track);
    result = null; selection = null; loading = true; failed = false;
    let cancelled = false;
    void loadRecording(current).then(value => {if (!cancelled) result = value;}).catch(() => {if (!cancelled) failed = true;}).finally(() => {if (!cancelled) loading = false;});
    return () => {cancelled = true;};
  });
</script>
<section class="recording-information" class:compact aria-label="Recording catalog information">
  <h2>Song information</h2>
  {#if recording}
    {#if !result?.recording}<p class="notice">Catalog suggestion · saved track information is unchanged.</p>{/if}
    <strong>{recording.title}{recording.version ? ` (${recording.version})` : ""}</strong>
    {#if recording.recordingArtist && onArtist}<button class="artist" onclick={() => onArtist?.(recording!.recordingArtist!.artist)}>{recording.recordingArtist.name}</button>{:else if recording.artist}<p>{recording.artist}</p>{/if}
    {#if recording.firstReleaseDate}<p>Catalog release date {recording.firstReleaseDate}</p>{/if}
    {#if recording.genres.length}<p>{recording.genres.join(" · ")}</p>{/if}
    {#if recording.releases.length}<h3>Appears on</h3><ul>{#each recording.releases as release (release.id)}<li><button class="release" disabled={!onRelease} onclick={() => onRelease?.(release, recording?.recordingArtist?.artist ?? null)}><strong>{release.title}</strong>{#if release.date || release.country}<small>{[release.date, release.country].filter(Boolean).join(" · ")}</small>{/if}</button></li>{/each}</ul>{/if}
    <small class="attribution">MusicBrainz catalog information</small>
  {:else if loading}<p role="status">Looking up song and album information…</p>
  {:else if failed}<p>Catalog information is temporarily unavailable.</p>
  {:else if !result?.candidates.length}<p>No catalog information found yet.</p>{/if}
  {#if !result?.recording && result?.candidates.length}<details><summary>{result.resolution === "ambiguous_isrc" ? "Choose a catalog recording" : "Possible catalog recordings"} ({result.candidates.length})</summary>{#each result.candidates as candidate (candidate.recording.id)}<button class="candidate" onclick={() => selection = candidate.recording}>{candidate.recording.title}{candidate.recording.version ? ` (${candidate.recording.version})` : ""}<small>{candidate.recording.artist ?? "Artist unknown"}</small></button>{/each}</details>{/if}
</section>
<style>
  .recording-information{margin-top:26px}h2{font-size:24px;margin:0 0 18px}h3{font-size:15px;margin:22px 0 10px}p{color:var(--muted);font-size:14px;line-height:1.5}.artist{display:block;margin-top:8px;background:transparent;border:0;padding:0;color:var(--text);text-align:left;text-decoration:underline}ul{list-style:none;padding:0;margin:0;display:grid;gap:7px;max-height:300px;overflow:auto}.release,.candidate{width:100%;display:grid;gap:5px;text-align:left;padding:9px 10px;border:1px solid var(--line);border-radius:7px;background:var(--raised);color:var(--text);overflow-wrap:anywhere}.release:disabled{opacity:1}.candidate{margin-top:8px}.release small,.candidate small,.attribution{color:var(--muted)}.attribution{display:block;margin-top:14px;font-size:11px}summary{cursor:pointer;color:var(--accent);margin-top:16px}.notice{font-size:12px}.compact h2{font-size:15px}.compact p,.compact button{font-size:12px}button:focus-visible,summary:focus-visible{outline:2px solid var(--accent);outline-offset:3px}
</style>
