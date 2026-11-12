<script lang="ts">
  import { territoryLabel } from "./lib/territory-label";
  import { untrack } from "svelte";
  import { call } from "./lib/api";
  import { plainTextMarkdown } from "./lib/plain-text-markdown";
  import type { Track } from "./lib/types";
  import type { ArtistRef } from "./lib/artist-types";
  import type { RecordingInformationResult, RecordingCatalogInfo, RecordingReleaseInfo } from "./lib/recording-information";
  type ContextTarget = { track: Track; label: string; targetKind: "song" | "concert" | "set" };
  type ContextResponse = { requestId: string; identity: string; answer: string; model: string; provider: "openaiCompatible"; sources?: {title:string;url:string}[] };
  let {track, loadRecording, onRelease, onArtist, compact = false, contextLookupConfigured = false, contextLookupRevision = 0, contextTargets}: {
    track: Track; loadRecording: (track: Track) => Promise<RecordingInformationResult>;
    onRelease?: (release: RecordingReleaseInfo, artist: ArtistRef | null) => void;
    onArtist?: (artist: ArtistRef) => void; compact?: boolean; contextLookupConfigured?: boolean; contextLookupRevision?: number; contextTargets?: ContextTarget[];
  } = $props();
  let result = $state<RecordingInformationResult | null>(null);
  let selection = $state<RecordingCatalogInfo | null>(null);
  let loading = $state(false);
  let failed = $state(false);
  let importStatus = $state("");
  let importedOverride = $state<Record<string,unknown> | null>(null);
  let playbackOverride = $state<string | null>(null);
  let contextAnswer = $state("");
  let contextAnswerLabel = $state("");
  let contextSources = $state<{title:string;url:string}[]>([]);
  let contextError = $state("");
  let contextPending = $state(false);
  let contextKind = $state<"song" | "concert" | "set">("song");
  let contextGeneration = 0;
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
  function trackTargetKey(value: Track, targetKind: ContextTarget["targetKind"]): string {
    const range = value.playbackRange;
    return JSON.stringify([value.id, value.provider, value.title, value.artist, value.album ?? "", value.version ?? "", value.recordingDate ?? "", value.sourceUrl ?? "", range ? [range.parentTrackId,range.chapterId,range.startTime,range.endTime] : null, targetKind]);
  }
  const defaultContextTargets = $derived.by((): ContextTarget[] => [{track, label: contextKind === "set" ? "This DJ set" : contextKind === "concert" ? "This concert" : "This song", targetKind: contextKind}]);
  const visibleContextTargets = $derived(contextTargets?.length ? contextTargets : defaultContextTargets);
  const contextScope = $derived(JSON.stringify([contextLookupConfigured, contextLookupRevision, visibleContextTargets.map(item => [trackTargetKey(item.track, item.targetKind), item.label])]));
  $effect(() => {
    contextScope;
    contextGeneration++;
    contextPending = false;
    contextAnswer = "";
    contextSources = [];
    contextAnswerLabel = "";
    contextError = "";
    return () => { contextGeneration++; };
  });
  function boundedText(value: unknown, limit: number): string | undefined {
    if (typeof value !== "string") return undefined;
    const text = value.trim();
    if (!text) return undefined;
    let bytes = 0;
    let output = "";
    const encoder = new TextEncoder();
    for (const character of text) {
      const size = encoder.encode(character).length;
      if (bytes + size > limit) break;
      output += character;
      bytes += size;
    }
    return output || undefined;
  }
  function sourceKind(value: Track): "youtube" | "soundcloud" | "bandcamp" | "local" | "radio" | "other" {
    if (["youtube", "soundcloud", "bandcamp"].includes(value.provider)) return value.provider as "youtube" | "soundcloud" | "bandcamp";
    if (value.provider.startsWith("local")) return "local";
    if (value.provider === "radio" || value.provider === "radio_song") return "radio";
    return "other";
  }
  function contextSourceUrl(value: Track): string | undefined {
    try {
      if (typeof value.sourceUrl !== "string" || value.sourceUrl.length > 2048) return undefined;
      const url = new URL(value.sourceUrl);
      if (url.protocol !== "https:" || url.username || url.password || url.port) return undefined;
      const kind = sourceKind(value);
      if (!(kind === "youtube" && ["youtube.com","www.youtube.com","music.youtube.com","youtu.be"].includes(url.hostname)
        || kind === "soundcloud" && ["soundcloud.com","www.soundcloud.com","m.soundcloud.com"].includes(url.hostname)
        || kind === "bandcamp" && url.hostname.endsWith(".bandcamp.com"))) return undefined;
      const videoId = kind === "youtube" && url.pathname === "/watch" ? url.searchParams.get("v") : null;
      if (kind === "youtube" && url.pathname === "/watch" && !/^[A-Za-z0-9_-]{11}$/.test(videoId ?? "")) return undefined;
      if (kind === "soundcloud" && url.pathname.split("/").some(part => part.startsWith("s-"))) return undefined;
      url.search = ""; url.hash = "";
      if (videoId) url.searchParams.set("v",videoId);
      return url.href;
    } catch { return undefined; }
  }
  function isPublicCitationUrl(raw: string): boolean {
    try {
      const url = new URL(raw);
      const host = url.hostname.toLowerCase().replace(/\.$/, "");
      return url.protocol === "https:" && !url.port && !url.username && !url.password && host.includes(".")
        && !/^\d{1,3}(\.\d{1,3}){3}$/.test(host) && !host.startsWith("[") && host !== "localhost"
        && ![".local", ".internal", ".localhost"].some(suffix => host.endsWith(suffix));
    } catch { return false; }
  }
  async function lookupContext(target: ContextTarget) {
    if (!contextLookupConfigured || contextPending) return;
    const targetTrack = target.track;
    const title = boundedText(targetTrack.title, 512);
    if (!title) { contextError = "This item needs a title before it can be looked up."; return; }
    const generation = contextGeneration;
    const targetKey = trackTargetKey(targetTrack, target.targetKind);
    const requestId = newRequestId();
    if (!requestId) { contextError = "Secure request IDs are unavailable in this session."; return; }
    contextPending = true;
    contextAnswer = "";
    contextSources = [];
    contextError = "";
    try {
      const response = await call<ContextResponse>("lookup_media_context", {request: {
        requestId,
        title,
        artist: boundedText(targetTrack.artist, 256),
        album: boundedText(targetTrack.album, 256),
        version: boundedText(targetTrack.version, 256),
        sourceKind: sourceKind(targetTrack),
        targetKind: target.targetKind,
        date: boundedText(targetTrack.recordingDate, 32),
        ...(contextSourceUrl(targetTrack) ? {sourceUrl:contextSourceUrl(targetTrack)} : {}),
      }});
      if (generation !== contextGeneration || targetKey !== trackTargetKey(target.track, target.targetKind) || response.requestId !== requestId) return;
      contextAnswer = plainTextMarkdown(response.answer);
      contextSources = (response.sources ?? []).filter(source => typeof source.title === "string" && !!source.title.trim() && isPublicCitationUrl(source.url)).slice(0,8);
      contextAnswerLabel = `${target.label}: ${[targetTrack.title,targetTrack.artist].filter(Boolean).join(" — ")}`;
    } catch (failure) {
      if (generation === contextGeneration && targetKey === trackTargetKey(target.track, target.targetKind)) contextError = String(failure).includes("source-backed") ? "No reliable source information was found for this item." : "Could not look up context. Check the configured AI connection and try again.";
    } finally {
      if (generation === contextGeneration && targetKey === trackTargetKey(target.track, target.targetKind)) contextPending = false;
    }
  }
  function clearContext() { contextGeneration++; contextAnswer = ""; contextSources = []; contextAnswerLabel = ""; contextError = ""; contextPending = false; }
  function newRequestId(): string | null {
    if (globalThis.crypto?.randomUUID) return globalThis.crypto.randomUUID();
    if (!globalThis.crypto?.getRandomValues) return null;
    const bytes = globalThis.crypto.getRandomValues(new Uint8Array(16));
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    const hex = [...bytes].map(value => value.toString(16).padStart(2,"0")).join("");
    return `${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`;
  }
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
  {#if contextLookupConfigured}<section class="context-lookup" aria-label="AI song and set context">
    <h3>Song / set context</h3>
    {#if !contextTargets?.length}<label class="context-kind">Context type<select aria-label="Context type" bind:value={contextKind}><option value="song">Song</option><option value="concert">Concert</option><option value="set">DJ set</option></select></label>{/if}
    {#each visibleContextTargets as target (trackTargetKey(target.track, target.targetKind))}
      <button class="context-action" disabled={!contextLookupConfigured || contextPending} onclick={() => void lookupContext(target)}>Ask AI about {target.label}</button>
    {/each}
    {#if contextPending}<p role="status">Looking up context…</p>{/if}
    {#if contextError}<p role="alert">{contextError}</p>{/if}
    {#if contextAnswer}<div class="context-result"><small class="context-badge">{contextSources.length ? "AI summary · web sources" : "AI-generated · unverified"}</small><strong>{contextAnswerLabel}</strong><p>{contextAnswer}</p>{#if contextSources.length}<nav class="context-sources" aria-label="AI context sources">{#each contextSources as source,index}<a href={source.url} target="_blank" rel="noopener noreferrer" onclick={event=>void openSourcePage(event,source.url)}>{index+1}. {source.title} ↗</a>{/each}</nav>{/if}<button class="context-clear" onclick={clearContext}>Clear answer</button></div>{/if}
  </section>{/if}
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
  .recording-information{margin-top:26px}nav{display:flex;flex-wrap:wrap;gap:12px;margin:12px 0}nav a{color:var(--accent);font-size:13px}.playback-preference{margin:14px 0}.playback-preference label{display:grid;gap:6px;margin:0;color:var(--muted);font-size:11px}.playback-preference select{width:100%;min-width:0;max-width:100%;font-size:12px}.playback-preference p{margin:6px 0 0;font-size:11px;line-height:1.4}h2{font-size:24px;margin:0 0 18px}h3{font-size:15px;margin:22px 0 10px}p{color:var(--muted);font-size:14px;line-height:1.5}.artist{display:block;margin-top:8px;background:transparent;border:0;padding:0;color:var(--text);text-align:left;text-decoration:underline}ul{list-style:none;padding:0;margin:0;display:grid;gap:7px;max-height:300px;overflow:auto}.release,.candidate{width:100%;display:grid;gap:5px;text-align:left;padding:9px 10px;border:1px solid var(--line);border-radius:7px;background:var(--raised);color:var(--text);overflow-wrap:anywhere}.release:disabled{opacity:1}.candidate{margin-top:8px}.release small,.candidate small,.attribution,.context-badge{color:var(--muted)}.attribution{display:block;margin-top:14px;font-size:11px}summary{cursor:pointer;color:var(--accent);margin-top:16px}.notice{font-size:12px}.compact h2{font-size:15px}.compact p,.compact button{font-size:12px}.context-lookup{margin:14px 0;padding:12px;border:1px solid var(--line);border-radius:8px}.context-lookup h3{margin:0 0 8px}.context-action,.context-clear{margin:4px 8px 4px 0}.context-result{margin-top:14px;display:grid;gap:10px;min-width:0}.context-result>strong{display:block;font-size:13px;line-height:1.5;overflow-wrap:anywhere}.context-result p{margin:0}.context-result .context-clear{justify-self:start}.context-result p{white-space:pre-wrap;overflow-wrap:anywhere}.context-sources{display:grid;gap:8px;margin:0;min-width:0}.context-sources a{white-space:normal;overflow-wrap:anywhere;line-height:1.4}.context-badge{display:block;font-size:11px;line-height:1.4}.context-clear{font-size:12px}button:focus-visible,summary:focus-visible{outline:2px solid var(--accent);outline-offset:3px}
</style>
