import test from 'node:test';
import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
import {JSDOM} from 'jsdom';
import {extract,mergeSnapshots,providerFor,isVerification} from '../parsers.js';
const doc=(html,url='https://www.last.fm/charts')=>new JSDOM(html,{url}).window.document;
test('Traxsource preserves explicit rank, version and contributor separation',()=>{
 const s=extract(doc('<h1>Deep House Top 100</h1><div class="trk-row"><span class="tnum">8</span><div class="title"><a href="/track/123/hello">Hello</a><span class="version">Extended Mix</span> (6:32)</div><div class="artists"><a>Artist</a><span class="remixers"><a>Remixer</a></span></div><div class="label">Label</div></div>'),'https://www.traxsource.com/genre/13/deep-house/top');
 assert.equal(s.entries[0].artist,'Artist');assert.equal(s.entries[0].rank,8);assert.equal(s.entries[0].durationSeconds,392);assert.equal(s.expectedTotal,100);assert.equal(mergeSnapshots([s]).complete,false);
});
test('Beatport discards auxiliary unranked tracks and private fields',()=>{
 const s=extract(doc('<h1>House Top 2</h1><script id="__NEXT_DATA__">'+JSON.stringify({token:'secret',data:[{id:9,name:'Related',artists:[{name:'X'}]},{position:2,track:{id:12,name:'Chart',artists:[{name:'Y'}],mix_name:'Radio Edit',length_ms:180000,authorization:'secret'}}]})+'</script>'),'https://www.beatport.com/genre/house/5/top-100');
 assert.equal(s.entries.length,1);assert.equal(s.entries[0].rank,2);assert.equal(s.entries[0].version,'Radio Edit');assert.ok(!JSON.stringify(s).includes('secret'));
});
test('Spotify merges virtualized rows and retains repeated occurrences',()=>{
 const make=(position,id)=>extract(doc(`<h1>My playlist</h1><p>3 songs</p><div data-testid="tracklist-row" aria-rowindex="${position}"><a href="/track/${id}">Song</a><a href="/artist/A">Artist</a><a href="/album/B">Album</a>3:10</div>`),'https://open.spotify.com/playlist/PL');
 const s=mergeSnapshots([make(1,'ABC'),make(2,'DEF'),make(3,'ABC'),make(1,'ABC')]);assert.deepEqual(s.entries.map(x=>x.position),[1,2,3]);assert.equal(s.complete,true);assert.equal(s.entries[0].rank,null);
});
test('Spotify converts grid row indexes to playlist positions when a header row exists',()=>{
 const page=doc(`<h1>My playlist</h1><p>2 songs</p><div role="grid"><div role="row" aria-rowindex="1"><div role="columnheader">#</div><div role="columnheader">Title</div></div><div data-testid="tracklist-row" role="row" aria-rowindex="2"><div role="gridcell">Song one</div><a href="/track/ONE">Song one</a><a href="/artist/A">Artist</a></div><div data-testid="tracklist-row" role="row" aria-rowindex="3"><div role="gridcell">Song two</div><a href="/track/TWO">Song two</a><a href="/artist/A">Artist</a></div></div>`);
 const s=extract(page,'https://open.spotify.com/playlist/PL');
 assert.deepEqual(s.entries.map(e=>e.position),[1,2]);
 assert.equal(mergeSnapshots([s]).complete,true);
});
test('Spotify recognizes singular playlist totals',()=>{
 const page=doc('<h1>One track</h1><p>1 song</p><div data-testid="tracklist-row" aria-rowindex="1"><a href="/track/ONE">Song</a><a href="/artist/A">Artist</a></div>');
 const s=extract(page,'https://open.spotify.com/playlist/PL');
 assert.equal(s.expectedTotal,1);assert.equal(mergeSnapshots([s]).complete,true);
});
test('Spotify album captures preserve ordered tracks and apply album title and artwork',()=>{
 const page=doc('<main><h1>Night Drive</h1><p>2 songs, 7 min</p><img src="https://i.scdn.co/image/cover"><div data-testid="tracklist-row" aria-rowindex="1"><a href="/track/ONE">First</a><a href="/artist/A">Artist</a></div><div data-testid="tracklist-row" aria-rowindex="2"><a href="/track/TWO">Second</a><a href="/artist/B">Another artist</a></div></main>');
 const s=mergeSnapshots([extract(page,'https://open.spotify.com/album/ALBUM')]);
 assert.equal(s.kind,'album');assert.equal(s.expectedTotal,2);assert.equal(s.complete,true);
 assert.deepEqual(s.entries.map(e=>e.position),[1,2]);assert.ok(s.entries.every(e=>e.album==='Night Drive'&&e.artworkUrl==='https://i.scdn.co/image/cover'));
 assert.throws(()=>extract(page,'https://open.spotify.com/album/ALBUM/extra'),/playlist or album/);
});
test('Spotify album name includes only its explicit header artist; playlist names stay as captured',()=>{
 const album=doc('<main><section data-testid="entityTitle"><h1>Night Drive</h1><a href="/artist/ALBUMARTIST">The Album Artist</a></section><div data-testid="tracklist-row" aria-rowindex="1"><a href="/track/ONE">First</a><a href="/artist/TRACKARTIST">Featuring Guest</a></div></main>');
 const albumSnapshot=extract(album,'https://open.spotify.com/album/ALBUM');
 assert.equal(albumSnapshot.title,'The Album Artist — Night Drive');
 assert.equal(albumSnapshot.entries[0].artist,'Featuring Guest');
 const playlist=doc('<main><h1>Night Drive</h1><div data-testid="tracklist-row" aria-rowindex="1"><a href="/track/ONE">First</a><a href="/artist/TRACKARTIST">Featuring Guest</a></div></main>');
 assert.equal(extract(playlist,'https://open.spotify.com/playlist/PLAYLIST').title,'Night Drive');
 // If Spotify provides no album-header artist, track-row metadata is not a fallback.
 assert.equal(extract(playlist,'https://open.spotify.com/album/ALBUM').title,'Night Drive');
});
test('Spotify localized playlist and album URLs capture with stable unlocalized source URLs',()=>{
 const page=doc('<main><h1>Localized collection</h1><p>1 song</p><div data-testid="tracklist-row" aria-rowindex="1"><a href="/track/TRACK">Track</a><a href="/artist/ARTIST">Artist</a></div></main>');
 for(const [kind,url] of [['playlist','https://open.spotify.com/intl-en/playlist/PLAYLIST?si=share'],['album','https://open.spotify.com/intl-es-MX/album/ALBUM?si=share']]) {
  const snapshot=extract(page,url);
  assert.equal(snapshot.kind,kind);
  assert.equal(snapshot.sourceUrl,`https://open.spotify.com/${kind}/${kind==='playlist'?'PLAYLIST':'ALBUM'}`);
 }
});
test('Unknown total never implies complete and missing positions prevent completeness',()=>{
 const base={entries:[{position:1,providerId:'1'},{position:3,providerId:'3'}],warnings:[],expectedTotal:null};assert.equal(mergeSnapshots([base]).complete,false);assert.equal(mergeSnapshots([{...base,expectedTotal:2}]).complete,false);
});
test('Rejects unsupported origins, insecure pages and provider challenge',()=>{
 assert.equal(providerFor('https://www.beatport.com.evil.invalid/'),null);assert.throws(()=>providerFor('http://www.beatport.com/'));assert.ok(isVerification(doc('<title>Just a moment...</title>')));assert.throws(()=>extract(doc('<title>Just a moment...</title>'),'https://www.beatport.com/chart/x'));
});
test('Challenge handling recognizes provider verification text without relying on title or iframe',()=>{
 const page=doc('<main><h1>Security check</h1><p>Please verify you are human to continue.</p></main>');
 assert.ok(isVerification(page));
 assert.throws(()=>extract(page,'https://open.spotify.com/playlist/PL'),/Complete provider verification/);
});
test('Challenge detection ignores ordinary track titles containing challenge-like wording',()=>{
 const page=doc('<h1>My playlist</h1><div data-testid="tracklist-row" aria-rowindex="1"><a href="/track/ONE">Just a Moment</a><a href="/artist/A">Artist</a></div>');
 assert.equal(isVerification(page),false);
 assert.equal(extract(page,'https://open.spotify.com/playlist/PL').entries[0].title,'Just a Moment');
});
test('Spotify does not invent track position for aria row 1 when a header exists',()=>{
 const page=doc('<div role="grid"><div role="row"><div role="columnheader">Title</div></div><div data-testid="tracklist-row" role="row" aria-rowindex="1"><div role="gridcell">Song</div><a href="/track/ONE">Song</a><a href="/artist/A">Artist</a></div></div>');
 assert.equal(extract(page,'https://open.spotify.com/playlist/PL').entries.length,0);
});

