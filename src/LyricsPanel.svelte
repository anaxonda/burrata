<script lang="ts">
  import { dismissDetails } from "./lib/dismiss-details";
  import { tick, untrack, onDestroy } from "svelte";
  import { sortLyricCandidates, lyricRuntime, lyricDurationDifference, type LyricResultSort } from "./lib/lyric-candidates";
  import LanguagePicker from "./LanguagePicker.svelte";
  import { lyricsTiming } from "./lib/lyrics-timing.svelte";
  import type { Track } from "./lib/types";
  import { lyricsPlaybackPosition } from "./lib/lyrics-request";
  import { validateLyricsTranslation, validateLyricsTranslationProgress, type LyricsTranslationResult } from "./lib/lyrics-translation";

  export type LyricCandidate = { id: string; trackName: string; artistName: string; albumName?: string | null; durationSeconds: number; hasSynced: boolean; automaticMatch: boolean; plainTextMatch?: boolean; explanation: string; sourceUrl: string };
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
  type Translator = (sourceHash: string, lines: TranslationLine[], progress?: (result: LyricsTranslationResult)=>void, signal?:AbortSignal, options?:{useFallback?:boolean}) => Promise<LyricsTranslationResult>;
  let { fullscreenRequested = false, onFullscreenClose, track, absolutePosition, loadLyrics, findLyrics, chooseLyrics, syncLyrics, syncScope = "", saveLyrics, exportLyrics, translationLanguage = "en", changeTranslationLanguage, translationEnabled = false, translationCredentialPresent = false, translateLyrics, bottomInset = 0, onSeek, playbackControlsId = "burrata-playback-controls" }: { fullscreenRequested?: boolean; onFullscreenClose?: () => void; track: Track; absolutePosition: number; loadLyrics: Loader; findLyrics?: () => Promise<LyricCandidates>; chooseLyrics?: (scopeId: string, candidateId: string) => Promise<CurrentTrackLyrics>; syncLyrics?: () => Promise<LyricsSyncResult | null>; syncScope?: string; saveLyrics?: (sourceHash: string) => Promise<unknown>; exportLyrics?: (sourceHash: string, text: string) => Promise<{cancelled?: boolean}>; translationLanguage?: string; changeTranslationLanguage?: (language: string) => Promise<unknown>; translationEnabled?: boolean; translationCredentialPresent?: boolean; translateLyrics?: Translator; bottomInset?: number; onSeek?: (seconds:number) => void; playbackControlsId?:string } = $props();

  let open = $state(false);
  let fullscreen = $state(false);
  let panel = $state<HTMLElement | null>(null);
  let optionsMenu = $state<HTMLDetailsElement | null>(null);
  let expandButton = $state<HTMLButtonElement | null>(null);
  let destroyed = false;
  onDestroy(() => { translationAbort?.abort(); clearTimeout(timingNoticeTimer); destroyed = true; if (panel?.parentElement === document.body) panel.remove(); });
  $effect(() => {
    if (!fullscreen || !panel) return;
    const node = panel, marker = document.createComment("lyrics-panel");
    node.before(marker);
    document.body.append(node);
    node.focus();
    return () => { if (destroyed || !marker.isConnected) { node.remove(); marker.remove(); } else { marker.replaceWith(node); expandButton?.focus(); } };
  });
  function lyricsKeys(event: KeyboardEvent) {
    if (event.key === "Escape" && timingFromLine) {event.preventDefault();event.stopPropagation();timingFromLine=false;return;}
    if (event.key === "Escape" && lyricsSearchOpen && lyricsSearchQuery.trim()) {event.preventDefault();event.stopPropagation();updateLyricsSearch("");lyricsSearchOpen=false;return;}
    if (!fullscreen) return;
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); if (optionsMenu?.open) optionsMenu.open = false; else {lyricsSearchOpen=false;closeFullscreen();} }
  }
  function fullscreenFocusableElements(root:HTMLElement | null):HTMLElement[] {
    if(!root)return [];
    return [...root.querySelectorAll<HTMLElement>("button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),a[href],summary,[tabindex]:not([tabindex='-1'])")]
      .filter(node=>!node.closest("[hidden],[inert],[aria-hidden='true']")
        && (node.tagName==="SUMMARY" || !node.closest("details:not([open])")));
  }
  function trapFullscreenTab(event:KeyboardEvent) {
    if(!fullscreen || event.key!=="Tab" || !panel)return;
    const footer=playbackControlsId ? document.getElementById(playbackControlsId) : null;
    const targets=[...fullscreenFocusableElements(panel),...fullscreenFocusableElements(footer)];
    if(!targets.length){event.preventDefault();panel.focus();return;}
    const current=document.activeElement as HTMLElement | null;
    const index=current ? targets.indexOf(current) : -1;
    const next=index<0 ? (event.shiftKey ? targets.length-1 : 0) : (index+(event.shiftKey?-1:1)+targets.length)%targets.length;
    event.preventDefault();targets[next]?.focus();
  }
  let loading = $state(false);
  let candidatePage = $state<LyricCandidates | null>(null);
  let candidatesOpen = $state(false);
  let candidatesBusy = $state(false);
  let candidateMessage = $state("");
  let candidateSort = $state<LyricResultSort>("match");
  let failure = $state("");
  let lyrics = $state<CurrentTrackLyrics | null>(null);
  const displayedCandidates = $derived(sortLyricCandidates(candidatePage?.candidates ?? lyrics?.candidates ?? [], track.durationSeconds, candidateSort));

  let temporaryTimingOffset = $state(0);
  const timingKey=$derived(lyrics?.status==="synced" && lyrics.sourceHash ? JSON.stringify([track.provider,track.providerId||track.id,lyrics.sourceHash,lyrics.sourceOffsetSeconds??0]) : null);
  const timingOffset=$derived(timingKey ? lyricsTiming.get(timingKey) : temporaryTimingOffset);
  let timingFromLine = $state(false);
  let timingNotice = $state("");
  let timingNoticeTimer: ReturnType<typeof setTimeout> | undefined;
  let anchorOffset = $state<number | null>(null);
  let syncing = $state(false);
  let syncMessage = $state("");
  let translation = $state<LyricsTranslationResult | null>(null);
  let translating = $state(false);
  let translationMessage = $state("");
  let translationFailed = $state(false);
  let translationEpoch = 0;
  let translationAbort: AbortController | null = null;
  let pairAlternatingLines = $state(false);
  let sourceTranslationEnabled = $state(true);
  let showTranslation = $state(false);
  let savingLyrics = $state(false);
  let saveMessage = $state("");
  let lyricsSearchQuery = $state("");
  let lyricsSearchOpen = $state(false);
  let lyricsSearchIndex = $state(0);
  let lyricsSearchNodes = $state<(HTMLElement | undefined)[]>([]);
  let lyricsSearchInput = $state<HTMLInputElement | null>(null);
  const sourcePlainLines = $derived((lyrics?.plainLyrics || lyrics?.lines?.map(line => line.text).join("\n") || "").split("\n"));
  const pairedSourceLines = $derived(sourcePlainLines.filter(line => line.trim()));
  type SearchItem = { key:string; text:string };
  const lyricsSearchItems = $derived.by(():SearchItem[] => {
    if (lyrics?.status === "synced") return (lyrics.lines ?? []).flatMap((line,index) => [
      {key:`source-${index}`,text:line.text},
      ...(showTranslation ? [{key:`translation-${index}`,text:translation?.lines.find(item=>item.id===line.id)?.text ?? ""}] : []),
    ]);
    if (lyrics?.status !== "unsynced") return [];
    const pairedGrid=showTranslation && (Boolean(translation) || !pairAlternatingLines || !sourceTranslationEnabled);
    if (pairedGrid) return displaySourceLines(lyrics).flatMap((line,index) => [
      {key:`source-${index}`,text:line.text},
      ...(showTranslation ? [{key:`translation-${index}`,text:translation?.lines.find(item=>item.id===line.id)?.text ?? ""}] : []),
    ]);
    if (pairAlternatingLines) {
      const nonempty=pairedSourceLines;
      return nonempty.filter((_,index)=>index%2===0).flatMap((text,index) => [
        {key:`paired-source-${index}`,text},
        ...(showTranslation && sourceTranslationEnabled ? [{key:`paired-translation-${index}`,text:nonempty[index*2+1] ?? ""}] : []),
      ]);
    }
    return sourcePlainLines.map((text,index)=>({key:`plain-${index}`,text}));
  });
  function rangesIn(text:string) {
    const query=lyricsSearchQuery.trim();
    if(!query)return [] as {start:number;end:number}[];
    const escaped=query.replace(/[.*+?^${}()|[\]\\]/g,"\\$&"), expression=new RegExp(escaped,"giu"), ranges:{start:number;end:number}[]=[];
    for(const match of text.matchAll(expression)){const start=match.index;if(start!==undefined)ranges.push({start,end:start+match[0].length});}
    return ranges;
  }
  const lyricsSearchMatches = $derived.by(() => lyricsSearchItems.flatMap(item=>rangesIn(item.text).map(range=>({key:item.key,...range}))));
  $effect(() => {const count=lyricsSearchMatches.length;if(!count)lyricsSearchIndex=0;else if(lyricsSearchIndex>=count)lyricsSearchIndex=count-1;});
  function searchParts(key:string,text:string) {
    const itemIndex=lyricsSearchItems.findIndex(item=>item.key===key);
    const base=lyricsSearchItems.slice(0,Math.max(0,itemIndex)).reduce((count,item)=>count+rangesIn(item.text).length,0);
    const ranges=rangesIn(text), parts:{text:string;match:boolean;index:number}[]=[];
    let offset=0;
    for(let index=0;index<ranges.length;index++){const range=ranges[index]!;if(range.start>offset)parts.push({text:text.slice(offset,range.start),match:false,index:-1});parts.push({text:text.slice(range.start,range.end),match:true,index:base+index});offset=range.end;}
    if(offset<text.length||!parts.length)parts.push({text:text.slice(offset),match:false,index:-1});
    return parts;
  }
  async function navigateLyricsSearch(delta:number) {
    if(!lyricsSearchMatches.length)return;
    lyricsSearchIndex=(lyricsSearchIndex+delta+lyricsSearchMatches.length)%lyricsSearchMatches.length;
    await tick();
    lyricsSearchNodes[lyricsSearchIndex]?.scrollIntoView?.({block:"center",behavior:"smooth"});
  }
  async function toggleLyricsSearch() {
    lyricsSearchOpen=!lyricsSearchOpen;
    if(!lyricsSearchOpen)updateLyricsSearch("");
    else {await tick();lyricsSearchInput?.focus();}
  }
  function updateLyricsSearch(value:string) {
    lyricsSearchQuery=value;
    lyricsSearchIndex=0;
    lyricsSearchNodes=[];
    void tick().then(()=>lyricsSearchNodes[0]?.scrollIntoView?.({block:"center",behavior:"smooth"}));
  }
  let lyricScroll = $state<HTMLDivElement | null>(null);
  let lineNodes = $state<(HTMLDivElement | undefined)[]>([]);
  let requestedScope = "";
  let requestEpoch = 0;
  let syncEpoch = 0;
  let requestedSyncScope = "";
  $effect(() => {
    const target=translationLanguage.trim().toLowerCase(), cached=translation;
    if (cached && cached.targetLanguage.trim().toLowerCase()!==target) {translationAbort?.abort(); translationEpoch++;translation=null;translating=false;translationMessage="Translation language changed. Retry translation.";}
  });
  const scope = $derived([track.id, track.provider, track.providerId, track.sourceUrl, track.artist, track.assetId ?? "", track.localPath ?? "", track.title, track.version ?? "", track.album ?? "", track.durationSeconds ?? ""].join(String.fromCharCode(0)));
  const timingMismatch = $derived(lyrics?.status === "synced" && lyrics?.match?.durationDeltaSeconds != null && Number.isFinite(lyrics.match.durationDeltaSeconds) && Math.abs(lyrics.match.durationDeltaSeconds) > 2);
  const canSeekLyricLines = $derived.by(() => {
    const provenance=track.provenance && typeof track.provenance==="object" ? track.provenance as Record<string,unknown> : {};
    return Boolean(onSeek && lyrics?.status==="synced" && !timingMismatch && !lyrics.selectionNotice
      && !provenance.activeLyricsDetection && !provenance.approximateExcerpt);
  });
  function seekToLyricLine(time:unknown) {
    if(!canSeekLyricLines || timingFromLine || typeof time!=="number" || !Number.isFinite(time) || time<0)return;
    const sourceStart=track.playbackRange?.startTime ?? 0;
    onSeek?.(Math.max(0,time-timingOffset-(anchorOffset??0)-sourceTimingOffset()+sourceStart));
  }
  // Only infer an already bilingual source for clear alternating scripts or
  // repeated English sentence markers. Ordinary plain lyrics stay in source order.
  function sourceHasAlternatingLanguages(document: CurrentTrackLyrics): boolean {
    const lines = (document.plainLyrics || document.lines?.map(line => line.text).join("\n") || "").split("\n").filter(line => line.trim());
    if (lines.length < 4 || lines.length % 2) return false;
    const english = (text: string) => /^(?:the |a |an |and |but |so |it |she |he |i |you |we |they |underneath |unwanted |then |when |there |that |this )/i.test(text.trim()) || /\b(?:was|were|did|does|don't|didn't)\b/i.test(text);
    const cyrillic = (text: string) => /[\u0400-\u04ff]/.test(text);
    const left=lines.filter((_,i)=>i%2===0), right=lines.filter((_,i)=>i%2===1);
    return [english,cyrillic].some(matches => (matches !== english || lines.length >= 8) && ((left.filter(matches).length/left.length >= .75 && right.filter(matches).length/right.length <= .25) || (right.filter(matches).length/right.length >= .75 && left.filter(matches).length/left.length <= .25)));
  }
  function displayedLyricsText(): string {
    if (!lyrics) return "";
    if (translation && showTranslation) return displaySourceLines(lyrics).map(line => [line.text,translation!.lines.find(item=>item.id===line.id)?.text ?? ""].join("\t")).join("\n");
    if (pairAlternatingLines) return pairedSourceLines.filter((_,index)=>index%2===0).map((text,index)=>showTranslation ? `${text}\t${pairedSourceLines[index*2+1] ?? ""}` : text).join("\n");
    return lyrics.plainLyrics || lyrics.lines?.map(line=>line.text).join("\n") || "";
  }
  function displaySourceLines(document: CurrentTrackLyrics) {
    const lines = document.lines ?? [];
    return pairAlternatingLines ? lines.filter(line=>line.text.trim()).filter((_,index)=>index%2===0) : lines;
  }
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
  const lyricPosition=$derived(lyricsPlaybackPosition(track,absolutePosition));
  const lyricsClock=$derived(lyricPosition+timingOffset+(anchorOffset??0)+sourceTimingOffset());
  const nextTimedLine=$derived(lyrics?.status==="synced" ? lyrics.lines?.find(line=>typeof line.timeSeconds==="number"&&line.timeSeconds>lyricsClock) : undefined);
  const activeLine = $derived.by(() => {
    if (lyrics?.status !== "synced" || timingMismatch || !Number.isFinite(lyricPosition)) return -1;
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
    const reopenRadio=untrack(()=>open) && ["radio","radioparadise","internet-radio"].includes(track.provider);
    requestedScope = currentScope;
    requestEpoch++;
    open = false;
    lyricsSearchOpen=false;lyricsSearchQuery="";lyricsSearchIndex=0;lyricsSearchNodes=[];
    fullscreen = false;
    temporaryTimingOffset = 0;
    timingFromLine = false; timingNotice = "";
    loading = false;
    lyrics = null;
    pairAlternatingLines = false; sourceTranslationEnabled = true; showTranslation = false; saveMessage = "";
    candidatePage = null; candidatesOpen = false; candidatesBusy = false; candidateMessage = "";
    failure = "";
    temporaryTimingOffset = 0;
    anchorOffset = null;
    translationAbort?.abort(); translationEpoch++;
    translation = null;
    translating = false;
    translationMessage = "";
    if(reopenRadio)void tick().then(()=>{if(!destroyed && scope===currentScope)void showLyrics();});
  });
  $effect(() => {
    const currentSyncScope = syncScope;
    if (requestedSyncScope === currentSyncScope) return;
    requestedSyncScope = currentSyncScope;
    syncEpoch++;
    syncing = false;
    syncMessage = "";
    anchorOffset = null;
    temporaryTimingOffset = 0;
    timingFromLine = false; timingNotice = "";
  });
  $effect(() => {
    const active = activeLine, content = lyrics, container = lyricScroll;
    if (active < 0 || !container || content?.status !== "synced" || lyricsSearchQuery.trim()) return;
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
      pairAlternatingLines = lyrics?.status === "unsynced" && sourceHasAlternatingLanguages(lyrics); sourceTranslationEnabled = ["en","english"].includes(translationLanguage.toLowerCase()); showTranslation = pairAlternatingLines && sourceTranslationEnabled;
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
    pairAlternatingLines = false; sourceTranslationEnabled = true; showTranslation = false; saveMessage = "";
    translationAbort?.abort(); translationEpoch++; translation = null; translating = false; translationMessage = "";
    candidatesBusy = true;
    try {
      const result = await chooseLyrics(page.scopeId, id);
      if (expected !== scope || epoch !== requestEpoch) return;
      lyrics = result; candidatesOpen = false; pairAlternatingLines = result.status === "unsynced" && sourceHasAlternatingLanguages(result); sourceTranslationEnabled = ["en","english"].includes(translationLanguage.toLowerCase()); showTranslation = pairAlternatingLines && sourceTranslationEnabled; temporaryTimingOffset = 0; anchorOffset = null; syncMessage = "";
    } catch (error) { if (expected === scope) candidateMessage = String(error); }
    finally { if (expected === scope) candidatesBusy = false; }
  }
  function closeFullscreen() { fullscreen = false; onFullscreenClose?.(); }
  $effect(() => {
    const requested = fullscreenRequested, currentScope = scope;
    if (requested && currentScope) untrack(() => { void showLyrics(); fullscreen = true; });
  });
  function hideLyrics() { closeFullscreen(); open = false; }
  function retry() { candidatePage = null; candidatesOpen = false; candidateMessage = ""; lyrics = null; translationAbort?.abort(); translationEpoch++; translation = null; translating = false; translationMessage = ""; failure = ""; void showLyrics(); }
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
  function sourceTimingOffset() {
    const offset=lyrics?.sourceOffsetSeconds;
    return typeof offset==="number" && Number.isFinite(offset) && Math.abs(offset)<=300 ? offset : 0;
  }
  function setTimingOffset(seconds:number){
    if(timingKey){
      const combined=Math.round((seconds+(anchorOffset??0))*10)/10;
      if(Math.abs(combined)>30){timingNotice="Timing adjustment must stay within 30 seconds.";return false;}
      anchorOffset=null;lyricsTiming.set(timingKey,combined);
    }else temporaryTimingOffset=seconds;
    return true;
  }
  function beginLineTiming() {
    timingFromLine=true;timingNotice="";clearTimeout(timingNoticeTimer);
    if(optionsMenu)optionsMenu.open=false;
    void tick().then(()=>panel?.querySelector<HTMLButtonElement>(`[data-lyric-index="${Math.max(0,activeLine)}"] .lyrics-set-line`)?.focus({preventScroll:true}));
  }
  function alignTimingToLine(index:number) {
    const time=lyrics?.lines?.[index]?.timeSeconds;
    if(!timingFromLine || lyrics?.status!=="synced" || typeof time!=="number" || !Number.isFinite(time) || !Number.isFinite(lyricPosition))return;
    const next=Math.round((time-lyricPosition-sourceTimingOffset()-(anchorOffset??0))*10)/10;
    if(Math.abs(next)>30){timingNotice="Choose a line within 30 seconds of the current playback position.";return;}
    if(!setTimingOffset(next))return;timingFromLine=false;timingNotice="Lyrics aligned to that line.";
    clearTimeout(timingNoticeTimer);timingNoticeTimer=setTimeout(()=>timingNotice="",3500);
  }
  function resetTiming() { anchorOffset = null;setTimingOffset(0);syncMessage = ""; timingFromLine=false;timingNotice="";clearTimeout(timingNoticeTimer); }
  async function translateLoadedLyrics(useFallback=false) {
    const document = lyrics;
    if (!translationEnabled || !translationCredentialPresent || !translateLyrics) { translationFailed=true; translationMessage = "Configure lyric translation in Settings → Connections."; return; }
    if (translating || (document?.status !== "synced" && document?.status !== "unsynced") || !document.sourceHash) return;
    const sourceHash = document.sourceHash;
    const lines = translationSourceLines(document);
    if (!lines) return;
    const expectedScope = scope, expectedLanguage = translationLanguage.trim().toLowerCase(), epoch = ++translationEpoch;
    translationAbort?.abort(); const controller = new AbortController(); translationAbort=controller;
    translating = true; translationFailed=false; translationMessage = "";
    const current=()=>epoch===translationEpoch && expectedScope===scope && lyrics?.sourceHash===sourceHash && expectedLanguage===translationLanguage.trim().toLowerCase() && !controller.signal.aborted;
    try {
      const result = await translateLyrics(sourceHash, lines, partial=>{
        if(!current() || partial.targetLanguage.trim().toLowerCase()!==expectedLanguage)return;
        const checked=validateLyricsTranslationProgress(sourceHash,lines,partial);if(checked)translation=checked;
      }, controller.signal, useFallback?{useFallback:true}:undefined);
      if (epoch !== translationEpoch || expectedScope !== scope || lyrics?.sourceHash !== sourceHash) return;
      if (expectedLanguage !== translationLanguage.trim().toLowerCase()) { translationFailed=true; translationMessage = "Translation language changed. Retry translation."; return; }
      const checked = validateLyricsTranslation(sourceHash, lines, result);
      if (!checked) { translationFailed=true; translationMessage = "The translation did not match these lyrics. The original is unchanged."; return; }
      if (checked.targetLanguage.trim().toLowerCase() !== expectedLanguage) { translationFailed=true; translationMessage = "The service returned a different translation language. Retry translation."; return; }
      translation = checked;
      translationMessage = `Translated to ${checked.targetLanguage}${checked.model ? ` · ${checked.provider === "openRouter" ? "OpenRouter · " : ""}${checked.model}` : checked.provider === "googleFree" ? " · Google Translate" : ""}${checked.fallbackUsed ? " · API fallback" : ""}.`;
    } catch (error) {
      if (epoch === translationEpoch && expectedScope === scope) { translationFailed=true; translationMessage = (error instanceof Error ? error.message : typeof error === "string" ? error : "Translation failed. Check the provider settings and try again.").slice(0,400); }
    } finally {
      if (epoch === translationEpoch) translating = false;
    }
  }
</script>

{#snippet translationHeadings(language: string)}
  <div class="lyrics-column-headings" aria-hidden="true"><span>Original</span><span>Translation · {language}</span></div>
{/snippet}
{#snippet searchableText(key: string, text: string)}
  {#each searchParts(key,text) as part}
    {#if part.match}<mark bind:this={lyricsSearchNodes[part.index]} class:current-search-match={part.index===lyricsSearchIndex} data-lyrics-search-index={part.index}>{part.text}</mark>{:else}{part.text}{/if}
  {/each}
{/snippet}

<svelte:window onkeydown={trapFullscreenTab} />
<!-- svelte-ignore a11y_no_noninteractive_tabindex a11y_no_noninteractive_element_interactions -->
<article class="lyrics-panel" class:fullscreen bind:this={panel} style:bottom={fullscreen ? `${Math.max(0,Number.isFinite(bottomInset)?bottomInset:0)}px` : undefined} tabindex={fullscreen ? -1 : undefined} role={fullscreen ? "dialog" : undefined} aria-modal={fullscreen ? "true" : undefined} aria-owns={fullscreen ? playbackControlsId : undefined} aria-label={fullscreen ? "Fullscreen lyrics" : "Lyrics"} onkeydown={lyricsKeys}>
  <div class="lyrics-heading"><h2 title={lyrics?.provider === "shazam" ? "Lyrics from Shazam identification" : undefined}>Lyrics</h2><div class="lyrics-actions">
    {#if open}
      {#if lyricsSearchItems.length}<button type="button" aria-label="Search lyrics" aria-expanded={lyricsSearchOpen} title="Search lyrics" onclick={()=>void toggleLyricsSearch()}>⌕</button>{/if}
      {#if lyrics?.status === "synced" || lyrics?.status === "unsynced"}
        <span class="lyrics-translation-controls"><button type="button" class:enabled={showTranslation} aria-busy={translating} aria-label="Show translation" aria-pressed={showTranslation} title={showTranslation ? "Hide translation" : "Show translation"} onclick={() => {
          showTranslation = !showTranslation;
          if (showTranslation && !translation && !(lyrics?.status === "unsynced" && pairAlternatingLines && sourceTranslationEnabled)) void translateLoadedLyrics();
        }}><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M3 5h12M9 2v3M5 5c1 5 4 8 8 10M13 5c-1 5-4 8-9 11M13 21l4-10 4 10M14.5 18h5"/></svg></button></span>
      {/if}
      <details use:dismissDetails class="lyrics-menu" bind:this={optionsMenu}><summary aria-label="Lyrics options" title="Lyrics options">⋯</summary><div class="lyrics-menu-content">
    {#if findLyrics}<button class="lyrics-other-results" type="button" aria-expanded={candidatesOpen} onclick={() => {if(optionsMenu)optionsMenu.open=false; void showCandidates();}}>Other lyric results</button>{/if}
    {#if lyrics?.match?.artistName}<small class="lyrics-source">Lyrics credit: {lyrics.match.artistName}{#if lyrics.match.albumName} · {lyrics.match.albumName}{/if} · {lyrics.provider === "lrclib" ? "LRCLIB" : lyrics.provider}</small>{/if}
    {#if lyrics?.selectionNotice}<p class="lyrics-status" role="status">{lyrics.selectionNotice}</p>{/if}
    {#if candidatesOpen}<small class="lyrics-external-source">Other transcriptions: <a href={`https://www.google.com/search?q=${encodeURIComponent(`site:lyricstranslate.com ${track.title} lyrics`)}`} target="_blank" rel="noreferrer">Search LyricsTranslate ↗</a>. Performances and repetitions may differ.</small>{/if}
        {#if lyrics?.status === "unsynced" && !translation}<button class="lyrics-pair-lines" type="button" aria-pressed={pairAlternatingLines} onclick={() => {pairAlternatingLines = !pairAlternatingLines; sourceTranslationEnabled = ["en","english"].includes(translationLanguage.toLowerCase()); showTranslation = pairAlternatingLines && sourceTranslationEnabled;}}>Source has alternating bilingual lines</button>{/if}
        {#if translationMessage && (translation || !showTranslation)}<p class="lyrics-status" role="status">{translationMessage}</p>{/if}
        {#if translateLyrics && translationEnabled}<button type="button" disabled={translating} title="Use the configured fallback provider and API key for this translation" onclick={()=>{showTranslation=true;void translateLoadedLyrics(true);}}>Retry with fallback</button>{/if}
        {#if !translation && showTranslation && !(lyrics?.status === "unsynced" && pairAlternatingLines && sourceTranslationEnabled)}<button type="button" onclick={() => void translateLoadedLyrics()} disabled={translating}>Retry translation</button>{/if}
        {#if saveLyrics && lyrics?.sourceHash}<button type="button" disabled={savingLyrics || !track.localPath} title={!track.localPath ? "Download this song first" : "Embed original source lyrics in this downloaded audio file"} onclick={async () => {savingLyrics = true; saveMessage = ""; try {await saveLyrics!(lyrics!.sourceHash!); saveMessage = "Lyrics saved to audio file.";} catch(error) {saveMessage = String(error);} finally {savingLyrics = false;}}}>{savingLyrics ? "Saving…" : "Embed lyrics in audio file"}</button>{/if}
        {#if lyrics?.sourceHash}
          <button type="button" onclick={async () => {try {await navigator.clipboard.writeText(displayedLyricsText());saveMessage="Lyrics copied.";} catch {saveMessage="Could not copy lyrics to the clipboard.";}}}>Copy lyrics</button>
          {#if exportLyrics}<button type="button" onclick={async () => {try {const result=await exportLyrics!(lyrics!.sourceHash!,displayedLyricsText());saveMessage=result.cancelled ? "" : "Lyrics saved as text.";} catch(error){saveMessage=String(error);}}}>Save lyrics as text file</button>{/if}
        {/if}
        {#if changeTranslationLanguage}<LanguagePicker value={translationLanguage} onChange={async language => {const expectedScope=scope, expectedHash=lyrics?.sourceHash;await changeTranslationLanguage!(language);if(scope!==expectedScope || lyrics?.sourceHash!==expectedHash)return;translationAbort?.abort(); translationEpoch++;translation=null;translationMessage="";translating=false;sourceTranslationEnabled=false;if(showTranslation)void translateLoadedLyrics();}} />{/if}
        {#if saveMessage}<p role="status">{saveMessage}</p>{/if}
        {#if lyrics?.status === "synced"}
      {#if syncLyrics}<div class="lyrics-sync">
        <button type="button" disabled={syncing} onclick={() => void syncToAudio()}>{syncing ? "Identifying audio…" : "Sync with audio identification"}</button>
        <small>When you click, a short sample from current native playback is sent for song recognition. The returned timing is used only for these lyrics.</small>
        {#if syncMessage}<p role="status">{syncMessage}</p>{/if}
      </div>{/if}
      <div class="lyrics-timing" aria-label="Manual lyrics timing adjustment">
        <button type="button" class="lyrics-align-action" onclick={beginLineTiming}>Set timing from a line</button>
        <div class="lyrics-timing-stepper"><button type="button" aria-label="Shift lyrics later by 0.1 seconds" title="Shift lyrics later by 0.1 seconds" disabled={timingOffset <= -30} onclick={() => setTimingOffset(Math.max(-30, Math.round((timingOffset - 0.1) * 10) / 10))}>Later</button>
        <span aria-live="polite" title="Applies immediately to lyric cue times; the highlighted line changes when a shifted cue is reached.">{Math.abs(timingOffset).toFixed(1)} s{timingOffset < 0 ? " later" : timingOffset > 0 ? " earlier" : ""}</span>
        <button type="button" aria-label="Shift lyrics earlier by 0.1 seconds" title="Shift lyrics earlier by 0.1 seconds" disabled={timingOffset >= 30} onclick={() => setTimingOffset(Math.min(30, Math.round((timingOffset + 0.1) * 10) / 10))}>Earlier</button></div>
        {#if anchorOffset !== null}<span>Audio anchor {anchorOffset >= 0 ? "+" : ""}{anchorOffset.toFixed(1)} s</span>{/if}
        <button type="button" class="lyrics-reset" disabled={timingOffset === 0 && anchorOffset === null} onclick={resetTiming}>Reset</button>
        <small class="lyrics-timing-preview">{#if nextTimedLine && typeof nextTimedLine.timeSeconds==="number"}Next line in {Math.max(0,nextTimedLine.timeSeconds-lyricsClock).toFixed(1)} s{:else}Last timed line{/if}</small>
        {#if timingKey && timingOffset!==0}<small>Remembered for this song and lyric source.</small>{/if}
      </div>
        {/if}
      </div></details>
      <button bind:this={expandButton} type="button" aria-label={fullscreen ? "Exit fullscreen lyrics" : "Maximize lyrics"} title={fullscreen ? "Exit fullscreen (Escape)" : "Maximize lyrics"} onclick={() => { if (fullscreen) closeFullscreen(); else fullscreen = true; }}>{fullscreen ? "⤢" : "⛶"}</button>
      <button type="button" aria-label="Hide lyrics" title="Hide lyrics" onclick={hideLyrics}>×</button>
    {:else}<button type="button" class="lyrics-open" aria-expanded={open} onclick={() => void showLyrics()}>Show lyrics</button>{/if}
  </div></div>
  {#if open}
    {#if lyricsSearchOpen && lyricsSearchItems.length}
      <div class="lyrics-search" role="search" aria-label="Search lyrics">
        <input bind:this={lyricsSearchInput} type="search" aria-label="Search lyrics text" placeholder="Search lyrics" value={lyricsSearchQuery} oninput={(event)=>updateLyricsSearch(event.currentTarget.value)} onkeydown={(event)=>{if(event.key==="Enter"){event.preventDefault();void navigateLyricsSearch(event.shiftKey?-1:1);}}} />
        <span class="lyrics-search-count" aria-live="polite">{lyricsSearchMatches.length ? `${lyricsSearchIndex+1} of ${lyricsSearchMatches.length}` : lyricsSearchQuery.trim() ? "0 matches" : ""}</span>
        <button type="button" aria-label="Previous lyric match" title="Previous match" disabled={!lyricsSearchMatches.length} onclick={()=>void navigateLyricsSearch(-1)}>↑</button>
        <button type="button" aria-label="Next lyric match" title="Next match" disabled={!lyricsSearchMatches.length} onclick={()=>void navigateLyricsSearch(1)}>↓</button>
      </div>
    {/if}
    {#if timingFromLine || timingNotice}<div class="lyrics-timing-notice" role="status"><span>{timingNotice || "Click a line when you hear it start."}</span>{#if timingFromLine}<button type="button" onclick={()=>{timingFromLine=false;timingNotice="";}}>Cancel timing adjustment</button>{/if}</div>{/if}
    {#if showTranslation && (translating || translationFailed || !translation) && !(pairAlternatingLines && sourceTranslationEnabled)}
      <div class="translation-notice" role="status">{#if translating}<span>Translating…</span>{:else if translationMessage}<span>{translationMessage}</span><button type="button" onclick={() => void translateLoadedLyrics()}>Retry translation</button>{/if}</div>
    {/if}
    {#if candidatesOpen || (lyrics?.candidates?.length && !lyrics?.lines?.length)}
      <div class="lyric-candidates" class:results-only={!lyrics?.lines?.length} aria-label="Lyric results">
        {#if candidatesBusy}<p role="status">Looking up lyric results…</p>{/if}
        {#if candidateMessage}<p role="status">{candidateMessage}</p>{/if}
        <div class="candidate-sort"><label>Sort results <select aria-label="Sort lyric results" bind:value={candidateSort}><option value="match">Best match</option><option value="duration">Closest duration</option><option value="shortest">Shortest first</option><option value="longest">Longest first</option></select></label>{#if track.durationSeconds}<span>Playing {lyricRuntime(track.durationSeconds)}</span>{/if}</div>
        {#each displayedCandidates as candidate (candidate.id)}
          <article><strong>{candidate.trackName}</strong><p>{candidate.artistName}{#if candidate.albumName} · {candidate.albumName}{/if} · {lyricRuntime(candidate.durationSeconds)}{lyricDurationDifference(candidate.durationSeconds, track.durationSeconds) ? ` · ${lyricDurationDifference(candidate.durationSeconds, track.durationSeconds)}` : ""} · {candidate.hasSynced ? candidate.automaticMatch ? "Timed lyrics" : "Timed source · displayed as plain text" : "Plain lyrics"}</p><small>{candidate.explanation}</small>
          <div><button type="button" disabled={candidatesBusy} onclick={() => void selectCandidate(candidate.id)}>Show these lyrics</button> <a href={candidate.sourceUrl} target="_blank" rel="noreferrer">LRCLIB source ↗</a></div></article>
        {/each}
      </div>
    {/if}
    {#if loading}<p class="lyrics-status" role="status">Looking up lyrics for this track…</p>
    {:else if failure}<p class="lyrics-error" role="alert">Could not load lyrics: {failure} <button type="button" onclick={retry}>Retry</button></p>
    {:else if (candidatePage?.candidates.length ?? lyrics?.candidates?.length ?? 0) > 0 && !lyrics?.lines?.length}<p class="lyrics-status" role="status">Choose a lyric result above. The exact recording has not been confirmed.</p>
    {:else if timingMismatch || lyrics?.status === "unmatched"}<p class="lyrics-status" role="status">A lyrics result did not match this recording's title, performers, or duration. It is not displayed.</p>
    {:else if lyrics?.status === "instrumental"}<p class="lyrics-status" role="status">This recording is marked instrumental.</p>
    {:else if lyrics?.status === "notFound"}<p class="lyrics-status" role="status">No lyrics were found for this track.</p>
    {:else if lyrics?.status === "noCurrentTrack"}<p class="lyrics-status" role="status">{#if ["radio","radioparadise","internet-radio"].includes(track.provider)}Waiting for the current radio song to be identified.{:else}No current full-track metadata is available for lyrics. Start playback of a track with a title, artist, and duration; excerpts are not matched as full songs.{/if}</p>
    {:else if lyrics?.status === "invalidRequest"}<p class="lyrics-status" role="status">LRCLIB needs a valid title, artist, and duration. Check the current track metadata, then retry.</p>
    {:else if lyrics?.status === "providerUnavailable"}<p class="lyrics-status" role="status">Lyrics are temporarily unavailable from LRCLIB. <button type="button" onclick={retry}>Retry</button></p>
    {:else if lyrics?.status === "unavailable"}<p class="lyrics-status" role="status">Lyrics are currently unavailable.</p>
    {:else if lyrics?.status === "unsynced"}
      {#if showTranslation && (translation || !pairAlternatingLines || !sourceTranslationEnabled)}
        {@render translationHeadings(translation?.targetLanguage ?? translationLanguage)}
        <div class="lyrics-scroll lyrics-plain-rows" aria-label="Original and translated lyrics">
          {#each displaySourceLines(lyrics) as line, index (line.id)}
            {@const translatedLine = translation?.lines.find(item => item.id === line.id)}
            <div class="lyrics-line-row translated-row"><p>{@render searchableText(`source-${index}`,line.text || " ")}</p><p class="translated-line" aria-label={translatedLine ? `Translated paragraph ${index + 1}` : undefined}>{@render searchableText(`translation-${index}`,translatedLine?.text || " ")}</p></div>
          {/each}
        </div>
      {:else}
        {#if pairAlternatingLines}
          <div class="lyrics-scroll lyrics-plain-rows" aria-label="Paired source lyrics">
            {#each pairedSourceLines.filter((_, index) => index % 2 === 0) as line, index}
              <div class="lyrics-line-row" class:translated-row={showTranslation && sourceTranslationEnabled}><p>{@render searchableText(`paired-source-${index}`,line || " ")}</p>{#if showTranslation && sourceTranslationEnabled}<p class="translated-line">{@render searchableText(`paired-translation-${index}`,pairedSourceLines[index * 2 + 1] || " ")}</p>{/if}</div>
            {/each}
          </div>
        {:else}<p class="lyrics-unsynced">{#each sourcePlainLines as line,index}{@render searchableText(`plain-${index}`,line)}{#if index<sourcePlainLines.length-1}<br />{/if}{/each}</p>{/if}
      {/if}
    {:else if lyrics?.status === "synced"}
      {#if showTranslation}{@render translationHeadings(translation?.targetLanguage ?? "")}{/if}
      <div class="lyrics-scroll" bind:this={lyricScroll} aria-label="Synchronized lyrics">
        {#each lyrics.lines ?? [] as line, index (line.id ?? line.timeSeconds + ":" + index)}
          {@const translatedLine = translation?.lines.find(item => item.id === line.id)}
          <div bind:this={lineNodes[index]} class:translated-row={showTranslation} class:setting-timing={timingFromLine} class="lyrics-line-row" data-lyric-index={index}>
            {#if canSeekLyricLines && !timingFromLine && typeof line.timeSeconds==="number" && Number.isFinite(line.timeSeconds) && line.timeSeconds>=0 && line.text.trim()}<button type="button" class="lyrics-seek-line" aria-label={`Seek playback to lyric line ${index+1}: ${line.text}`} title={`Seek to ${line.timeSeconds.toFixed(1)} seconds`} onclick={()=>seekToLyricLine(line.timeSeconds)}></button>{/if}
            {#if timingFromLine}<button type="button" class="lyrics-set-line" aria-label={`Align lyric line ${index + 1} to current playback: ${line.text}`} onclick={()=>alignTimingToLine(index)}></button>{/if}
            <p class:active={index === activeLine} aria-current={index === activeLine ? "true" : undefined}>{@render searchableText(`source-${index}`,line.text)}</p>
            {#if showTranslation}<p class="translated-line" class:active={index === activeLine} aria-current={index === activeLine && translatedLine ? "true" : undefined} aria-label={translatedLine ? `Translated line ${index + 1}` : undefined}>{@render searchableText(`translation-${index}`,translatedLine?.text || " ")}</p>{/if}
          </div>
        {/each}
      </div>

    {/if}
  {/if}
</article>

<style>
  .candidate-sort{display:flex;align-items:center;justify-content:space-between;gap:12px;margin:0 0 14px;font-size:12px;color:var(--muted)}.candidate-sort label{display:flex;flex-direction:row;flex-wrap:wrap;align-items:center;gap:8px;margin:0;min-width:0}.candidate-sort select{max-width:180px}
  .translation-notice{display:flex;align-items:center;gap:.75rem;color:var(--muted);font-size:.85rem}.translation-notice:empty{display:none}.translation-notice button{flex:none}
  .lyrics-menu{position:relative}.lyrics-menu summary{cursor:pointer;list-style:none;border:1px solid var(--line,#454a50);border-radius:7px;padding:.35rem .65rem;font-size:1.2rem;line-height:1}
  .lyrics-menu summary::-webkit-details-marker{display:none}.lyrics-menu-content{position:absolute;right:0;top:calc(100% + 8px);z-index:3;width:min(350px,calc(100vw - 48px));max-height:65vh;overflow:auto;box-sizing:border-box;padding:14px;background:var(--surface,#18221d);border:1px solid var(--line,#454a50);border-radius:10px;box-shadow:0 8px 30px #0006;display:grid;gap:10px}
  .lyrics-translation-controls button{display:flex;align-items:center;justify-content:center;width:36px;height:34px;flex-shrink:0}.lyrics-translation-controls button.enabled{color:var(--accent);background:color-mix(in srgb,var(--accent) 12%,transparent)}
  .lyrics-other-results { margin:.5rem 0; }
  .lyric-candidates { max-height:20rem; overflow:auto; margin:.5rem 0; }
  .fullscreen .lyric-candidates.results-only { flex:1; min-height:0; max-height:none; }
  .lyric-candidates a, .lyrics-external-source a { color:var(--text); text-decoration-color:var(--muted); }
  .lyric-candidates article { padding:.7rem; border-bottom:1px solid var(--line); }
  .lyric-candidates p { margin:.3rem 0; }
  .lyric-candidates small { display:block; color:var(--muted); margin-bottom:.4rem; }

  .lyrics-panel{min-width:0}
  .lyrics-search{display:flex;align-items:center;gap:.4rem;flex:none;padding:.1rem 0}.lyrics-search input{min-width:0;flex:1;padding:.45rem .6rem;border:1px solid var(--line,#454a50);border-radius:6px;background:var(--surface,#18221d);color:var(--text,#e5ebe7)}.lyrics-search-count{min-width:4.2rem;text-align:center;color:var(--muted,#9ca69f);font-size:.78rem;font-variant-numeric:tabular-nums}.lyrics-search button{min-width:32px;padding:.35rem}.lyrics-panel :global(mark){background:#a4dc72;color:#172019;border-radius:2px}.lyrics-panel :global(mark.current-search-match){background:#ffcf58;color:#17130b;outline:2px solid #fff2a8;outline-offset:1px}
  .lyrics-panel.fullscreen{position:fixed;inset:0;z-index:2000;box-sizing:border-box;padding:12px clamp(20px,4vw,60px) 20px;background:var(--surface,#18221d);color:var(--text,#e5ebe7);display:flex;flex-direction:column;gap:6px;overflow:hidden;border-radius:0}
  .fullscreen .lyrics-heading{align-items:center;min-height:34px}.fullscreen .lyrics-heading h2{margin:0;font-size:18px}
  .fullscreen .lyrics-scroll,.fullscreen .lyrics-unsynced{flex:1;min-height:0;max-height:none;font-size:clamp(18px,2.5vw,32px)}
  .fullscreen .lyrics-scroll p{font-size:clamp(18px,2.5vw,32px)}
  .lyrics-actions{display:flex;align-items:center;gap:.5rem;flex-shrink:0}.lyrics-actions .lyrics-translation-controls{padding:0}
.lyrics-heading{display:flex;align-items:start;justify-content:space-between;gap:1rem}
  h2{font-size:24px;margin:0 0 8px}.lyrics-status{margin:.15rem 0 .8rem;color:#b5beb8;font-size:.83rem;line-height:1.45}
  button{border:1px solid var(--line,#454a50);border-radius:7px;background:#ffffff0a;color:inherit;padding:.4rem .65rem;cursor:pointer}
  .lyrics-open{flex:none}.lyrics-scroll{max-height:min(42vh,420px);overflow:auto;overscroll-behavior:contain;scroll-behavior:smooth;padding:.25rem .45rem}
  .lyrics-scroll p{margin:0;padding:.5rem .3rem;color:#9ca69f;line-height:1.5;font-size:1rem;transition:color .15s ease}
  .lyrics-scroll p.active{color:var(--text,#f0f4f1);font-weight:650}
  .lyrics-line-row.setting-timing,.lyrics-line-row:has(.lyrics-seek-line){position:relative;border-radius:6px}.lyrics-line-row.setting-timing:has(.lyrics-set-line:hover),.lyrics-line-row.setting-timing:has(.lyrics-set-line:focus-visible){background:#ffffff0a;outline:1px solid var(--accent,#b7e88b)}.lyrics-set-line,.lyrics-seek-line{position:absolute;inset:0;z-index:1;width:100%;height:100%;padding:0;border:0;background:transparent;cursor:crosshair}.lyrics-seek-line{cursor:pointer}.lyrics-seek-line:focus-visible{outline:2px solid var(--accent,#b7e88b);outline-offset:1px}.lyrics-timing-notice{display:flex;align-items:center;gap:.7rem;padding:.3rem .15rem;color:#b5beb8;font-size:.82rem}.lyrics-timing-notice button{padding:.2rem .4rem;font-size:inherit}
  .lyrics-translation-controls{display:flex;align-items:center;gap:.65rem;flex-wrap:wrap;padding:.65rem .15rem 0}.lyrics-line-row{min-width:0}.lyrics-line-row.translated-row{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:1rem;padding:0}.lyrics-line-row.translated-row p{margin:0;min-width:0;overflow-wrap:anywhere}.lyrics-plain-rows .translated-line{color:var(--text,#f0f4f1)}
  .lyrics-column-headings{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:1rem;padding:0 .75rem;color:#8f9c94;font-size:.72rem;text-transform:uppercase;letter-spacing:.08em}
  .lyrics-unsynced{max-height:min(42vh,420px);overflow:auto;white-space:pre-wrap;line-height:1.65;color:var(--text,#f0f4f1)}
  .lyrics-sync{display:grid;justify-items:start;gap:.35rem;padding:.7rem .15rem 0}.lyrics-sync small{max-width:42rem;color:#8f9c94;line-height:1.4}.lyrics-sync p{margin:0;color:#b5beb8;font-size:.85rem}
  .lyrics-timing{display:grid;gap:.6rem;padding:.65rem .15rem 0;color:#b5beb8;font-size:.8rem}.lyrics-timing-stepper{display:grid;grid-template-columns:auto minmax(0,1fr) auto;align-items:center;gap:.5rem}.lyrics-timing-stepper span{text-align:center;font-variant-numeric:tabular-nums}.lyrics-timing button{padding:.4rem .5rem}.lyrics-timing button:disabled{opacity:.5;cursor:default}.lyrics-timing .lyrics-reset{justify-self:end}
  .lyrics-error{color:var(--danger,#ff8c8c)}.lyrics-error button{margin-left:.4rem}
</style>
