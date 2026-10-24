<script lang="ts">
  import { tick, untrack, onDestroy } from "svelte";
  import type { Track } from "./lib/types";
  import { validateLyricsTranslation, type LyricsTranslationResult } from "./lib/lyrics-translation";

  export type LyricCandidate = { id: string; trackName: string; artistName: string; albumName?: string | null; durationSeconds: number; hasSynced: boolean; automaticMatch: boolean; explanation: string; sourceUrl: string };
  export type LyricCandidates = { scopeId: string; status: string; candidates: LyricCandidate[] };
  export type CurrentTrackLyrics = {
    scopeId?: string; candidates?: LyricCandidate[]; selectionNotice?: string;
    status: "synced" | "unsynced" | "instrumental" | "notFound" | "unmatched" | "unavailable" | "noCurrentTrack" | "invalidRequest" | "providerUnavailable";
    provider: "lrclib" | "shazam" | "localLrc" | "embeddedLyrics";
    sourceHash?: string;
    sourceOffsetSeconds?: number;
    match?: { trackName: string; artistName: string; albumName?: string | null; durationSeconds?: number | null; durationDeltaSeconds?: number | null };
    lines?: { id?: string; timeSeconds: number | null; text: string }[];
    plainLyrics?: string;
  };
  export type LyricsSyncResult = { offsetSeconds: number };
  type Loader = () => Promise<CurrentTrackLyrics>;
  type TranslationLine = { id: string; timeSeconds: number | null; text: string };
  type Translator = (sourceHash: string, lines: TranslationLine[]) => Promise<LyricsTranslationResult>;
  let { fullscreenRequested = false, onFullscreenClose, track, absolutePosition, loadLyrics, findLyrics, chooseLyrics, syncLyrics, syncScope = "", translationEnabled = false, translationCredentialPresent = false, translateLyrics }: { fullscreenRequested?: boolean; onFullscreenClose?: () => void; track: Track; absolutePosition: number; loadLyrics: Loader; findLyrics?: () => Promise<LyricCandidates>; chooseLyrics?: (scopeId: string, candidateId: string) => Promise<CurrentTrackLyrics>; syncLyrics?: () => Promise<LyricsSyncResult | null>; syncScope?: string; translationEnabled?: boolean; translationCredentialPresent?: boolean; translateLyrics?: Translator } = $props();

  let open = $state(false);
  let fullscreen = $state(false);
  let panel = $state<HTMLElement | null>(null);
  let expandButton = $state<HTMLButtonElement | null>(null);
  let destroyed = false;
  onDestroy(() => { destroyed = true; if (panel?.parentElement === document.body) panel.remove(); });
  $effect(() => {
    if (!fullscreen || !panel) return;
    const node = panel, marker = document.createComment("lyrics-panel");
    node.before(marker);
    document.body.append(node);
    node.focus();
    return () => { if (destroyed || !marker.isConnected) { node.remove(); marker.remove(); } else { marker.replaceWith(node); expandButton?.focus(); } };
  });
  function lyricsKeys(event: KeyboardEvent) {
    if (!fullscreen) return;
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); closeFullscreen(); }
    if (event.key === "Tab" && panel) {
      const buttons = [...panel.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
      const first = buttons[0], last = buttons[buttons.length - 1];
      if (event.shiftKey && (document.activeElement === first || document.activeElement === panel)) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && (document.activeElement === last || document.activeElement === panel)) { event.preventDefault(); first?.focus(); }
    }
  }
  let loading = $state(false);
  let candidatePage = $state<LyricCandidates | null>(null);
  let candidatesOpen = $state(false);
  let candidatesBusy = $state(false);
  let candidateMessage = $state("");
  let failure = $state("");
  let lyrics = $state<CurrentTrackLyrics | null>(null);
  let timingOffset = $state(0);
  let anchorOffset = $state<number | null>(null);
  let syncing = $state(false);
  let syncMessage = $state("");
  let translation = $state<LyricsTranslationResult | null>(null);
  let translating = $state(false);
  let translationMessage = $state("");
  let translationEpoch = 0;
  let lyricScroll = $state<HTMLDivElement | null>(null);
  let lineNodes = $state<(HTMLParagraphElement | undefined)[]>([]);
  let requestedScope = "";
  let requestEpoch = 0;
  let syncEpoch = 0;
  let requestedSyncScope = "";
  const scope = $derived([track.id, track.provider, track.providerId, track.sourceUrl, track.title, track.version ?? "", track.album ?? "", track.durationSeconds ?? ""].join(String.fromCharCode(0)));
  const timingMismatch = $derived(lyrics?.status === "synced" && lyrics?.match?.durationDeltaSeconds != null && Number.isFinite(lyrics.match.durationDeltaSeconds) && Math.abs(lyrics.match.durationDeltaSeconds) > 2);
  function translationSourceLines(document: CurrentTrackLyrics): TranslationLine[] | null {
    if ((document.status !== "synced" && document.status !== "unsynced") || !Array.isArray(document.lines) || !document.lines.length) return null;
    const lines: TranslationLine[] = [];
    for (const line of document.lines) {
      if (!line || typeof line.id !== "string" || !line.id.trim() || typeof line.text !== "string") return null;
      if (document.status === "synced" ? !Number.isFinite(line.timeSeconds) || (line.timeSeconds as number) < 0 : line.timeSeconds !== null) return null;
      lines.push({ id: line.id, timeSeconds: line.timeSeconds, text: line.text });
    }
    return lines;
  }
  const activeLine = $derived.by(() => {
    if (lyrics?.status !== "synced" || timingMismatch || !Number.isFinite(absolutePosition)) return -1;
    const sourceOffset = lyrics?.sourceOffsetSeconds;
    const lyricsClock = absolutePosition + timingOffset + (anchorOffset ?? 0) + (typeof sourceOffset === "number" && Number.isFinite(sourceOffset) && Math.abs(sourceOffset) <= 300 ? sourceOffset : 0);
    let active = -1;
    for (let index = 0; index < (lyrics.lines?.length ?? 0); index++) {
      const line = lyrics.lines![index]!;
      if (typeof line.timeSeconds !== "number" || !Number.isFinite(line.timeSeconds) || line.timeSeconds < 0 || typeof line.text !== "string" || !line.text.trim()) continue;
      if (line.timeSeconds <= lyricsClock) active = index;
      else break;
    }
    return active;
  });

  $effect(() => {
    const currentScope = scope;
    if (requestedScope === currentScope) return;
    requestedScope = currentScope;
    requestEpoch++;
    open = false;
    fullscreen = false;
    timingOffset = 0;
    loading = false;
    lyrics = null;
    candidatePage = null; candidatesOpen = false; candidatesBusy = false; candidateMessage = "";
    failure = "";
    timingOffset = 0;
    anchorOffset = null;
    translationEpoch++;
    translation = null;
    translating = false;
    translationMessage = "";
  });
  $effect(() => {
    const currentSyncScope = syncScope;
    if (requestedSyncScope === currentSyncScope) return;
    requestedSyncScope = currentSyncScope;
    syncEpoch++;
    syncing = false;
    syncMessage = "";
    anchorOffset = null;
    timingOffset = 0;
  });
  $effect(() => {
    const active = activeLine, content = lyrics, container = lyricScroll;
    if (active < 0 || !container || content?.status !== "synced") return;
    void tick().then(() => {
      const line = lineNodes[active];
      if (line && lyricScroll === container && typeof container.scrollTo === "function") container.scrollTo({ top: Math.max(0, line.offsetTop - container.offsetTop - container.clientHeight * 0.4), behavior: "smooth" });
    });
  });

  async function showLyrics() {
    open = true;
    if (lyrics || loading) return;
    const epoch = ++requestEpoch, expectedScope = scope;
    loading = true; failure = "";
    try {
      const result = await loadLyrics();
      if (epoch !== requestEpoch || expectedScope !== scope) return;
      if (!result || !["synced", "unsynced", "instrumental", "notFound", "unmatched", "unavailable", "noCurrentTrack", "invalidRequest", "providerUnavailable"].includes(result.status)) {
        lyrics = { status: "unavailable", provider: "lrclib" };
      } else if (result.status === "synced") {
        const lines = (Array.isArray(result.lines) ? result.lines : []).filter((line): line is { id?: string; timeSeconds: number; text: string } => line != null && typeof line.timeSeconds === "number" && Number.isFinite(line.timeSeconds) && line.timeSeconds >= 0 && typeof line.text === "string" && Boolean(line.text.trim())).slice().sort((a, b) => a.timeSeconds - b.timeSeconds);
        lyrics = lines.length ? { ...result, lines } : { status: "unavailable", provider: "lrclib" };
      } else lyrics = result;
    } catch (error) {
      if (epoch === requestEpoch) failure = error instanceof Error ? error.message : String(error);
    } finally {
      if (epoch === requestEpoch) loading = false;
    }
  }
  async function showCandidates() {
    candidatesOpen = !candidatesOpen;
    if (!candidatesOpen || !findLyrics || candidatesBusy) return;
    const expected = scope, epoch = requestEpoch;
    candidatesBusy = true; candidateMessage = "";
    try {
      const result = await findLyrics();
      if (scope !== expected || epoch !== requestEpoch) return;
      candidatePage = result;
      if (!result.candidates.length) candidateMessage = result.status === "providerUnavailable" ? "Lyric search is temporarily unavailable. Try again." : "No other lyric results found.";
    } catch (error) { if (scope === expected) candidateMessage = String(error); }
    finally { if (scope === expected) candidatesBusy = false; }
  }
  async function selectCandidate(id: string) {
    const page = candidatePage ?? (lyrics?.scopeId ? { scopeId: lyrics.scopeId, candidates: lyrics.candidates ?? [], status: "ok" } : null);
    if (!chooseLyrics || !page || candidatesBusy) return;
    const expected = scope, epoch = ++requestEpoch;
    translationEpoch++; translation = null; translating = false; translationMessage = "";
    candidatesBusy = true;
    try {
      const result = await chooseLyrics(page.scopeId, id);
      if (expected !== scope || epoch !== requestEpoch) return;
      lyrics = result; timingOffset = 0; anchorOffset = null; syncMessage = "";
    } catch (error) { if (expected === scope) candidateMessage = String(error); }
    finally { if (expected === scope) candidatesBusy = false; }
  }
  function closeFullscreen() { fullscreen = false; onFullscreenClose?.(); }
  $effect(() => {
    const requested = fullscreenRequested, currentScope = scope;
    if (requested && currentScope) untrack(() => { void showLyrics(); fullscreen = true; });
  });
  function hideLyrics() { closeFullscreen(); open = false; }
  function retry() { candidatePage = null; candidatesOpen = false; candidateMessage = ""; lyrics = null; translationEpoch++; translation = null; translating = false; translationMessage = ""; failure = ""; void showLyrics(); }
  async function syncToAudio() {
    if (lyrics?.selectionNotice) return;
    if (!syncLyrics || syncing) return;
    const epoch = ++syncEpoch;
    syncing = true; syncMessage = "";
    try {
      const result = await syncLyrics();
      if (epoch !== syncEpoch) return;
      if (result && Number.isFinite(result.offsetSeconds) && Math.abs(result.offsetSeconds) <= 30) {
        anchorOffset = result.offsetSeconds;
        syncMessage = `Synced to the identified audio (${result.offsetSeconds >= 0 ? "+" : ""}${result.offsetSeconds.toFixed(1)} s).`;
      } else {
        syncMessage = "No matching timing anchor was available; lyrics were not shifted.";
      }
    } catch (error) {
      if (epoch === syncEpoch) syncMessage = error instanceof Error ? error.message : "Audio identification failed.";
    } finally { if (epoch === syncEpoch) syncing = false; }
  }
  function resetTiming() { timingOffset = 0; anchorOffset = null; syncMessage = ""; }
  async function translateLoadedLyrics() {
    const document = lyrics;
    if (!translationEnabled || !translateLyrics || translating || (document?.status !== "synced" && document?.status !== "unsynced") || !document.sourceHash) return;
    const sourceHash = document.sourceHash;
    const lines = translationSourceLines(document);
    if (!lines) return;
    const expectedScope = scope, epoch = ++translationEpoch;
    translating = true; translationMessage = "";
    try {
      const result = await translateLyrics(sourceHash, lines);
      if (epoch !== translationEpoch || expectedScope !== scope || lyrics?.sourceHash !== sourceHash) return;
      const checked = validateLyricsTranslation(sourceHash, lines, result);
      if (!checked) { translationMessage = "The translation did not match these lyrics. The original is unchanged."; return; }
      translation = checked;
      translationMessage = `Translated to ${checked.targetLanguage}${checked.model ? ` · ${checked.provider === "openRouter" ? "OpenRouter · " : ""}${checked.model}` : checked.provider === "googleFree" ? " · Google Translate" : ""}${checked.fallbackUsed ? " · API fallback" : ""}.`;
    } catch {
      if (epoch === translationEpoch && expectedScope === scope) translationMessage = "Translation failed. Check the translation provider settings and try again.";
    } finally {
      if (epoch === translationEpoch) translating = false;
    }
  }