const beatportUrl='https://www.beatport.com/genre/afro-house/89/top-100';
test('Beatport compact live layout reads explicit rank controls and nested mix names',()=>{
 const html=readFileSync(new URL('./fixtures/beatport-afro-house-list.html',import.meta.url),'utf8');
 // The old diagnostic sanitizer omitted buttons, including the rank control.
 assert.equal(extract(doc(html),beatportUrl).entries.length,0);
 const page=doc(html);page.querySelector('[data-testid="tracks-list-item"]').insertAdjacentHTML('afterbegin','<button>1</button><button>Play</button>');
 const s=extract(page,beatportUrl);
 assert.equal(s.entries.length,1);assert.equal(s.entries[0].rank,1);
 assert.equal(s.entries[0].title,'Jamaican (Bam Bam)');assert.equal(s.entries[0].version,'Extended Mix');
 assert.equal(s.entries[0].artist,'HUGEL, SOLTO (FR)');assert.equal(s.entries[0].label,'MoBlack Records');
 page.querySelector('button').outerHTML='<button aria-label="1"></button>';
 assert.equal(extract(page,beatportUrl).entries[0].rank,1);
});
test('Compact chart diagnostic retains numeric rank buttons without action attributes',async()=>{
 const {diagnosticSample}=await import('../parsers.js');
 const page=doc(readFileSync(new URL('./fixtures/beatport-afro-house-list.html',import.meta.url),'utf8'));
 page.querySelector('[data-testid="tracks-list-item"]').insertAdjacentHTML('afterbegin','<button onclick="secret()" data-token="secret">1</button><button>Buy now</button>');
 const d=diagnosticSample(page,beatportUrl);
 assert.match(d.sampleHtml,/<button>1<\/button>/);assert.ok(!d.sampleHtml.includes('secret'));assert.ok(!d.sampleHtml.includes('Buy now'));
 assert.equal(extract(doc(d.sampleHtml),beatportUrl).entries[0].rank,1);
});
test('Spotify live presentation row parses once and uses playlist heading over sidebar',()=>{
 const row=readFileSync(new URL('./fixtures/spotify-live-row.html',import.meta.url),'utf8');
 const s=extract(doc(`<aside><h1>Your Library</h1></aside><main><h1>Beatport Top 100 - Afro House</h1><p>96 songs</p><div role="row" aria-rowindex="2">${row}</div></main>`),'https://open.spotify.com/playlist/PL');
 assert.equal(s.title,'Beatport Top 100 - Afro House');assert.equal(s.entries.length,1);
 assert.equal(s.entries[0].position,1);assert.equal(s.entries[0].durationSeconds,156);
});
test('Traxsource chart total can come from explicit provider document title',()=>{
 const s=extract(doc('<title>Afro House - TOP 100: Get Afro House Tracks on Traxsource</title><h1>AFRO HOUSE</h1>'),'https://www.traxsource.com/genre/27/afro-house/top');
 assert.equal(s.title,'AFRO HOUSE');assert.equal(s.expectedTotal,100);
 const unknown=extract(doc('<title>Afro House on Traxsource</title><h1>AFRO HOUSE</h1>'),'https://www.traxsource.com/genre/27/afro-house/top');
 assert.equal(unknown.expectedTotal,null);
});
test('Beatport rendered table reads linked artists and first-cell rank, not title-link test ID',()=>{
 const s=extract(doc(`<h1>Afro House</h1><div data-testid="tracks-table-row"><div class="cell"># 7</div><div class="cell title"><a data-testid="track-title" href="/track/recording/123"><span>Recording</span></a><a href="/artist/artist/5">Artist</a><a href="/artist/artist/5">Artist</a><span class="mix-name">Extended Mix</span></div><div class="cell label"><a href="/label/label/6">Label</a></div><div class="cell bpm">122 BPM</div><div class="cell key">A Minor</div><div class="cell length">6:32</div></div>`),beatportUrl);
 assert.equal(s.entries.length,1);assert.equal(s.entries[0].rank,7);assert.equal(s.entries[0].artist,'Artist');assert.equal(s.entries[0].title,'Recording');assert.equal(s.entries[0].version,'Extended Mix');assert.equal(s.entries[0].bpm,122);assert.equal(s.entries[0].durationSeconds,392);assert.equal(s.expectedTotal,100);
});
test('Beatport unranked recommendations never use a later BPM cell as chart rank',()=>{
 const s=extract(doc(`<h1>Afro House</h1><div data-testid="tracks-table-row"><div class="cell"><button>Play</button></div><div class="cell title"><a href="/track/recording/123">Recording</a><a href="/artist/artist/5">Artist</a></div><div class="cell bpm">122</div></div>`),beatportUrl);
 assert.equal(s.entries.length,0);
});
test('Beatport DOM membership excludes ranked JSON recommendations and enriches exact identity',()=>{
 const json={tracks:[{position:99,track:{id:123,name:'Canonical Title',artists:[{name:'Canonical Artist'}],mix_name:'Extended Mix',bpm:124}},{position:1,track:{id:999,name:'Other Chart',artists:[{name:'Other'}]}}]};
 const s=extract(doc(`<h1>Afro House</h1><script id="__NEXT_DATA__">${JSON.stringify(json)}</script><div data-testid="tracks-table-row"><span class="cell position">4</span><div class="cell title"><a href="/track/recording/123">Canonical Title Extended Mix</a><a href="/artist/artist/5">Artist</a></div></div>`),beatportUrl);
 assert.equal(s.entries.length,1);assert.equal(s.entries[0].rank,4);assert.equal(s.entries[0].title,'Canonical Title');assert.equal(s.entries[0].bpm,124);
});
test('Beatport local styled rows retain explicit rank without absorbing multiple recordings',()=>{
 const s=extract(doc(`<h1>Afro House</h1><section><div class="styled-row"><span class="rank">2</span><div><a href="/track/recording/123">Recording</a><a href="/artist/artist/5">Artist</a></div></div><div class="styled-row"><span class="rank">3</span><div><a href="/track/other/124">Other</a><a href="/artist/other/6">Other artist</a></div></div></section>`),beatportUrl);
 assert.deepEqual(s.entries.map(e=>e.rank),[2,3]);
});

