const fs = require("node:fs");
const vm = require("node:vm");
const assert = require("node:assert/strict");
const ts = require("typescript");

const app = fs.readFileSync("src/App.svelte", "utf8").split('<script lang="ts">')[1].split("</script>")[0];
const ast = ts.createSourceFile("app.ts", app, ts.ScriptTarget.Latest, true);
const names = ["startPreview", "startPreviewInner", "sameNativePlaybackTrack", "sameNativePlaybackQueue", "nativeQueueSelectIdentity", "playQueueIndex", "playArtistTrack", "rememberQueueCandidate", "inspectQueueChapters", "sourceIsAuthoritative"];
const functions = ast.statements.filter(node => ts.isFunctionDeclaration(node) && names.includes(node.name?.text));
assert.equal(functions.length, names.length, "all playback functions were extracted from App.svelte");
const lock = fs.readFileSync("src/lib/source-lock.ts", "utf8").replace(/^import .*;$/gm, "").replace(/export /g, "");
const javascript = ts.transpileModule(`${lock}\n${functions.map(node => node.getText(ast)).join("\n")}`, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None },
}).outputText;

function deferred() {
  let resolve;
  const promise = new Promise(accept => { resolve = accept; });
  return { promise, resolve };
}

function context() {
  const commands = [];
  const value = {
    commands, console, clearTimeout, portablePlaybackSource: () => null, error: "",
    clearPlaybackSkipNotice: () => {}, nativeSnapshot: null, playbackLoadSequence: 0,
    resolvingPlayback: false, playbackOperationTrace: () => ({}), playbackRunGeneration: 0,
    cancelRadioIdentification: () => {}, autoTimer: undefined, setAutomationPlaybackOwner: () => {},
    systemPausedTrack: false, queueTracks: [], view: "search", historyItems: [], sorted: [],
    queueCandidates: {}, audioSearchTrackIds: new Set(), settings: {}, candidateForQueue: () => null,
    importedAudioCandidate: track => track.provenance?.audioCandidate,
    nativeCommand: async command => commands.push(command), call: async () => {},
    artistPlaybackMatchGeneration: 0, matchArtistPlaybackSources: async tracks => tracks, selected: null,
  };
  vm.createContext(value);
  vm.runInContext(javascript, value);
  return value;
}

const track = id => ({
  id, provider: "youtube", providerId: id, title: id, artist: "Artist",
  sourceUrl: `https://www.youtube.com/watch?v=${id}`, provenance: {},
});

(async () => {
  {
    const app = context();
    app.call = async () => { throw new Error("playback must not use a separate album-context IPC preflight"); };
    const resourceA = { resourceKey: "album:A", scope: "album" };
    const resourceB = { resourceKey: "album:B", scope: "album" };
    await app.startPreview(track("A"), false, false, [track("A")], 0, 0, null, false, undefined, resourceA);
    await app.startPreview(track("B"), false, false, [track("B")], 0, 0, null, false, undefined, resourceB);
    assert.deepEqual(app.commands.map(command => command.tracks[0].id), ["A", "B"]);
    assert.equal(app.commands[0].loudnessAlbumResource, resourceA);
    assert.equal(app.commands[1].loudnessAlbumResource, resourceB);
  }

  {
    const app = context();
    const gate = deferred();
    app.matchArtistPlaybackSources = () => gate.promise;
    const oldPlay = app.playArtistTrack(track("A"));
    await app.startPreview(track("B"), false, false, [track("B")], 0);
    gate.resolve([track("A")]);
    await oldPlay;
    assert.deepEqual(app.commands.map(command => command.tracks[0].id), ["B"]);
  }

  {
    const app = context();
    const old = track("X");
    old.provenance = { playbackMode: "source" };
    const refreshed = { ...old, providerId: "catalog-X", isrc: "USAAA2600001", provenance: { playbackMode: "lookup" } };
    assert.equal(app.sameNativePlaybackTrack(old, refreshed), false);
    app.nativeSnapshot = { queue: [old] };
    await app.startPreview(refreshed, false, false, [refreshed], 0);
    assert.equal(app.commands[0].action, "play");
    assert.equal(app.commands[0].tracks[0].isrc, "USAAA2600001");
  }

  {
    const app = context();
    const parent = track("X");
    const candidate = { id: "youtube:X", provider: "youtube", url: parent.sourceUrl, title: "X" };
    parent.provenance = { playbackMode: "lookup", audioCandidate: candidate };
    assert.equal(app.sourceIsAuthoritative(parent), false);
    app.call = async () => ({ ...candidate, chapters: [] });
    await app.inspectQueueChapters(parent);
    assert.equal(app.sourceIsAuthoritative(parent), false);
    await app.startPreview(parent, false, false, [parent], 0);
    assert.equal(app.commands[0].action, "play");
    assert.equal(app.commands[0].candidate, undefined);

    const manuallyPinned = { ...parent, provenance: { ...parent.provenance, userSelectedSource: true } };
    assert.equal(app.sourceIsAuthoritative(manuallyPinned), true);
  }

  {
    const app = context();
    const rows = [track("same"), track("same")];
    app.queueTracks = rows;
    app.nativeSnapshot = { queue: rows, queueRevision: 23, queueEntryIds: ["occurrence-a", "occurrence-b"] };
    app.playQueueIndex(1);
    assert.equal(JSON.stringify(app.commands[0]), JSON.stringify({ action: "select", index: 1, expectedQueueRevision: 23, expectedEntryId: "occurrence-b" }));
    app.commands.length = 0;
    app.nativeSnapshot.queueEntryIds = ["duplicate-entry", "duplicate-entry"];
    app.playQueueIndex(1);
    assert.equal(app.commands.length, 0, "ambiguous occurrence identity is rejected");
  }

  {
    const app = context();
    const displayed = [track("A"), track("B"), track("C")];
    app.queueTracks = displayed;
    app.nativeSnapshot = { queue: [displayed[0], displayed[2], displayed[1]], queueRevision: 24, queueEntryIds: ["A", "C", "B"] };
    app.playQueueIndex(1);
    assert.equal(app.commands.length, 0, "stale displayed index is rejected");
    assert.match(app.error, /Refresh the queue/);
  }

  {
    const app = context();
    const row = track("reuse");
    app.nativeSnapshot = { queue: [row] };
    await app.startPreview(row, false, false, [row], 0);
    assert.equal(app.commands.length, 0, "queue reuse without occurrence identity does not fall back to select or replace play");
    assert.match(app.error, /Refresh the queue/);
  }

  {
    const app = context();
    const row = track("reuse");
    app.nativeSnapshot = { queue: [row], queueRevision: 25, queueEntryIds: ["entry-25"] };
    await app.startPreview(row, false, false, [row], 0);
    assert.equal(JSON.stringify(app.commands[0]), JSON.stringify({ action: "select", index: 0, expectedQueueRevision: 25, expectedEntryId: "entry-25", loudnessAlbumResource: null }));
  }

  console.log("UI playback race and queue occurrence regression checks passed (8 cases).");
})().catch(failure => { console.error(failure); process.exitCode = 1; });