</script>

{#snippet translationHeadings(language: string)}
  <div class="lyrics-column-headings" aria-hidden="true"><span>Original</span><span>Translation · {language}</span></div>
{/snippet}

<!-- svelte-ignore a11y_no_noninteractive_tabindex a11y_no_noninteractive_element_interactions -->
<article class="lyrics-panel" class:fullscreen bind:this={panel} tabindex={fullscreen ? -1 : undefined} role={fullscreen ? "dialog" : undefined} aria-modal={fullscreen ? "true" : undefined} aria-label={fullscreen ? "Fullscreen lyrics" : "Lyrics"} onkeydown={lyricsKeys}>
  <div class="lyrics-heading"><h2 title={lyrics?.provider === "shazam" ? "Lyrics from Shazam identification" : undefined}>Lyrics</h2><div class="lyrics-actions">
    {#if open}
      {#if lyrics?.status === "synced" || lyrics?.status === "unsynced"}
        <span class="lyrics-translation-controls"><button type="button" aria-label="Translate lyrics" title={!translationEnabled || !translationCredentialPresent ? "Configure lyric translation in Settings → Connections" : "Translate lyrics"} disabled={translating || Boolean(translation) || !translationEnabled || !translationCredentialPresent || !translateLyrics || !lyrics.sourceHash || !translationSourceLines(lyrics)} onclick={() => void translateLoadedLyrics()}>{translating ? "Translating…" : "Translate"}</button></span>
      {/if}
      <button bind:this={expandButton} type="button" aria-label={fullscreen ? "Exit fullscreen lyrics" : "Maximize lyrics"} title={fullscreen ? "Exit fullscreen (Escape)" : "Maximize lyrics"} onclick={() => { if (fullscreen) closeFullscreen(); else fullscreen = true; }}>{fullscreen ? "⤢" : "⛶"}</button>
      <button type="button" aria-label="Hide lyrics" title="Hide lyrics" onclick={hideLyrics}>×</button>
    {:else}<button type="button" class="lyrics-open" aria-expanded={open} onclick={() => void showLyrics()}>Show lyrics</button>{/if}
  </div></div>
  {#if translationMessage && !translation}<p class="lyrics-status" role="status">{translationMessage}</p>{/if}
  {#if open}
    {#if findLyrics}<button class="lyrics-other-results" type="button" aria-expanded={candidatesOpen} onclick={() => void showCandidates()}>Other lyric results</button>{/if}
    {#if candidatesOpen || (lyrics?.candidates?.length && !lyrics?.lines?.length)}
      <div class="lyric-candidates" aria-label="Lyric results">
        {#if candidatesBusy}<p role="status">Looking up lyric results…</p>{/if}
        {#if candidateMessage}<p role="status">{candidateMessage}</p>{/if}
        {#each candidatePage?.candidates ?? lyrics?.candidates ?? [] as candidate (candidate.id)}
          <article><strong>{candidate.trackName}</strong><p>{candidate.artistName}{#if candidate.albumName} · {candidate.albumName}{/if} · {Math.round(candidate.durationSeconds)} s · {candidate.hasSynced ? candidate.automaticMatch ? "Timed lyrics" : "Timed source · displayed as plain text" : "Plain lyrics"}</p><small>{candidate.explanation}</small>
          <div><button type="button" disabled={candidatesBusy} onclick={() => void selectCandidate(candidate.id)}>Show these lyrics</button> <a href={candidate.sourceUrl} target="_blank" rel="noreferrer">LRCLIB source ↗</a></div></article>
        {/each}
      </div>
    {/if}
    {#if lyrics?.match?.artistName}<small class="lyrics-source">Lyrics credit: {lyrics.match.artistName}{#if lyrics.match.albumName} · {lyrics.match.albumName}{/if} · {lyrics.provider === "lrclib" ? "LRCLIB" : lyrics.provider}</small>{/if}
    {#if lyrics?.selectionNotice}<p class="lyrics-status" role="status">{lyrics.selectionNotice}</p>{/if}
    {#if candidatesOpen}<small class="lyrics-external-source">Other transcriptions: <a href={`https://www.google.com/search?q=${encodeURIComponent(`site:lyricstranslate.com ${track.title} lyrics`)}`} target="_blank" rel="noreferrer">Search LyricsTranslate ↗</a>. Performances and repetitions may differ.</small>{/if}
    {#if loading}<p class="lyrics-status" role="status">Looking up lyrics for this track…</p>
    {:else if failure}<p class="lyrics-error" role="alert">Could not load lyrics: {failure} <button type="button" onclick={retry}>Retry</button></p>
    {:else if (candidatePage?.candidates.length ?? lyrics?.candidates?.length ?? 0) > 0 && !lyrics?.lines?.length}<p class="lyrics-status" role="status">Choose a lyric result above. The exact recording has not been confirmed.</p>
    {:else if timingMismatch || lyrics?.status === "unmatched"}<p class="lyrics-status" role="status">A lyrics result did not match this recording's title, performers, or duration. It is not displayed.</p>
    {:else if lyrics?.status === "instrumental"}<p class="lyrics-status" role="status">This recording is marked instrumental.</p>
    {:else if lyrics?.status === "notFound"}<p class="lyrics-status" role="status">No lyrics were found for this track.</p>
    {:else if lyrics?.status === "noCurrentTrack"}<p class="lyrics-status" role="status">No current full-track metadata is available for lyrics. Start playback of a track with a title, artist, and duration; excerpts are not matched as full songs.</p>
    {:else if lyrics?.status === "invalidRequest"}<p class="lyrics-status" role="status">LRCLIB needs a valid title, artist, and duration. Check the current track metadata, then retry.</p>
    {:else if lyrics?.status === "providerUnavailable"}<p class="lyrics-status" role="status">LRCLIB could not be reached. Check your internet connection. <button type="button" onclick={retry}>Retry</button></p>
    {:else if lyrics?.status === "unavailable"}<p class="lyrics-status" role="status">Lyrics are currently unavailable.</p>
    {:else if lyrics?.status === "unsynced"}
      {#if translation}
        {@render translationHeadings(translation.targetLanguage)}
        <div class="lyrics-scroll lyrics-plain-rows" aria-label="Original and translated lyrics">
          {#each translationSourceLines(lyrics) ?? [] as line, index (line.id)}
            {@const translatedLine = translation.lines.find(item => item.id === line.id)}
            <div class="lyrics-line-row translated-row"><p>{line.text || " "}</p><p class="translated-line" aria-label={`Translated paragraph ${index + 1}${translatedLine ? "" : ", unavailable"}`}>{translatedLine ? translatedLine.text || " " : "Translation unavailable for this line."}</p></div>
          {/each}
        </div>
      {:else}<p class="lyrics-unsynced">{lyrics.plainLyrics || lyrics.lines?.map(line => line.text).join("\n") || "No plain lyrics text is available."}</p>{/if}
    {:else if lyrics?.status === "synced"}
      {#if translation}{@render translationHeadings(translation.targetLanguage)}{/if}
      <div class="lyrics-scroll" bind:this={lyricScroll} aria-label="Synchronized lyrics">
        {#each lyrics.lines ?? [] as line, index (line.id ?? line.timeSeconds + ":" + index)}
          {@const translatedLine = translation?.lines.find(item => item.id === line.id)}
          <div class:translated-row={Boolean(translation)} class="lyrics-line-row">
            <p bind:this={lineNodes[index]} class:active={index === activeLine} aria-current={index === activeLine ? "true" : undefined}>{line.text}</p>
            {#if translation}<p class="translated-line" aria-label={`Translated line ${index + 1}${translatedLine ? "" : ", unavailable"}`}>{translatedLine?.text || "Translation unavailable for this line."}</p>{/if}
          </div>
        {/each}
      </div>
      {#if syncLyrics}<div class="lyrics-sync">
        <button type="button" disabled={syncing} onclick={() => void syncToAudio()}>{syncing ? "Identifying audio…" : "Sync with audio identification"}</button>
        <small>When you click, a short sample from current native playback is sent for song recognition. The returned timing is used only for these lyrics.</small>
        {#if syncMessage}<p role="status">{syncMessage}</p>{/if}
      </div>{/if}
      <div class="lyrics-timing" aria-label="Manual lyrics timing adjustment">
        <button type="button" aria-label="Shift lyrics later by 0.5 seconds" title="Shift lyrics later" disabled={timingOffset <= -30} onclick={() => timingOffset = Math.max(-30, timingOffset - 0.5)}>−</button>
        <span>Manual offset {timingOffset > 0 ? "+" : ""}{timingOffset.toFixed(1)} s</span>
        <button type="button" aria-label="Shift lyrics earlier by 0.5 seconds" title="Shift lyrics earlier" disabled={timingOffset >= 30} onclick={() => timingOffset = Math.min(30, timingOffset + 0.5)}>+</button>
        {#if anchorOffset !== null}<span>Audio anchor {anchorOffset >= 0 ? "+" : ""}{anchorOffset.toFixed(1)} s</span>{/if}
        <button type="button" class="lyrics-reset" disabled={timingOffset === 0 && anchorOffset === null} onclick={resetTiming}>Reset</button>
        <small>Display sync only; source timestamps and playback are unchanged.</small>
      </div>
    {/if}
  {/if}
</article>

<style>
  .lyrics-other-results { margin:.5rem 0; }
  .lyric-candidates { max-height:20rem; overflow:auto; margin:.5rem 0; }
  .lyric-candidates article { padding:.7rem; border-bottom:1px solid var(--line); }
  .lyric-candidates p { margin:.3rem 0; }
  .lyric-candidates small { display:block; color:var(--muted); margin-bottom:.4rem; }

  .lyrics-panel{min-width:0}
  .lyrics-panel.fullscreen{position:fixed;inset:0;z-index:2000;box-sizing:border-box;padding:clamp(20px,4vw,60px);background:var(--surface,#18221d);color:var(--text,#e5ebe7);display:flex;flex-direction:column;gap:12px;overflow:hidden;border-radius:0}
  .fullscreen .lyrics-scroll,.fullscreen .lyrics-unsynced{flex:1;min-height:0;max-height:none;font-size:clamp(18px,2.5vw,32px)}
  .fullscreen .lyrics-scroll p{font-size:clamp(18px,2.5vw,32px)}
  .lyrics-actions{display:flex;align-items:center;gap:.5rem}.lyrics-actions .lyrics-translation-controls{padding:0}
.lyrics-heading{display:flex;align-items:start;justify-content:space-between;gap:1rem}
  h2{font-size:24px;margin:0 0 8px}.lyrics-status{margin:.15rem 0 .8rem;color:#b5beb8;font-size:.83rem;line-height:1.45}
  button{border:1px solid var(--line,#454a50);border-radius:7px;background:#ffffff0a;color:inherit;padding:.4rem .65rem;cursor:pointer}
  .lyrics-open{flex:none}.lyrics-scroll{max-height:min(42vh,420px);overflow:auto;overscroll-behavior:contain;scroll-behavior:smooth;padding:.25rem .45rem}
  .lyrics-scroll p{margin:0;padding:.5rem .3rem;color:#9ca69f;line-height:1.5;font-size:1rem;transition:color .15s ease,transform .15s ease}
  .lyrics-scroll p.active{color:var(--text,#f0f4f1);font-weight:650;transform:scale(1.015);transform-origin:left center}
  .lyrics-translation-controls{display:flex;align-items:center;gap:.65rem;flex-wrap:wrap;padding:.65rem .15rem 0}.lyrics-line-row{min-width:0}.lyrics-line-row.translated-row{display:grid;grid-template-columns:1fr 1fr;gap:1rem;padding:.2rem 0;border-bottom:1px solid #ffffff0b}.lyrics-line-row.translated-row p{margin:0}.lyrics-line-row .translated-line{color:var(--text,#f0f4f1)}
  .lyrics-column-headings{display:grid;grid-template-columns:1fr 1fr;gap:1rem;padding:0 .75rem;color:#8f9c94;font-size:.72rem;text-transform:uppercase;letter-spacing:.08em}
  .lyrics-unsynced{max-height:min(42vh,420px);overflow:auto;white-space:pre-wrap;line-height:1.65;color:var(--text,#f0f4f1)}
  .lyrics-sync{display:grid;justify-items:start;gap:.35rem;padding:.7rem .15rem 0}.lyrics-sync small{max-width:42rem;color:#8f9c94;line-height:1.4}.lyrics-sync p{margin:0;color:#b5beb8;font-size:.85rem}
  .lyrics-timing{display:flex;align-items:center;gap:.5rem;flex-wrap:wrap;padding:.65rem .15rem 0;color:#b5beb8;font-size:.8rem}.lyrics-timing small{flex-basis:100%;color:#8f9c94}.lyrics-timing button{padding:.2rem .5rem}.lyrics-timing button:disabled{opacity:.5;cursor:default}.lyrics-timing .lyrics-reset{margin-left:.3rem}
  .lyrics-error{color:var(--danger,#ff8c8c)}.lyrics-error button{margin-left:.4rem}
</style>
