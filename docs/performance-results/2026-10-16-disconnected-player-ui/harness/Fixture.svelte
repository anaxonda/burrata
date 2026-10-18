<script lang="ts">
  import TrackList from "/home/loki/projects/burrata/src/TrackList.svelte";
  import PlaybackQueuePanel from "/home/loki/projects/burrata/src/PlaybackQueuePanel.svelte";
  import PlayingInfo from "/home/loki/projects/burrata/src/PlayingInfo.svelte";
  import PlaybackQuality from "/home/loki/projects/burrata/src/PlaybackQuality.svelte";
  import SeekSlider from "/home/loki/projects/burrata/src/SeekSlider.svelte";
  import type { Track, Chapter } from "/home/loki/projects/burrata/src/lib/types";

  const source = (id: string, title: string, artist: string): Track => ({
    id, provider: "youtube", providerId: id, sourceUrl: "https://example.invalid/track",
    artist, title, album: "A very long album name to exercise constrained track layouts",
    durationSeconds: 272, favorite: false, provenance: {}, localAvailability: "none",
  });
  const tracks: Track[] = [
    source("track-long", "A Very Long Track Title That Must Stay Readable In A Narrow Player Pane (Extended Mix)", "An Artist With A Deliberately Long Credit Name"),
    source("track-unavailable", "Unavailable Recording With No Local Audio Source", "Unknown Artist"),
    source("track-upcoming", "Upcoming Track", "Another Artist"),
  ];
  const chapters: Chapter[] = [
    { id: "chapter-a", title: "Intro with a deliberately long chapter name", startTime: 0, endTime: 52, confidence: "high" },
    { id: "chapter-b", title: "Main section", startTime: 52, endTime: 184, confidence: "high" },
    { id: "chapter-c", title: "Outro", startTime: 184, endTime: 272, confidence: "high" },
  ];
  const params = new URLSearchParams(location.search);
  let panel = $state<"queue" | "details" | null>(params.get("panel") === "queue" || params.get("panel") === "details" ? params.get("panel") as "queue" | "details" : null);
  let paused = $state(params.get("state") === "paused");
  let buffering = $state(params.get("state") === "buffering");
  let unavailable = $state(params.get("state") === "unavailable");
  let identifying = $state(params.get("identify") === "active");
  const showControls = params.get("controls") === "1";
  const doNothing = () => {};
  const chapterRange = {
    parentTrackId: "track-long", chapterId: "chapter-b", sourceUrl: tracks[0].sourceUrl,
    startTime: 52, endTime: 184, parentTitle: tracks[0].title,
  };
</script>