test('Shazam charts use explicit ranked Next data and ignore unranked recommendations',()=>{
 const page=doc(readFileSync(new URL('./fixtures/shazam-chart.html',import.meta.url),'utf8'));
 const s=extract(page,'https://www.shazam.com/charts/top-200/world');
 assert.equal(s.provider,'shazam');assert.equal(s.kind,'chart');assert.equal(s.expectedTotal,200);
 assert.deepEqual(s.entries.map(e=>[e.providerId,e.rank,e.title,e.artist]),[['12345',1,'Loser','Tame Impala']]);
 assert.throws(()=>extract(doc('<h1>Song page</h1>'),'https://www.shazam.com/song/12345/loser'),/chart page/);
});

test('Last.fm chart keeps explicit positions and reports a partial page against the stated total',()=>{
 const page=doc(readFileSync(new URL('./fixtures/lastfm-chart.html',import.meta.url),'utf8'),'https://www.last.fm/charts');
 const s=extract(page,'https://www.last.fm/charts');
 assert.equal(s.kind,'chart');assert.equal(s.expectedTotal,200);assert.deepEqual(s.entries.map(e=>e.position),[1,2]);assert.equal(mergeSnapshots([s]).complete,false);
});

test('Last.fm user library imports only exact track rows and no inferred completeness',()=>{
 const page=doc('<main><h1>Saved tracks</h1><p>1–1 of 36 tracks</p><table><tr><td><a href="/music/Artist/_/Song">Song</a></td><td><a href="/music/Artist">Artist</a></td></tr></table></main>','https://www.last.fm/user/example/library/tracks');
 const s=extract(page,'https://www.last.fm/user/example/library/tracks');
 assert.equal(s.kind,'playlist');assert.equal(s.entries.length,1);assert.equal(s.entries[0].providerId,null);assert.equal(s.expectedTotal,36);assert.equal(mergeSnapshots([s]).complete,false);
});

