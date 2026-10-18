<script lang="ts">
  import DownloadActivity from "/home/loki/projects/burrata/src/DownloadActivity.svelte";
  import VirtualResults from "/home/loki/projects/burrata/src/VirtualResults.svelte";
  import type { DownloadJob, Track } from "/home/loki/projects/burrata/src/lib/types";
  const params = new URLSearchParams(location.search);
  const mode = params.get("mode") === "activity" ? "activity" : "all";
  const makeTrack = (index: number): Track => ({
    id: `fixture-track-${index}`, provider: "youtube", providerId: `source-${index}`,
    sourceUrl: `https://example.invalid/${index}`, artist: index < 2 ? "A very long artist credit that tests narrow activity rows" : "Example artist",
    title: index === 0 ? "A Very Long Recording Title With An Extended Mix Version That Wraps Across Several Lines" : `Historical download ${index}`,
    version: index === 0 ? "Collector's extended club version, remastered and re-edited" : undefined,
    durationSeconds: 241, album: "Fixture album", favorite: false, provenance: {},
  });
  const jobs: DownloadJob[] = Array.from({ length: 500 }, (_, index) => {
    const track = makeTrack(index);
    return {
      id: `job-${index}`, track,
      candidate: { id: `candidate-${index}`, provider: "youtube", url: "https://example.invalid/audio", title: track.title, durationSeconds: 241, formats: [], permission: "allowed", confidence: "high", score: 90, reasons: [], official: false },
      state: index === 1 || index === 250 ? "failed" : index === 0 ? "downloading" : "completed",
      progress: 38, error: index === 1 ? "The selected source timed out while checking a long, malformed remote recording response." : null,
      outputPath: `/music/fixture-${index}.mp3`, createdAt: "2026-10-16T10:00:00Z", outputProfile: "source", formatId: null,
    };
  });
  let action = $state("");
  let sortField = $state("sourceOrder");
  let direction = $state("asc");
  let minimumDuration = $state("");
  let maximumDuration = $state("");
  const viewKey = $derived(`downloads:${sortField}:${direction}:${minimumDuration}:${maximumDuration}`);
  const noAction = () => {};
  const openAll = () => location.assign("?mode=all");
</script>

<div class="app-shell" style="--player-height:92px;--library-sidebar-width:64px;--info-panel-width:340px">
  <aside class="library-sidebar"><div class="brand-mark">B</div><nav><button aria-label="Library">▤</button><button aria-label="Search">⌕</button></nav></aside>
  <main>
    <header><div class="navigation-controls"><button aria-label="Go back">←</button><button aria-label="Go forward" disabled>→</button></div><div><div class="eyebrow">DOWNLOADS · HISTORY</div><h1>Downloads</h1></div><div class="header-actions"><button>Refresh queue</button></div></header>
    {#if mode === "all"}
      <div class="toolbar">
        <span>{jobs.length} jobs</span>
        <label>Sort downloads<select aria-label="Sort download jobs" bind:value={sortField}><option value="sourceOrder">Original order</option><option value="title">Title</option><option value="artist">Artist</option><option value="duration">Duration</option></select></label>
        <label>Direction<select aria-label="Download sort direction" bind:value={direction}><option value="asc">Ascending</option><option value="desc">Descending</option></select></label>
        <label>Minimum duration<input aria-label="Download minimum duration" bind:value={minimumDuration} placeholder="seconds or mm:ss" /></label>
        <label>Maximum duration<input aria-label="Download maximum duration" bind:value={maximumDuration} placeholder="seconds or mm:ss" /></label>
      </div>
      <section class="queue">
        <VirtualResults items={jobs} itemKey={(job) => job.id} estimateHeight={() => 112} scrollKey={viewKey} regionKey="download-jobs" scrollRootSelector=".queue" ariaLabel="Download job rows">
          {#snippet children(job)}<article class="job">
            <div><strong>{job.track.title}</strong><p>{job.track.artist}{#if job.track.version} · {job.track.version}{/if}</p><p>YouTube</p><small>{job.state}{#if job.state === "downloading"} · {job.progress}%{/if}</small>{#if job.error}<p class="error-text">{job.error}</p>{/if}</div>
            <div>{#if job.state === "failed"}<button onclick={() => action = `retry:${job.id}`}>Retry</button>{:else if job.state !== "completed"}<button onclick={() => action = `cancel:${job.id}`}>Cancel</button>{/if}<button onclick={() => action = `show:${job.outputPath}`}>Show file</button></div>
          </article>{/snippet}
        </VirtualResults>
      </section>
      <output class="fixture-result">{action}</output>
    {:else}
      <div class="activity-background" aria-hidden="true"></div>
      <DownloadActivity {jobs} onClose={noAction} onRefresh={noAction} onCancel={noAction} onRetry={noAction} onShowFile={noAction} onViewAll={openAll} />
    {/if}
  </main>
  <footer class="player player-reserved">Playback area reserved in this disconnected Downloads fixture</footer>
</div>

<style>
  .player-reserved { display:flex; align-items:center; justify-content:center; color:var(--muted); font-size:11px; }
</style>
