<script lang="ts">
  import {shazamRegions,shazamGenres} from "./lib/source-presets";
  let { provider, onImport, onOpen }: {provider:string; onImport:(url:string)=>Promise<void>; onOpen:(url:string)=>void} = $props();
  let region=$state('world'), chartType=$state('top-200'), tag=$state('afro house'), period=$state('7day');
  let username=$state(''), importing=$state(false);
  try { username=localStorage.getItem('burrata.lastfmUsername') ?? ''; } catch {}
  const countries=shazamRegions;
  let shazamGenre=$state("house");
  const tags=['afro house','deep house','house','jazz','soul','funk','latin','folk','electronic','indie'];
  const periods=[['7day','Last 7 days','LAST_7_DAYS'],['1month','Last 30 days','LAST_30_DAYS'],['3month','Last 90 days','LAST_90_DAYS'],['6month','Last 180 days','LAST_180_DAYS'],['12month','Last year','LAST_365_DAYS'],['overall','All time','ALL_TIME']];
  const bandcampGenres=[['electronic','Electronic'],['jazz','Jazz'],['r-b-soul','R&B / Soul'],['funk','Funk'],['folk','Folk'],['world','World'],['hip-hop-rap','Hip-hop / Rap'],['ambient','Ambient'],['rock','Rock'],['latin','Latin'],['experimental','Experimental']];
  const shazamUrl=$derived(chartType === "genre" ? `https://www.shazam.com/charts/genre/${region}/${shazamGenre}` : `https://www.shazam.com/charts/${chartType}/${region}`);
  const tagUrl=$derived(`https://www.last.fm/tag/${encodeURIComponent(tag).replace(/%20/g,'+')}/tracks`);
  const userUrl=$derived(`https://www.last.fm/user/${encodeURIComponent(username.trim())}/library/tracks?date_preset=${periods.find(p=>p[0]===period)?.[2] ?? 'LAST_7_DAYS'}`);
  async function importUrl(url:string) { importing=true;try { await onImport(url); } finally { importing=false; } }
  function rememberUsername() { try {localStorage.setItem('burrata.lastfmUsername',username.trim());}catch{} }
  const label=(value:string)=>value.split(/[ -]/).map(word=>word.charAt(0).toUpperCase()+word.slice(1)).join(' ');
</script>

<section class="presets" aria-label={`${provider === 'lastfm' ? 'Last.fm' : provider === 'shazam' ? 'Shazam' : 'Bandcamp'} presets`}>
  {#if provider === 'shazam'}
    <h2>Charts</h2><p>Choose a region and import its latest chart. Top 200 imports directly; other charts may require capture in the companion browser.</p>
    <div class="controls"><label>Region<select aria-label="Shazam region" bind:value={region}>{#each countries as [value,name]}<option {value}>{name}</option>{/each}</select></label>
    <label>Chart<select aria-label="Shazam chart" bind:value={chartType}><option value="top-200">Top 200</option><option value="viral">Viral</option><option value="genre">Genre charts</option></select></label>{#if chartType === "genre"}<label>Genre<select aria-label="Shazam genre" bind:value={shazamGenre}>{#each shazamGenres as [value,name]}<option {value}>{name}</option>{/each}</select></label>{/if}</div>
    <div class="actions"><button class="primary" disabled={importing} onclick={()=>void importUrl(shazamUrl)}>{importing?'Importing…':'Import chart'}</button><button onclick={()=>onOpen(shazamUrl)}>Open in browser ↗</button></div>
  {:else if provider === 'lastfm'}
    <h2>Top tracks</h2><p>Import popular tracks globally, by genre or from a Last.fm profile.</p>
    <button class="primary" disabled={importing} onclick={()=>void importUrl('https://www.last.fm/charts')}>Import global top tracks</button>
    <div class="preset-block"><label>Genre / tag<select aria-label="Last.fm genre" bind:value={tag}>{#each tags as value}<option {value}>{label(value)}</option>{/each}</select></label><div class="actions"><button disabled={importing} onclick={()=>void importUrl(tagUrl)}>Import genre tracks</button><button onclick={()=>onOpen(tagUrl)}>Open in browser ↗</button></div></div>
    <details class="preset-block"><summary>Profile top tracks</summary><div class="controls"><label>Username<input aria-label="Last.fm username" bind:value={username} onchange={rememberUsername} placeholder="Your Last.fm username" autocomplete="off" /></label><label>Period<select aria-label="Last.fm period" bind:value={period}>{#each periods as [value,name]}<option {value}>{name}</option>{/each}</select></label></div><button disabled={importing || !username.trim() || !/^[a-zA-Z0-9_-]+$/.test(username.trim())} onclick={()=>void importUrl(userUrl)}>Import profile top tracks</button></details>
  {:else if provider === 'bandcamp'}
    <h2>Discover albums</h2><p>Browse a genre in your browser, choose an album, then use the Burrata companion to import its tracklist. Saved albums appear under Bandcamp.</p>
    <div class="genre-grid">{#each bandcampGenres as [genre,name]}<button onclick={()=>onOpen(`https://bandcamp.com/discover/${genre}`)}>{name} <span aria-hidden="true">↗</span></button>{/each}</div>
    <button class="browse-all" onclick={()=>onOpen('https://bandcamp.com/discover')}>Browse all Bandcamp music ↗</button>
  {/if}
</section>
<style>
.presets{max-width:760px;padding:20px;box-sizing:border-box}.presets h2{margin:0 0 8px;font-size:22px}.presets p{color:var(--muted);font-size:13px;line-height:1.5;max-width:600px;margin:0 0 20px}.controls{display:flex;flex-wrap:wrap;gap:12px}.presets label{display:grid;gap:6px;font-size:12px;color:var(--muted);min-width:0;flex:1;max-width:320px}.presets select,.presets input{min-width:0;width:100%;box-sizing:border-box;background:var(--raised);border:1px solid var(--line);border-radius:7px;color:var(--text);padding:10px}.presets option{background:var(--surface);color:var(--text)}.actions{display:flex;flex-wrap:wrap;gap:8px;margin-top:14px}.presets button{border:1px solid var(--line);border-radius:7px;background:var(--raised);color:var(--text);padding:10px 14px;font-size:12px;text-align:left}.presets button:hover{border-color:var(--accent)}.presets button:disabled{opacity:.5}.presets button.primary{background:var(--accent);color:var(--base);border-color:var(--accent);font-weight:650}.preset-block{border-top:1px solid var(--line);margin-top:24px;padding-top:20px}.preset-block summary{cursor:pointer;color:var(--text);font-size:13px;margin-bottom:12px}.preset-block>button{margin-top:14px}.genre-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));gap:10px}.genre-grid button{display:flex;justify-content:space-between}.genre-grid span{color:var(--muted)}.browse-all{margin-top:14px}@media(max-width:600px){.presets{padding:14px 8px}.controls{display:grid;grid-template-columns:1fr}.presets label{max-width:none}.actions button{flex:1}.genre-grid{grid-template-columns:repeat(2,minmax(0,1fr));gap:8px}}
</style>