test('Last.fm tag track charts capture ordered rows as charts, not genre-tagged metadata',()=>{
 const page=doc('<main><h1>Top Tracks tagged Afro House</h1><section><table><tr class="chartlist-row"><td class="chartlist-rank">1</td><td class="chartlist-name"><a href="/music/Artist/_/Song">Song</a></td><td class="chartlist-artist"><a href="/music/Artist">Artist</a></td></tr></table></section></main>','https://www.last.fm/tag/afro+house/tracks');
 const s=extract(page,'https://www.last.fm/tag/afro+house/tracks');
 assert.equal(s.kind,'chart');assert.equal(s.entries.length,1);assert.equal(s.entries[0].rank,1);assert.equal(s.entries[0].genre,null);assert.equal(s.sourceUrl,'https://www.last.fm/tag/afro+house/tracks');
});

test('Bandcamp album imports explicit TralbumData tracks; collections never convert album cards into songs',()=>{
 const page=doc(readFileSync(new URL('./fixtures/bandcamp-album.html',import.meta.url),'utf8'));
 const s=extract(page,'https://jpegmafia.bandcamp.com/album/lp');
 assert.equal(s.kind,'album');assert.equal(s.title,'LP!');assert.deepEqual(s.entries.map(e=>e.providerId),['987654','987655']);assert.deepEqual(s.entries.map(e=>e.position),[1,2]);assert.equal(s.entries[0].durationSeconds,91.5);
 const songRows=extract(doc('<h1>Mix</h1><div class="track_row_view"><a href="/track/song">Song</a><a href="/artist">Artist</a></div>'),'https://bandcamp.com/collection/example');
 assert.equal(songRows.kind,'playlist');assert.equal(songRows.entries[0].providerId,null);assert.match(songRows.entries[0].sourceUrl,/\/track\/song$/);
 assert.throws(()=>extract(doc('<h1>Collection</h1><a href="/album/lp">LP!</a>'),'https://bandcamp.com/collection/example'),/collections list albums, not song tracks/);
});