<div class="app-shell" class:has-player-panel={panel !== null} style="--player-height:96px;--library-sidebar-width:184px;--info-panel-width:340px">
  <aside class="sidebar"><strong>Burrata</strong><nav><button>Library</button><button>Queue</button><button>Radio</button></nav></aside>
  <main>
    <header>
      <div class="navigation-controls"><button aria-label="Go back" disabled>←</button><button aria-label="Go forward" disabled>→</button></div>
      <div><div class="eyebrow">DISCOVER · AUDITION · COLLECT</div><h1>Library</h1></div>
      <div class="header-actions"><button>＋ New playlist</button></div>
    </header>
    <div class="toolbar library-toolbar"><button>All recordings</button><button>Favorites</button><button>Playlists</button></div>
    <div class="workspace">
      <section class="track-area">
        <div class="table-tools"><input placeholder="Filter this view…" aria-label="Filter this view" /><select aria-label="Sort tracks"><option>Title</option></select></div>
        <TrackList {tracks} playingTrack={tracks[0]} bufferingTrack={buffering ? tracks[0] : null} {paused}
          onPlay={doNothing} onDownload={doNothing} onFavorite={doNothing} onNext={doNothing}
          onQueue={doNothing} onPlaylist={doNothing} onArtist={doNothing} onInfo={doNothing} />
      </section>
    </div>
  </main>
  {#if panel !== null}
    <section class="player-panel-host" aria-label={panel === "queue" ? "Playback queue panel" : "Now playing details panel"}>
      <nav class="player-panel-tabs"><button class:active={panel === "details"} onclick={() => panel = "details"}>Now playing</button><button class:active={panel === "queue"} onclick={() => panel = "queue"}>Queue</button><button aria-label="Close player panel" onclick={() => panel = null}>×</button></nav>
      {#if panel === "queue"}
        <PlaybackQueuePanel tracks={tracks} currentIndex={0} position={118} chapterPosition={118} {chapters} paused={paused} buffering={buffering}
          activePlaybackTrack={{ ...tracks[0], playbackRange: chapterRange }} chapterRangesByTrack={{ "track-long": { "chapter-b": chapterRange } }}
          chaptersByTrack={{ "track-long": chapters }} repeatMode="off" shuffleEnabled={false} onClose={() => panel = null}
          onPlay={doNothing} onMove={doNothing} onRemove={doNothing} onSeek={doNothing}
          onToggleShuffle={doNothing} onCycleRepeat={doNothing}
          onIdentifyCurrent={doNothing} onCancelIdentifyCurrent={() => identifying = false}
          identifyingCurrentAudio={identifying} radioIdentifyProgress={3.5} />
      {:else}
        <PlayingInfo docked track={tracks[0]} onClose={() => panel = null} onArtist={doNothing} onDownload={doNothing} onUpgrade={doNothing} onPlaylist={doNothing}
          onPlay={doNothing} onFavorite={doNothing} onRadio={doNothing} streamQuality={{codec:"AAC",bitrateKbps:128,sampleRate:48000,channels:2}}
          onIdentifyRadio={doNothing} onCancelIdentifyRadio={() => identifying = false} identifyingRadio={identifying} identifyProgress={3.5} />
      {/if}
    </section>
  {/if}
  <footer class="player">
    <button class="now-playing click-info" aria-label="Show playing track information" disabled={unavailable}>
      <div class="mini-art">♫</div><div class="player-track-copy">
        <strong class="player-track-title">{unavailable ? "Audio source unavailable" : tracks[0].title}</strong>
        <small class="player-track-artist">{tracks[0].artist}</small>
        <small class="player-status" role="status">{buffering ? "Buffering audio…" : paused ? "Paused" : unavailable ? "Choose a source" : "Playing"}</small>
        <PlaybackQuality compact track={tracks[0]} streamQuality={{codec:"AAC",bitrateKbps:128,sampleRate:48000,channels:2}} />
        <small class="player-source" title="https://media.example.invalid/very/long/path/to/a/streaming/audio/source">Audio preview · https://media.example.invalid/very/long/path/to/a/streaming/audio/source</small>
      </div>
    </button>
    {#if unavailable}<div class="player-error" role="alert"><p>This recording is unavailable. Choose another audio source.</p><div class="player-error-actions"><button>Retry</button><button>Track info</button></div></div>{/if}
    <div class="player-center">
      <div class="player-transport">
        <button aria-label="Shuffle" aria-pressed="false">⤨</button>
        <button aria-label="Autoplay radio after queue" title="Continue with radio based on the last song" aria-pressed="false">∞ Radio</button>
        <button aria-label="Previous recording">⏮</button>
        <button class="play-button" class:buffering aria-busy={buffering} aria-label={buffering ? "Pause loading playback" : paused ? "Play" : "Pause"}>{buffering ? "Ⅱ" : paused ? "▶" : "Ⅱ"}</button>
        <button aria-label="Next recording">⏭</button>
        <button aria-label="Repeat: off" aria-pressed="false">↻</button>
      </div>
      <div class="player-scrubber"><span class="time">1:58</span><SeekSlider position={118} duration={272} disabled={false} onSeek={doNothing} /><span class="time">4:32</span></div>
    </div>
    <div class="player-right">
      <button class="player-heart" aria-label="Add playing track to favorites" aria-pressed="false" title="Favorites">♡</button>
      <button class="queue-toggle" aria-label="Show playback queue and chapters" title="Queue and chapters" aria-pressed={panel === "queue"} onclick={() => panel = panel === "queue" ? null : "queue"}><svg aria-hidden="true" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M3 5h16M3 10h16M3 15h8"/><path d="m16 14 5 4-5 4z"/></svg><span>Queue</span></button>
      <label class="player-volume-control"><span aria-hidden="true">◖))</span><input class="volume" type="range" min="0" max="1" step="0.01" value=".8" aria-label="Volume" /></label>
      <button aria-label="Show track details" onclick={() => panel = panel === "details" ? null : "details"}>ⓘ</button>
    </div>
  </footer>
  {#if showControls}<div class="fixture-controls" aria-label="Fixture states">
    <button onclick={() => buffering = !buffering}>Toggle buffering</button><button onclick={() => paused = !paused}>Toggle paused</button><button onclick={() => unavailable = !unavailable}>Toggle unavailable/error</button><button onclick={() => panel = panel === "queue" ? null : "queue"}>Expanded queue</button><button onclick={() => panel = panel === "details" ? null : "details"}>Expanded details</button>
  </div>{/if}
</div>

<style>
  .fixture-controls { position:fixed;z-index:100;left:8px;top:8px;display:flex;gap:4px;flex-wrap:wrap;max-width:min(680px,calc(100vw - 16px));padding:4px;border:1px solid var(--line);background:var(--surface); }
  .fixture-controls button { min-height:28px;padding:4px 7px;font-size:10px; }
  .sidebar nav { display:grid;gap:5px;margin-top:20px; }
  .sidebar nav button { width:100%;text-align:left; }
</style>
