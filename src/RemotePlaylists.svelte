<script lang="ts">
  import { onDestroy, onMount, tick } from "svelte";
  import { ProviderReadCancellation } from "./lib/provider-read-cancellation";
  import { providerAccountScope, providerNavigationSnapshots } from "./lib/provider-navigation-snapshots";
  import { normalizeSpotifyUserProfileUrl } from "./lib/spotify-profile-url";
  import { safeArtworkUrl } from "./lib/artwork-url";
  import { appendPageWithinLimit } from "./remote-playlist-paging";
  type Provider = "spotify" | "youtube";
  type RemotePlaylist = {
    id: string; name?: string; title?: string; ownerId?: string | null; ownerName?: string | null; url?: string | null;
    trackCount?: number | null; imageUrl?: string | null; artworkUrl?: string | null; collaborative?: boolean;
    visibility?: string | null; ownerUrl?: string | null; canEdit?: boolean; imported?: boolean; localCollectionId?: string | null;
  };
  type BrowseResult = { items: RemotePlaylist[]; nextCursor?: string | null; accountId?: string; totalCount?: number | null; truncated?: boolean; complete?: boolean; snapshotConsistency?: "progressive" };
  type BrowseDescriptor = { provider: Provider; mode: "mine" | "profile" | "search" | "url"; profileUrl: string; query: string; searchKind: "playlist" | "channel" };
  type PageDeadline = { at: number; onExpire: () => void };
  type ProfileChoice = { id: string; kind: string; title: string; subtitle?: string | null; url?: string | null; artworkUrl?: string | null };
  type ProfileSearchPage = { results?: ProfileChoice[]; nextCursor?: string | null };
  type ImportResult = { bindingId?: string; id?: string; playlistId?: string; title?: string; provider?: string; collectionId?: string; trackCount?: number; complete?: boolean; error?: string };
  type ImportFailure = { playlistId: string; message: string };
  type PlaylistsSnapshot = { provider: Provider; sourceMode: "mine" | "profile" | "search" | "url"; profileUrl: string; directUrl: string; playlistUrls: string[]; query: string; searchKind: "playlist" | "channel"; items: RemotePlaylist[]; cursor: string | null; selected: string[]; seenCursors: string[]; currentBrowseComplete: boolean | null; pageNote: string; snapshotConsistency: "progressive" | null; scroll: number; loadedCount: number; needsRefetch: boolean };
  type ImportJob = { jobId?: string; id?: string; state?: string; completed?: number; total?: number; results?: ImportResult[]; errors?: ImportFailure[]; error?: string | null };

  let { call, onImported = () => {}, onClose = () => {}, onOpenPlaylist = () => {}, onBrowseOwner, initialProvider = "spotify", initialOwner = "", initialPlaylistUrl = "", profileOnly = false }: {
    call: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
    onImported?: () => void;
    onClose?: () => void;
    onOpenPlaylist?: (provider: Provider, playlistUrl: string) => void;
    onBrowseOwner?: (provider: Provider, ownerUrl: string) => void;
    initialProvider?: Provider;
    initialOwner?: string;
    initialPlaylistUrl?: string;
    profileOnly?: boolean;
  } = $props();
  const providerReads = new ProviderReadCancellation((command, args) => call(command, args ?? {}));

  let provider = $state<Provider>("spotify");
  let sourceMode = $state<"mine" | "profile" | "search" | "url">("mine");
  let profileUrl = $state("");
  let profileChoices = $state<ProfileChoice[]>([]);
  let profileSearchBusy = $state(false);
  let profileSearchRequest = 0;
  let directUrl = $state("");
  let playlistUrls = $state<string[]>([]);
  let query = $state("");
  let searchKind = $state<"playlist" | "channel">("playlist");
  let items = $state<RemotePlaylist[]>([]);
  let cursor = $state<string | null>(null);
  let seenBrowseCursors = new Set<string>();
  let selected = $state<string[]>([]);
  let selectingAll = $state(false);
  let selectAllCanceled = false;
  let selectAllPageCount = $state(0);
  let selectAllFound = $state(0);
  let importedIds = $state<string[]>([]);
  let loading = $state(false);
  let importing = $state(false);
  let cancelRequested = $state(false);
  let importJob = $state<ImportJob | null>(null);
  let importResults = $state<ImportResult[]>([]);
  let importErrors = $state<ImportFailure[]>([]);
  let importTotal = $state(0);
  let activeJobId = $state<string | null>(null);
  let error = $state("");
  let notice = $state("");
  let pageNote = $state("");
  let currentBrowseComplete = $state<boolean | null>(null);
  let snapshotConsistency = $state<"progressive" | null>(null);
  let destroyed = false;
  let browseGeneration = 0;
  let browseRequest = 0;
  let bindingsRequest = 0;
  let selectAllRequest = 0;
  let accountScope: string | null = null;
  let scrollElement: HTMLElement | null = null;
  let scrollTop = 0;
  let restoreGeneration = 0;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let finishPollWait: (() => void) | null = null;
  const MAX_PROFILE_ROWS = 10_000;
  const SELECT_ALL_BUDGET_MS = 120_000;
  const RESTORE_BUDGET_MS = 120_000;
  const MAX_IMPORT_SELECTION = 500;

  $effect(() => { provider; void loadBindings(); });
  $effect(() => { provider = initialProvider; });
  onMount(() => { void restoreOrBrowse(); });
  onDestroy(() => {
    destroyed = true; profileSearchRequest += 1; providerReads.cancelAll(); finishPollWait?.();
    const snapshot: PlaylistsSnapshot = { provider, sourceMode, profileUrl, directUrl, playlistUrls, query, searchKind, items, cursor, selected, seenCursors: [...seenBrowseCursors], currentBrowseComplete, pageNote, snapshotConsistency, scroll: scrollElement?.scrollTop ?? scrollTop, loadedCount: items.length, needsRefetch: false };
    if (accountScope) providerNavigationSnapshots.put({ kind: "playlists", key: playlistSnapshotKey(snapshot), accountScope, snapshot, rows: items.length, stripRows: value => ({ ...value, items: [], cursor: null, selected: [], needsRefetch: true }) });
  });

  function playlistSnapshotKey(snapshot: Pick<PlaylistsSnapshot, "provider" | "sourceMode" | "profileUrl" | "query" | "searchKind" | "directUrl" | "playlistUrls">) {
    // One list snapshot belongs to the source-browser route; its current tab/query
    // are controls inside that route and must survive playlist detail navigation.
    return JSON.stringify([snapshot.provider, initialOwner.trim() || "mine"]);
  }
  async function restoreOrBrowse() {
    const generation = ++restoreGeneration;
    const routeKey = playlistSnapshotKey({ provider, sourceMode: initialOwner.trim() ? "profile" : initialPlaylistUrl.trim() ? "url" : "mine", profileUrl: initialOwner.trim(), query: "", searchKind: "playlist", directUrl: "", playlistUrls: [] });
    const isCurrent = () => !destroyed && generation === restoreGeneration && routeKey === playlistSnapshotKey({ provider, sourceMode, profileUrl, query, searchKind, directUrl, playlistUrls });
    const scope = await providerAccountScope(call, provider);
    if (!isCurrent()) return;
    accountScope = scope;
    const saved = providerNavigationSnapshots.get<PlaylistsSnapshot>("playlists", routeKey, accountScope);
    if (saved && isCurrent() && (!profileOnly || saved.sourceMode === "profile")) {
      const restoredDescriptor = { provider: saved.provider, mode: saved.sourceMode, profileUrl: saved.profileUrl.trim(), query: saved.query.trim(), searchKind: saved.searchKind };
      const restoreStillCurrent = () => isCurrent() && sameBrowseDescriptor(restoredDescriptor, getBrowseDescriptor());
      provider = saved.provider; sourceMode = saved.sourceMode; profileUrl = saved.profileUrl; directUrl = saved.directUrl; playlistUrls = saved.playlistUrls; query = saved.query; searchKind = saved.searchKind;
      items = saved.items; cursor = saved.cursor; selected = saved.selected; seenBrowseCursors = new Set(saved.seenCursors); currentBrowseComplete = saved.currentBrowseComplete; pageNote = saved.pageNote; snapshotConsistency = saved.snapshotConsistency ?? null; scrollTop = saved.scroll;
      if (saved.needsRefetch || saved.items.length < saved.loadedCount) {
        cursor = null; items = []; const started = Date.now(); let first = true; let restoreTimedOut = false;
        const restoreLimit = Math.min(saved.loadedCount, MAX_PROFILE_ROWS);
        let rowLimitReached = false;
        while (restoreStillCurrent() && (first || items.length < restoreLimit) && items.length < MAX_PROFILE_ROWS && Date.now() - started < RESTORE_BUDGET_MS) {
          const before = items.length; await browse(first, { token: generation, descriptor: restoredDescriptor }, MAX_PROFILE_ROWS, () => { rowLimitReached = true; }, { at: started + RESTORE_BUDGET_MS, onExpire: () => { if (restoreStillCurrent()) { restoreTimedOut = true; providerReads.cancelAll(); } } }); first = false;
          if (!restoreStillCurrent() || items.length <= before || !cursor) break;
        }
        if (restoreStillCurrent() && items.length < saved.loadedCount) {
          if (rowLimitReached || (saved.loadedCount > MAX_PROFILE_ROWS && items.length >= MAX_PROFILE_ROWS)) pageNote = `Showing ${items.length} playlists; restoring is limited to ${MAX_PROFILE_ROWS.toLocaleString()} rows. Load more to continue.`;
          else if (restoreTimedOut || Date.now() - started >= RESTORE_BUDGET_MS) pageNote = `Showing ${items.length} playlists; restoring paused after two minutes. Load more to continue.`;
          else pageNote = `Showing ${items.length} playlists; the previous loaded set could not be fully restored. Load more to continue.`;
        }
      }
      if (!restoreStillCurrent()) return;
      await tick();
      if (!restoreStillCurrent()) return;
      requestAnimationFrame(() => { if (restoreStillCurrent() && scrollElement) scrollElement.scrollTop = scrollTop; });
      return;
    }
    if (initialOwner.trim()) { sourceMode = "profile"; profileUrl = initialOwner.trim(); void browse(); }
    else if (initialPlaylistUrl.trim()) {
      sourceMode = "url"; directUrl = initialPlaylistUrl.trim();
      if (isProviderUrl(initialPlaylistUrl.trim())) { playlistUrls = [new URL(initialPlaylistUrl.trim()).toString()]; selected = [...playlistUrls]; directUrl = ""; }
    } else void browse();
  }

  async function loadBindings() {
    const request = ++bindingsRequest;
    const requestedProvider = provider;
    try {
      const result = await call<{ playlistId: string }[]>("list_remote_playlist_bindings", { provider: requestedProvider });
      if (destroyed || request !== bindingsRequest || provider !== requestedProvider) return;
      importedIds = (result ?? []).map(binding => binding.playlistId);
    } catch { /* Older backends can still browse and import without sync bindings. */ }
  }

  async function browse(reset = true, restore?: { token: number; descriptor: BrowseDescriptor }, maxRows?: number, onRowLimit?: () => void, deadline?: PageDeadline) {
    if (restore ? restore.token !== restoreGeneration : true) {
      if (restore) return;
      restoreGeneration += 1;
    }
    const request = ++browseRequest;
    const generation = browseGeneration;
    const descriptor = restore?.descriptor ?? getBrowseDescriptor();
    const isCurrent = () => !destroyed && request === browseRequest && generation === browseGeneration && (!restore || restore.token === restoreGeneration) && sameBrowseDescriptor(descriptor, getBrowseDescriptor());
    loading = true; error = ""; notice = "";
    if (reset) { cursor = null; items = []; selected = []; pageNote = ""; currentBrowseComplete = null; snapshotConsistency = null; seenBrowseCursors = new Set(); }
    const requestedCursor = reset ? null : cursor;
    if (requestedCursor) seenBrowseCursors.add(requestedCursor);
    try {
      const result = deadline ? await fetchBrowsePageBeforeDeadline(requestedCursor, descriptor, deadline) : await fetchBrowsePage(requestedCursor, descriptor);
      if (!result) return;
      if (deadline && Date.now() >= deadline.at) { deadline.onExpire(); return; }
      if (!isCurrent()) return;
      const nextItems = maxRows === undefined ? dedupeItems(reset ? result.items ?? [] : [...items, ...(result.items ?? [])]) : appendPageWithinLimit(reset ? [] : items, result.items ?? [], maxRows);
      if (nextItems === null) {
        cursor = requestedCursor;
        onRowLimit?.();
        return;
      }
      items = nextItems;
      if (result.snapshotConsistency === "progressive") snapshotConsistency = "progressive";
      const nextCursor = result.nextCursor ?? null;
      let repeatedCursor = false;
      if (nextCursor && (nextCursor === requestedCursor || seenBrowseCursors.has(nextCursor))) {
        cursor = null;
        repeatedCursor = true;
        error = "The provider repeated a playlist page. Load more has been stopped to prevent duplicates; the loaded set may be incomplete.";
      } else { if (nextCursor) seenBrowseCursors.add(nextCursor); cursor = nextCursor; }
      if (repeatedCursor) {
        currentBrowseComplete = false;
        pageNote = "";
      } else updateBrowseScope(result, nextCursor);
      const imported = new Set(importedIds);
      for (const item of result.items ?? []) if (item.imported) imported.add(item.id);
      importedIds = [...imported];
    } catch (failure) { if (isCurrent()) error = `Could not browse ${providerName(descriptor.provider)} playlists: ${String(failure)}`; }
    finally { if (isCurrent()) loading = false; }
  }

  function updateBrowseScope(result: BrowseResult, next: string | null) {
    if (result.complete === true && !next && result.truncated !== true) {
      currentBrowseComplete = true;
      pageNote = "";
    } else if (sourceMode === "profile" && snapshotConsistency === "progressive" && next && result.truncated !== true) {
      currentBrowseComplete = false;
      pageNote = Number.isSafeInteger(result.totalCount) && result.totalCount! >= 0
        ? `Showing ${items.length} of ${result.totalCount} playlists. Load more to continue.`
        : `${items.length} playlists loaded. Load more to continue.`;
    } else if (result.truncated === true || result.complete === false) {
      currentBrowseComplete = false;
      const total = Number.isSafeInteger(result.totalCount) ? ` ${result.totalCount} total items were reported.` : "";
      pageNote = `Showing ${items.length} loaded playlists. The provider returned a bounded or incomplete result; it may omit playlists.${total}`;
    } else currentBrowseComplete = null;
  }

  function getBrowseDescriptor(): BrowseDescriptor {
    return { provider, mode: sourceMode, profileUrl: profileUrl.trim(), query: query.trim(), searchKind };
  }
  function sameBrowseDescriptor(a: BrowseDescriptor, b: BrowseDescriptor) {
    return a.provider === b.provider && a.mode === b.mode && a.profileUrl === b.profileUrl && a.query === b.query && a.searchKind === b.searchKind;
  }
  function invalidateBrowseContext() {
    providerReads.cancelAll();
    restoreGeneration += 1; browseGeneration += 1; browseRequest += 1; selectAllRequest += 1; selectAllCanceled = true;
    loading = false; selectingAll = false; cursor = null; items = []; selected = []; pageNote = ""; currentBrowseComplete = null; snapshotConsistency = null; seenBrowseCursors = new Set(); error = ""; notice = "";
    profileSearchRequest += 1; profileSearchBusy = false; profileChoices = [];
  }

  async function fetchBrowsePage(pageCursor: string | null, descriptor: BrowseDescriptor) {
    if (descriptor.mode === "search") {
      if (!descriptor.query) throw new Error("Enter a playlist or channel to search for.");
      return await providerReads.read<BrowseResult>("search_remote_playlists", { provider: descriptor.provider, query: descriptor.query, kind: descriptor.searchKind, ...(pageCursor ? { cursor: pageCursor } : {}) });
    }
    const ownerId = descriptor.mode === "profile" ? descriptor.profileUrl : undefined;
    if (descriptor.mode === "profile" && !ownerId) throw new Error("Enter a public profile or channel URL.");
    if (descriptor.mode === "profile" && descriptor.provider === "spotify" && !isSpotifyProfileReference(ownerId!)) throw new Error("For a Spotify display name, use Search profiles and choose a result. Browse directly with an official Spotify profile URL or user ID.");
    return await providerReads.read<BrowseResult>("list_remote_playlists", { provider: descriptor.provider, ...(ownerId ? { ownerId } : {}), ...(pageCursor ? { cursor: pageCursor } : {}) });
  }

  async function fetchBrowsePageBeforeDeadline(pageCursor: string | null, descriptor: BrowseDescriptor, deadline: PageDeadline) {
    const remaining = deadline.at - Date.now();
    if (remaining <= 0) { deadline.onExpire(); return null; }
    let timer: ReturnType<typeof setTimeout> | null = null;
    try {
      const raced = await Promise.race([
        fetchBrowsePage(pageCursor, descriptor).then(result => ({ result })),
        new Promise<{ result: null }>(resolve => { timer = setTimeout(() => { deadline.onExpire(); resolve({ result: null }); }, remaining); }),
      ]);
      return raced.result;
    } finally { if (timer) clearTimeout(timer); }
  }

  function isSpotifyProfileReference(value: string) {
    let owner = value.trim();
    if (/^https?:\/\//i.test(owner)) {
      try {
        const url = new URL(owner);
        if (url.protocol !== "https:" || url.hostname !== "open.spotify.com" || url.username || url.password || (url.port && url.port !== "443")) return false;
        const match = url.pathname.match(/^\/user\/([^/]+)(?:\/playlists)?\/?$/);
        if (!match) return false;
        owner = decodeURIComponent(match[1]!);
      } catch { return false; }
    }
    if (owner.startsWith("spotify:user:")) owner = owner.slice("spotify:user:".length);
    return owner.length > 0 && owner.length <= 180 && !/[\s\u0000-\u001f\u007f/\\?#]/u.test(owner);
  }

  async function searchSpotifyProfiles() {
    const name = profileUrl.trim();
    if (!name) { error = "Enter a Spotify profile name to search."; return; }
    const request = ++profileSearchRequest;
    profileSearchBusy = true; profileChoices = []; items = []; cursor = null; selected = []; error = ""; notice = "";
    try {
      const page = await providerReads.read<ProfileSearchPage>("search_source", { query: name, sourceIds: ["spotify"], kinds: ["profile"], cursor: null, limit: 20 });
      if (destroyed || request !== profileSearchRequest || provider !== "spotify" || sourceMode !== "profile" || profileUrl.trim() !== name) return;
      profileChoices = (page.results ?? []).filter(row => row.kind === "profile");
      if (!profileChoices.length) notice = "No Spotify profiles found. Try a shorter name, check the spelling, or paste an official profile URL.";
    } catch (failure) { if (request === profileSearchRequest && !destroyed) error = `Could not search Spotify profiles: ${String(failure)}`; }
    finally { if (request === profileSearchRequest && !destroyed) profileSearchBusy = false; }
  }

  function browseProfileChoice(choice: ProfileChoice) {
    if (!choice.url || !isSpotifyProfileReference(choice.url)) { error = "This profile did not include a valid Spotify profile URL."; return; }
    invalidateBrowseContext(); profileUrl = choice.url;
    void browse();
  }

  function dedupeItems(values: RemotePlaylist[]) {
    const found = new Set<string>();
    return values.filter(item => { if (found.has(item.id)) return false; found.add(item.id); return true; });
  }

  function changeSourceMode(mode: "mine" | "profile" | "search" | "url") {
    invalidateBrowseContext();
    sourceMode = mode; playlistUrls = mode === "url" ? playlistUrls : [];
  }

  async function selectAllPages() {
    if (sourceMode === "url" || selectingAll || importing) return;
    selectingAll = true; selectAllCanceled = false; selectAllPageCount = 0; selectAllFound = Math.min(items.length, MAX_PROFILE_ROWS); error = ""; notice = "";
    const request = ++selectAllRequest;
    const generation = browseGeneration;
    const descriptor = getBrowseDescriptor();
    const isCurrent = () => !destroyed && request === selectAllRequest && generation === browseGeneration && sameBrowseDescriptor(descriptor, getBrowseDescriptor());
    let next = cursor;
    let rowLimitReached = items.length >= MAX_PROFILE_ROWS;
    let terminalComplete = next === null && currentBrowseComplete === true;
    const seenCursors = new Set<string>();
    const started = Date.now();
    let timedOut = false;
    const deadline = { at: started + SELECT_ALL_BUDGET_MS, onExpire: () => { if (isCurrent()) { timedOut = true; providerReads.cancelAll(); } } };
    try {
      while (next && !selectAllCanceled && items.length < MAX_PROFILE_ROWS && Date.now() - started < SELECT_ALL_BUDGET_MS) {
        if (seenCursors.has(next)) { error = "The provider repeated a page cursor; stopped to avoid loading the same playlists forever."; break; }
        seenCursors.add(next);
        const result = await fetchBrowsePageBeforeDeadline(next, descriptor, deadline);
        if (!result) break;
        if (!isCurrent()) return;
        if (Date.now() >= deadline.at) { deadline.onExpire(); break; }
        if (selectAllCanceled) {
          selected = [...new Set(items.slice(0, MAX_PROFILE_ROWS).map(item => item.id))];
          notice = `Selected ${selected.length} playlists before stopping.`;
          return;
        }
        const nextItems = appendPageWithinLimit(items, result.items ?? [], MAX_PROFILE_ROWS);
        if (nextItems === null) {
          rowLimitReached = true;
          cursor = next;
          break;
        }
        items = nextItems;
        if (result.snapshotConsistency === "progressive") snapshotConsistency = "progressive";
        next = result.nextCursor || null;
        updateBrowseScope(result, next);
        terminalComplete = currentBrowseComplete === true;
        cursor = next;
        selectAllPageCount += 1; selectAllFound = items.length;
      }
      if (!isCurrent()) return;
      if (next && Date.now() >= deadline.at) deadline.onExpire();
      selected = [...new Set(items.slice(0, MAX_PROFILE_ROWS).map(item => item.id))];
      if (selectAllCanceled) notice = `Selected ${selected.length} playlists before stopping.`;
      else if (rowLimitReached && (next || items.length > MAX_PROFILE_ROWS)) notice = `Selected ${selected.length} playlists. The ${MAX_PROFILE_ROWS.toLocaleString()}-playlist selection limit stopped paging; additional results remain unselected.`;
      else if (next && items.length >= MAX_PROFILE_ROWS) notice = `Selected ${selected.length} playlists. The ${MAX_PROFILE_ROWS.toLocaleString()}-playlist selection limit stopped paging; more playlists may remain.`;
      else if (next && timedOut) notice = `Selected ${selected.length} playlists. Select all paused after two minutes; more playlists may remain.`;
      else if (next) notice = `Selected ${selected.length} playlists before stopping. More playlists may remain.`;
      else if (terminalComplete) notice = `Selected all ${selected.length} playlists in this browse.`;
      else notice = `Selected the ${selected.length} playlists returned in this browse. The provider did not confirm the list is complete.`;
    } catch (failure) {
      if (isCurrent()) {
        if (selectAllCanceled) {
          selected = [...new Set(items.slice(0, MAX_PROFILE_ROWS).map(item => item.id))];
          notice = `Selected ${selected.length} playlists before stopping.`;
        } else error = `Could not select every playlist: ${String(failure)}`;
      }
    }
    finally { if (isCurrent()) selectingAll = false; }
  }

  function cancelSelectAll() { selectAllCanceled = true; providerReads.cancelAll(); }
  function cancelRestore() { restoreGeneration += 1; browseRequest += 1; providerReads.cancelAll(); loading = false; }
  function clearSelection() { cancelRestore(); selected = []; }

  async function importSelected() {
    if (!selected.length) return;
    const importProvider = provider;
    const playlistIds = [...new Set(selected)];
    if (playlistIds.length > MAX_IMPORT_SELECTION) { error = `Select up to ${MAX_IMPORT_SELECTION} playlists per import.`; return; }
    importing = true; importJob = null; importResults = []; importErrors = []; importTotal = playlistIds.length; activeJobId = null; error = ""; notice = "";
    try {
      cancelRequested = false;
      let job = await call<ImportJob>("import_remote_playlists", { provider: importProvider, playlistIds });
      if (destroyed) return;
      activeJobId = job.jobId || job.id || null;
      importJob = job;
      if (cancelRequested && activeJobId) await call("cancel_remote_playlist_import", { jobId: activeJobId });
      while (activeJobId && !destroyed && !isTerminal(job)) {
        await waitForPoll();
        if (destroyed) return;
        job = await call<ImportJob>("get_remote_playlist_import_job", { jobId: activeJobId });
        if (destroyed) return;
        importJob = job;
      }
      importResults = [...importResults, ...(job.results ?? [])];
      importErrors = [...importErrors, ...(job.errors ?? [])];
      importedIds = [...new Set([...importedIds, ...(job.results ?? []).map(result => result.playlistId).filter((playlistId): playlistId is string => !!playlistId)])];
      if (job.state === "failed" && !job.errors?.length) {
        importErrors = [...importErrors, ...playlistIds.map(playlistId => ({ playlistId, message: job.error || "Import failed." }))];
      }
      activeJobId = null;
      if (destroyed) return;
      const completedCount = importResults.filter(result => result.bindingId || result.collectionId).length;
      if (completedCount) { await onImported(); await loadBindings(); }
      notice = cancelRequested ? `Import canceled. ${completedCount} playlists completed; completed playlists remain in your library.` : `${completedCount} playlist${completedCount === 1 ? "" : "s"} imported.`;
      if (importErrors.length) error = `${importErrors.length} playlist${importErrors.length === 1 ? "" : "s"} could not be imported.`;
    } catch (failure) { if (!destroyed) error = `Import did not finish: ${String(failure)}`; }
    finally { if (!destroyed) importing = false; }
  }

  function waitForPoll() {
    return new Promise<void>(resolve => {
      let finished = false;
      const finish = () => {
        if (finished) return;
        finished = true;
        if (pollTimer !== null) clearTimeout(pollTimer);
        pollTimer = null; finishPollWait = null; resolve();
      };
      finishPollWait = finish;
      pollTimer = setTimeout(finish, 800);
    });
  }

  function addPlaylistUrl() {
    const url = directUrl.trim();
    if (!url) return;
    if (!isProviderUrl(url)) { error = `Enter a valid ${providerName(provider)} playlist URL.`; return; }
    const canonicalUrl = new URL(url).toString();
    if (playlistUrls.includes(canonicalUrl)) { error = "This playlist URL is already in the import list."; return; }
    if (selected.length + 1 > MAX_IMPORT_SELECTION) { error = `Import up to ${MAX_IMPORT_SELECTION} playlists at a time.`; return; }
    cancelRestore(); error = ""; playlistUrls = [...playlistUrls, canonicalUrl]; selected = [...selected, canonicalUrl]; directUrl = "";
  }

  function isProviderUrl(value: string) {
    try {
      const url = new URL(value);
      if (url.protocol !== "https:" || url.username || url.password || (url.port && url.port !== "443")) return false;
      if (provider === "spotify") return url.hostname === "open.spotify.com" && /^\/playlist\/[A-Za-z0-9]{22}(?:\/|$)/.test(url.pathname);
      return ["music.youtube.com", "www.youtube.com", "youtube.com"].includes(url.hostname) && url.pathname === "/playlist" && !!url.searchParams.get("list");
    } catch { return false; }
  }

  async function cancelImport() {
    if (!importing) return;
    cancelRequested = true;
    const jobId = activeJobId;
    if (jobId) {
      try { await call("cancel_remote_playlist_import", { jobId }); }
      catch (failure) { error = `Could not cancel the import: ${String(failure)}`; cancelRequested = false; }
    }
  }

  function toggleSelected(id: string) {
    cancelRestore();
    selected = selected.includes(id) ? selected.filter(value => value !== id) : [...selected, id];
  }

  function playlistName(item: RemotePlaylist) { return item.name || item.title || item.id; }
  function playlistArtwork(item: RemotePlaylist) { return item.imageUrl || item.artworkUrl || ""; }
  function creatorUrl(item: RemotePlaylist) {
    if (item.ownerUrl) {
      if (provider === "spotify") {
        const profileUrl = normalizeSpotifyUserProfileUrl(item.ownerUrl);
        if (profileUrl) return profileUrl;
      } else {
      try {
        const parsed = new URL(item.ownerUrl);
        const official = ["www.youtube.com", "youtube.com", "music.youtube.com"].includes(parsed.hostname);
        if (official && parsed.protocol === "https:" && !parsed.username && !parsed.password && (!parsed.port || parsed.port === "443")) return parsed.toString();
      } catch { /* Fall back to the provider owner ID when it is safe to derive a link. */ }
      }
    }
    let ownerId = item.ownerId?.trim();
    if (!ownerId) return "";
    if (provider === "spotify") {
      if (ownerId.startsWith("spotify:user:")) ownerId = ownerId.slice("spotify:user:".length);
      return normalizeSpotifyUserProfileUrl(`https://open.spotify.com/user/${encodeURIComponent(ownerId)}`) ?? "";
    }
    if (provider === "youtube" && /^@[A-Za-z0-9._-]+$/.test(ownerId)) return `https://www.youtube.com/${ownerId}`;
    if (provider === "youtube" && /^[A-Za-z0-9_-]+$/.test(ownerId)) return `https://www.youtube.com/channel/${encodeURIComponent(ownerId)}`;
    return "";
  }
  async function openCreator(item: RemotePlaylist, event: MouseEvent) {
    event.stopPropagation();
    const url = creatorUrl(item);
    if (!url) return;
    if (provider === "spotify" && onBrowseOwner) {
      cancelRestore();
      onBrowseOwner("spotify", url);
      return;
    }
    try { await call("open_external_url", { url }); }
    catch (failure) { error = `Could not open creator profile: ${String(failure)}`; }
  }
  function importProgress() { return Math.min(importTotal, importJob?.completed ?? 0); }
  function isTerminal(job: ImportJob) { return ["complete", "completed", "partial", "failed", "canceled", "cancelled"].includes((job.state || "").toLowerCase()); }
  const providerName = (value: Provider) => value === "spotify" ? "Spotify" : "YouTube";
  const visibility = (item: RemotePlaylist) => item.visibility || "";
</script>

<section class="remote-playlists" aria-label={provider === "spotify" ? "Spotify playlists" : "YouTube playlists"} bind:this={scrollElement} onscroll={(event) => { scrollTop = (event.currentTarget as HTMLElement).scrollTop; }}>
  {#if profileOnly}
    <header class="remote-profile-heading"><div><span>Public playlists by</span><strong title={profileUrl}>{profileUrl}</strong></div><button disabled={loading || selectingAll || importing} onclick={() => void browse()}>Refresh</button></header>
  {:else}<nav class="remote-provider-tabs" aria-label="Playlist provider">
    <button class:active={provider === "spotify"} aria-pressed={provider === "spotify"} disabled={selectingAll || importing} onclick={() => { invalidateBrowseContext(); provider = "spotify"; playlistUrls = []; directUrl = ""; sourceMode = "mine"; void browse(); }}>Spotify</button>
    <button class:active={provider === "youtube"} aria-pressed={provider === "youtube"} disabled={selectingAll || importing} onclick={() => { invalidateBrowseContext(); provider = "youtube"; playlistUrls = []; directUrl = ""; sourceMode = "mine"; void browse(); }}>YouTube</button>
    <button class="remote-close" aria-label="Close remote playlist picker" onclick={onClose}>×</button>
  </nav>{/if}

  <div class="remote-picker-grid">
    <section class="remote-picker-main" aria-label="Browse remote playlists">
      {#if !profileOnly}<div class="remote-source-tabs" aria-label="Playlist source">
        <button class:active={sourceMode === "mine"} aria-pressed={sourceMode === "mine"} disabled={selectingAll || importing} onclick={() => changeSourceMode("mine")}>Mine</button>
        <button class:active={sourceMode === "profile"} aria-pressed={sourceMode === "profile"} disabled={selectingAll || importing} onclick={() => changeSourceMode("profile")}>Public profile / channel</button>
        <button class:active={sourceMode === "search"} aria-pressed={sourceMode === "search"} disabled={selectingAll || importing} onclick={() => changeSourceMode("search")}>Search</button>
        <button class:active={sourceMode === "url"} aria-pressed={sourceMode === "url"} disabled={selectingAll || importing} onclick={() => changeSourceMode("url")}>Playlist URL</button>
      </div>{/if}
      {#if profileOnly}
        <p class="remote-profile-scope">{providerName(provider)} profile · playlist pages stay scoped to this profile.</p>
      {:else if sourceMode === "profile"}
        <form class="remote-search-form" onsubmit={event => { event.preventDefault(); void browse(); }}>
          <input aria-label={provider === "spotify" ? "Spotify profile name, URL, or user ID" : "YouTube channel URL or ID"} bind:value={profileUrl} oninput={invalidateBrowseContext} disabled={selectingAll || importing} placeholder={provider === "spotify" ? "Spotify profile name, URL, or user ID" : "YouTube channel URL or ID"} />
          <button type="submit" disabled={loading || profileSearchBusy || selectingAll || importing}>Browse URL / ID</button>
          {#if provider === "spotify"}<button type="button" disabled={profileSearchBusy || !profileUrl.trim() || selectingAll || importing} onclick={() => void searchSpotifyProfiles()}>{profileSearchBusy ? "Searching…" : "Search profiles"}</button>{/if}
        </form>
        {#if profileChoices.length}<ul class="remote-profile-choices" aria-label="Spotify profile matches">{#each profileChoices as choice (choice.id)}{@const artwork = safeArtworkUrl(choice.artworkUrl)}<li><span class="profile-art" aria-hidden="true"><span>♫</span>{#if artwork}<img src={artwork} alt="" loading="lazy" onerror={(event) => { (event.currentTarget as HTMLImageElement).hidden = true; }} />{/if}</span><div><strong>{choice.title}</strong>{#if choice.subtitle}<span>{choice.subtitle}</span>{/if}</div><button disabled={!choice.url || loading} onclick={() => browseProfileChoice(choice)}>Browse playlists</button></li>{/each}</ul>{/if}
      {:else if sourceMode === "search"}
        <form class="remote-search-form" onsubmit={event => { event.preventDefault(); void browse(); }}>
          <select aria-label="Search type" bind:value={searchKind} onchange={invalidateBrowseContext}><option value="playlist">Playlists</option><option value="channel">Channels</option></select>
          <input aria-label={`Search ${providerName(provider)}`} bind:value={query} oninput={invalidateBrowseContext} disabled={selectingAll || importing} placeholder="Playlist or channel name" />
          <button type="submit" disabled={loading || selectingAll || importing}>Search</button>
        </form>
      {:else if sourceMode === "url"}
        <form class="remote-search-form remote-url-form" onsubmit={event => { event.preventDefault(); addPlaylistUrl(); }}>
          <input aria-label="Playlist URL" type="url" bind:value={directUrl} placeholder={provider === "spotify" ? "https://open.spotify.com/playlist/…" : "https://music.youtube.com/playlist?list=…"} />
          <button type="submit" disabled={!directUrl.trim()}>Add URL</button>
        </form>
        <p class="remote-url-help">Add an existing public {providerName(provider)} playlist URL, then select and import it. Burrata will not create or modify it.</p>
        {#if playlistUrls.length}
          <ul class="remote-url-list" aria-label="Playlist URLs to import">
            {#each playlistUrls as url (url)}
              <li><label><input type="checkbox" checked={selected.includes(url)} aria-label={`Select ${url}`} onchange={() => toggleSelected(url)} /><span>{url}</span></label><button aria-label={`Remove ${url}`} onclick={() => { cancelRestore(); playlistUrls = playlistUrls.filter(value => value !== url); selected = selected.filter(value => value !== url); }}>×</button></li>
            {/each}
          </ul>
        {:else}<p class="remote-empty">No playlist URLs added.</p>{/if}
      {:else}
        <div class="remote-mine-bar"><span>Your {providerName(provider)} playlists</span><button onclick={() => void browse()} disabled={loading}>Refresh</button></div>
      {/if}

      {#if loading}<p class="remote-loading" role="status">Loading playlists…</p>{/if}
      {#if error}<p class="remote-error" role="alert">{error}</p>{/if}
      {#if notice}<p class="remote-notice" role="status">{notice}</p>{/if}
      {#if pageNote}<p class="remote-notice" role="status">{pageNote}{#if sourceMode === "profile" && snapshotConsistency === "progressive"}{" "}Playlists load progressively; refresh to reflect changes.{/if}</p>
      {:else if sourceMode === "profile" && snapshotConsistency === "progressive"}<p class="remote-notice" role="status">Playlists load progressively; refresh to reflect changes.</p>{/if}
      {#if sourceMode !== "url"}
        <div class="remote-selection-actions">
          <button disabled={!items.length || loading || selectingAll || importing} onclick={() => void selectAllPages()}>{selectingAll ? "Selecting all…" : "Select all across pages"}</button>
          <button disabled={!selected.length || importing || selectingAll} onclick={clearSelection}>Clear selection</button>
        </div>
      {/if}
      {#if selectingAll}
        <div class="remote-progress" role="status"><span>Fetching page {selectAllPageCount + 1} · {selectAllFound} playlists found</span><progress max={MAX_PROFILE_ROWS} value={Math.min(selectAllFound, MAX_PROFILE_ROWS)}></progress><button class="remote-cancel-import" onclick={cancelSelectAll}>Cancel select all</button></div>
      {/if}
      <ul class="remote-result-list" aria-label="Remote playlists">
        {#if items.length}
        {#each items as item (item.id)}
          <li class="remote-result-row">
            <label class="remote-select"><input type="checkbox" checked={selected.includes(item.id)} aria-label={`Select ${item.title || item.name || item.id}`} onchange={() => toggleSelected(item.id)} /></label>
            {#if playlistArtwork(item)}<img class="remote-artwork" src={playlistArtwork(item)} alt="" loading="lazy" />{:else}<div class="remote-artwork remote-no-art" aria-hidden="true">♫</div>{/if}
            <div class="remote-result-copy">
              <button class="remote-result-title" aria-label={`Browse ${playlistName(item)}`} onclick={() => onOpenPlaylist(provider, item.url || item.id)}>{playlistName(item)}</button>
              {#if creatorUrl(item)}{@const ownerLabel = item.ownerName || item.ownerId || "creator"}<button class="remote-creator-link" aria-label={provider === "spotify" && onBrowseOwner ? `Browse ${ownerLabel} playlists` : `Open ${ownerLabel} profile in browser`} title={provider === "spotify" && onBrowseOwner ? `Browse ${ownerLabel} playlists` : `Open ${ownerLabel} profile in browser`} onclick={event => void openCreator(item, event)}>{item.ownerName || item.ownerId || "Creator"}</button>{:else}<span>{item.ownerName || item.ownerId || "Unknown owner"}</span>{/if}
              <span>{item.trackCount != null ? `${item.trackCount} tracks` : ""}{visibility(item) ? `${item.trackCount != null ? " · " : ""}${visibility(item)}` : ""}{item.canEdit ? `${item.trackCount != null || visibility(item) ? " · " : ""}Editable` : ""}</span>
            </div>
            {#if importedIds.includes(item.id) || item.imported}<span class="remote-imported">Imported</span>{/if}
          </li>
        {/each}
        {:else if !loading}<li class="remote-empty">No playlists shown. Choose a source and browse.</li>
        {/if}
      </ul>
      {#if cursor}<button class="remote-more" disabled={loading || selectingAll} onclick={() => void browse(false)}>Load more</button>{/if}
      <footer class="remote-import-footer">
        <span aria-live="polite">{selected.length} selected{selected.length > MAX_IMPORT_SELECTION ? ` · Import is limited to ${MAX_IMPORT_SELECTION} playlists per batch` : ""}</span>
        <button class="primary" disabled={!selected.length || importing || selectingAll} onclick={() => void importSelected()}>{importing ? "Importing…" : sourceMode === "url" && selected.length === 1 ? "Import this playlist" : `Import ${selected.length || "selected"}`}</button>
      </footer>
      {#if importing}<div class="remote-progress" role="status"><span>Importing {importProgress()} of {importTotal} playlists</span><progress max={importTotal || 1} value={importProgress()}></progress><button class="remote-cancel-import" disabled={cancelRequested} onclick={() => void cancelImport()}>{cancelRequested ? "Canceling…" : "Cancel import"}</button></div>{/if}
      {#if importResults.length || importErrors.length}<ul class="remote-import-results" aria-label="Playlist import results">{#each importResults as result, index (`${result.bindingId || result.playlistId || result.id || index}`)}<li><strong>{result.title || result.playlistId || "Playlist"}</strong><span>{result.trackCount != null ? `${result.trackCount} tracks` : "Track count unknown"} {result.complete === false ? "· incomplete" : "· imported"}</span></li>{/each}{#each importErrors as failure (`${failure.playlistId}-${failure.message}`)}<li class="remote-error"><strong>{failure.playlistId}</strong><span>{failure.message}</span></li>{/each}</ul>{/if}
    </section>

  </div>
</section>

<style>
  .remote-playlists { container-type:inline-size; min-width:0; min-height:0; height:100%; overflow:auto; align-content:start; display:grid; gap:6px; color:var(--text); }
  .remote-provider-tabs .remote-close { flex:none; margin-left:auto; padding:3px 9px; font-size:20px; line-height:1; }
  .remote-provider-tabs { align-items:center; }
  .remote-profile-heading { min-width:0; display:flex; align-items:center; gap:10px; padding:5px 8px; border-bottom:1px solid var(--line); }
  .remote-profile-heading>div { min-width:0; display:grid; gap:2px; }
  .remote-profile-heading span,.remote-profile-scope { color:var(--muted); font-size:11px; }
  .remote-profile-heading strong { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:12px; }
  .remote-profile-heading button { flex:none; margin-left:auto; padding:4px 8px; font-size:11px; }
  .remote-profile-scope { margin:0; }
  .remote-provider-tabs,.remote-source-tabs { display:flex; flex-wrap:wrap; gap:4px; padding:0; border:0; border-radius:0; }
  .remote-provider-tabs button,.remote-source-tabs button { min-width:0; padding:5px 8px; background:transparent; color:var(--muted); font-size:11px; }
  .remote-provider-tabs button.active,.remote-source-tabs button.active { background:var(--raised); color:var(--text); }
  .remote-selection-actions { display:flex; flex-wrap:wrap; gap:6px; }
  .remote-selection-actions button { padding:5px 8px; font-size:10px; }
  .remote-picker-grid { display:grid; grid-template-columns:minmax(0,1fr); gap:6px; align-items:start; }
  .remote-picker-main { min-width:0; padding:7px; border:1px solid var(--line); border-radius:8px; background:var(--surface); }
  .remote-picker-main { display:grid; gap:6px; }
  .remote-search-form { min-width:0; display:flex; gap:6px; }
  .remote-profile-choices { display:grid; gap:5px; margin:0; padding:0; list-style:none; }
  .remote-profile-choices li { min-width:0; display:flex; align-items:center; justify-content:space-between; gap:8px; padding:7px; border-radius:6px; background:var(--raised); }
  .profile-art { position:relative; display:grid; flex:0 0 34px; place-items:center; width:34px; height:34px; overflow:hidden; border-radius:50%; background:var(--surface); color:var(--muted); }
  .profile-art img { position:absolute; inset:0; width:100%; height:100%; object-fit:cover; }
  .remote-profile-choices li>div { min-width:0; display:grid; gap:3px; }
  .remote-profile-choices strong,.remote-profile-choices span { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .remote-profile-choices strong { font-size:11px; }
  .remote-profile-choices span { color:var(--muted); font-size:10px; }
  .remote-profile-choices button { flex:none; padding:5px 8px; font-size:10px; }
  .remote-url-help { margin:0; color:var(--muted); font-size:10px; line-height:1.4; }
  .remote-url-list { display:grid; gap:5px; max-height:140px; overflow:auto; margin:0; padding:0; list-style:none; }
  .remote-url-list li { min-width:0; display:flex; align-items:center; gap:6px; padding:6px; border-radius:6px; background:var(--raised); }
  .remote-url-list label { min-width:0; flex:1; display:flex; align-items:center; gap:6px; font-size:10px; }
  .remote-url-list label span { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .remote-url-list li button { flex:none; padding:2px 6px; }
  .remote-search-form input,.remote-search-form select { min-width:0; width:100%; }
  .remote-search-form button,.remote-mine-bar button,.remote-more { flex:none; padding:6px 10px; font-size:11px; }
  .remote-mine-bar { display:flex; align-items:center; justify-content:space-between; gap:8px; color:var(--muted); font-size:11px; }
  .remote-result-list,.remote-import-results { display:grid; gap:4px; min-width:0; max-height:none; overflow:visible; margin:0; padding:0; list-style:none; }
  .remote-result-row { min-width:0; display:grid; grid-template-columns:18px 32px minmax(0,1fr) auto; gap:6px; align-items:center; padding:4px; border-radius:6px; background:var(--raised); }
  .remote-select { display:grid; place-items:center; }
  .remote-artwork { width:32px; height:32px; border-radius:5px; object-fit:cover; background:var(--surface); }
  .remote-no-art { display:grid; place-items:center; color:var(--muted); font-size:19px; }
  .remote-result-copy { min-width:0; display:grid; gap:3px; }
  .remote-result-copy span { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .remote-result-copy .remote-result-title { min-width:0; max-width:100%; justify-self:stretch; padding:0; border:0; background:transparent; color:var(--text); font-size:12px; font-weight:650; line-height:1.35; text-align:left; overflow:visible; text-overflow:clip; white-space:normal; overflow-wrap:anywhere; }
  .remote-result-copy span,.remote-imported { color:var(--muted); font-size:10px; }
  .remote-result-copy .remote-creator-link { justify-self:start; min-width:0; padding:0; border:0; background:transparent; color:var(--accent,#8e9df2); font-size:10px; text-align:left; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .remote-imported { color:var(--success,#5bab8a); }
  .remote-empty,.remote-loading { margin:0; padding:10px; color:var(--muted); font-size:11px; }
  .remote-import-footer { position:sticky; bottom:0; display:flex; align-items:center; justify-content:space-between; gap:8px; padding-top:8px; border-top:1px solid var(--line); background:var(--surface); font-size:11px; }
  .remote-import-footer button { padding:7px 10px; font-size:11px; }
  .remote-progress { display:grid; gap:5px; color:var(--muted); font-size:11px; }
  .remote-progress progress { width:100%; }
  .remote-import-results li { display:flex; justify-content:space-between; gap:8px; font-size:10px; }
  .remote-import-results span { color:var(--muted); }
  .remote-error,.remote-notice { margin:0; font-size:11px; line-height:1.4; overflow-wrap:anywhere; }
  .remote-error { color:var(--danger,#d66); }
  .remote-notice { color:var(--success,#5bab8a); }
  @container (max-width:650px) { .remote-picker-grid { grid-template-columns:minmax(0,1fr); } }
  @container (max-width:390px) { .remote-result-row { grid-template-columns:18px 32px minmax(0,1fr); } .remote-artwork { width:32px;height:32px; }.remote-imported { grid-column:3; } .remote-search-form { flex-wrap:wrap; }.remote-search-form input { flex:1 1 120px; } }
</style>