test('Bandcamp Discover captures only rendered artist album URLs, not track/card text',()=>{
 const page=doc(`<main><div class="result"><a href="https://daupe.bandcamp.com/album/beemer-on-broadway?from=discover_page">Beemer on Broadway</a><a href="/track/fake-song">Song card</a></div><a href="https://bandcamp.com/album/not-an-artist-album">Bad root</a><a href="https://evil.invalid/album/no">Bad host</a></main>`, 'https://bandcamp.com/discover/hip-hop-rap');
 const result=extract(page,'https://bandcamp.com/discover/hip-hop-rap');
 assert.deepEqual(result.albumCollection.albumUrls,['https://daupe.bandcamp.com/album/beemer-on-broadway']);
 assert.equal(result.albumCollection.sourceUrl,'https://bandcamp.com/discover/hip-hop-rap');
 assert.equal(result.snapshot,undefined);
});

test('Provider matching is HTTPS and confined to explicit host families',()=>{
 assert.equal(providerFor('https://shazam.com/charts/top-200/world'),'shazam');assert.equal(providerFor('https://last.fm/charts'),'lastfm');
 assert.equal(providerFor('https://artist.bandcamp.com/album/x'),'bandcamp');assert.equal(providerFor('https://bandcamp.com.evil.invalid/'),null);
});

test('Opt-in diagnostic captures one public row and excludes secrets, scripts and form data',async()=>{
 const {diagnosticSample}=await import('../parsers.js');
 const page=doc(`<main><div data-testid="unfamiliar-row" data-token="SECRET_ATTRIBUTE"><span class="new-rank">1</span><a href="/track/song/123?token=SECRET_QUERY">Public title</a><a href="/artist/name/5">Public artist</a><input value="SECRET_FORM"><form>SECRET_FORM_TEXT</form><script>SECRET_SCRIPT</script><a href="https://evil.invalid/?SECRET_OTHER">Public action</a></div><div><a href="/track/other/456">Other title</a><a href="/artist/other/6">Other artist</a></div></main>`);
 const result=diagnosticSample(page,beatportUrl+'?token=SECRET_SOURCE');const serialized=JSON.stringify(result);
 assert.equal(result.sourcePath,'/genre/afro-house/89/top-100');assert.ok(result.sampleHtml.includes('new-rank'));assert.ok(result.sampleHtml.includes('Public title'));assert.ok(!result.sampleHtml.includes('Other title'));assert.ok(!serialized.includes('SECRET'));assert.ok(!serialized.includes('evil.invalid'));assert.ok(result.ancestors.some(a=>a.trackCount===2));
});

const liveBeatportRow=readFileSync(new URL('./fixtures/beatport-afro-house-row.html',import.meta.url),'utf8');
test('Beatport real sanitized Afro House row preserves rank, clean title, mix and combined BPM/key',()=>{
 const s=extract(doc(liveBeatportRow),beatportUrl);
 assert.equal(s.entries.length,1);assert.equal(s.entries[0].providerId,'22357262');assert.equal(s.entries[0].rank,1);assert.equal(s.entries[0].position,1);assert.equal(s.entries[0].title,'Jamaican (Bam Bam)');assert.equal(s.entries[0].version,'Extended Mix');assert.equal(s.entries[0].artist,'HUGEL, SOLTO (FR)');assert.equal(s.entries[0].label,'MoBlack Records');assert.equal(s.entries[0].bpm,122);assert.equal(s.entries[0].key,'A Major');assert.equal(s.entries[0].releaseDate,'2025-11-07');assert.equal(s.entries[0].durationSeconds,null);assert.equal(mergeSnapshots([s]).complete,false);
});
test('Beatport real row retains independent ranks when several recordings are rendered',()=>{
 const second=liveBeatportRow.replaceAll('22357262','22357263').replace('>1</div>','>2</div>');
 const s=extract(doc(liveBeatportRow+second),beatportUrl);assert.deepEqual(s.entries.map(e=>e.rank),[1,2]);assert.deepEqual(s.entries.map(e=>e.providerId),['22357262','22357263']);
});
