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
test('Beatport JSON reads nested artist credits and title fallback without inventing names',()=>{
 const data={position:1,track:{id:17,name:'',title:'Nested title',artists:[{artist:{name:'Nested artist'},role:{name:'Artist'}}]}};
 const result=extract(doc(`<h1>Afro House</h1><script id="__NEXT_DATA__">${JSON.stringify(data)}</script>`),'https://www.beatport.com/genre/house/5/top-100');
 assert.equal(result.entries.length,1);assert.equal(result.entries[0].title,'Nested title');assert.equal(result.entries[0].artist,'Nested artist');
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
test('Spotify album rows inherit only explicit album artists when track credits are absent',()=>{
 const album=doc('<main><section data-testid="entityTitle"><h1>Night Drive</h1><a href="/artist/ALBUMARTIST">The Album Artist</a></section><div data-testid="tracklist-row" aria-rowindex="1"><a href="/track/ONE">First</a></div></main>');
 const result=extract(album,'https://open.spotify.com/album/ALBUM');
 assert.equal(result.entries.length,1);assert.equal(result.entries[0].artist,'The Album Artist');
 const playlist=doc('<main><h1>Night Drive</h1><div data-testid="tracklist-row" aria-rowindex="1"><a href="/track/ONE">First</a></div></main>');
 assert.equal(extract(playlist,'https://open.spotify.com/playlist/PLAYLIST').entries.length,0);
});
test('Spotify compilation heading is not treated as a missing track performer',()=>{
 const album=doc('<main><section data-testid="entityTitle"><h1>Compilation</h1><a href="/artist/VARIOUS">Various Artists</a></section><div data-testid="tracklist-row" aria-rowindex="1"><a href="/track/ONE">First</a></div></main>');
 const result=extract(album,'https://open.spotify.com/album/COMPILATION');
 assert.equal(result.title,'Compilation');
 assert.equal(result.entries.length,0);
});
test('Spotify album rows may inherit the explicit multi-artist album credit list',()=>{
 const album=doc('<main><section data-testid="entityTitle"><h1>Shared Record</h1><a href="/artist/FIRST">First Artist</a><a href="/artist/SECOND">Second Artist</a></section><div data-testid="tracklist-row" aria-rowindex="1"><a href="/track/ONE">First</a></div></main>');
 const result=extract(album,'https://open.spotify.com/album/SHARED');
 assert.equal(result.entries.length,1);
 assert.equal(result.entries[0].artist,'First Artist, Second Artist');
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

test('YouTube public video import creates one complete occurrence without a signed-in account',()=>{
 const page=doc('<main><ytd-watch-metadata><h1><yt-formatted-string>Song Title</yt-formatted-string></h1></ytd-watch-metadata><div id="owner"><ytd-channel-name><a href="/@Artist">Artist Channel</a></ytd-channel-name></div><meta itemprop="duration" content="PT3M12S"><meta property="og:image" content="https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg">','https://www.youtube.com/watch?v=abcdefghijk');
 const result=mergeSnapshots([extract(page,'https://www.youtube.com/watch?v=abcdefghijk')]);
 assert.equal(providerFor('https://www.youtube.com/watch?v=abcdefghijk'),'youtube');
 assert.equal(result.provider,'youtube');assert.equal(result.kind,'playlist');assert.equal(result.complete,true);
 assert.equal(result.expectedTotal,1);assert.equal(result.entries[0].providerId,'abcdefghijk');
 assert.equal(result.entries[0].artist,'Artist Channel');assert.equal(result.entries[0].title,'Song Title');
 assert.equal(result.entries[0].durationSeconds,192);assert.equal(result.entries[0].artworkUrl,'https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg');
});

test('YouTube watch extraction skips empty placeholder headings before the rendered title',()=>{
 const url='https://www.youtube.com/watch?v=Hqp8vxkPrI8';
 const page=doc('<ytd-watch-metadata><h1></h1><h1><yt-formatted-string></yt-formatted-string></h1><div id="title"><h1 class="title">BAD TIMES</h1></div><div id="owner"><ytd-channel-name><a href="/@ImaelAngel">Imael Angel</a></ytd-channel-name></div><meta itemprop="duration" content="PT2M48S"><meta property="og:image" content="https://i.ytimg.com/vi/Hqp8vxkPrI8/maxresdefault.jpg">',url);
 const result=mergeSnapshots([extract(page,url)]);
 assert.equal(result.entries[0].providerId,'Hqp8vxkPrI8');assert.equal(result.entries[0].title,'BAD TIMES');
 assert.equal(result.entries[0].artist,'Imael Angel');assert.equal(result.entries[0].durationSeconds,168);
});

test('YouTube public playlist preserves ordered duplicate occurrences and honest partial capture',()=>{
 const page=doc(`<main><ytd-playlist-header-renderer><h1>Public Mix</h1><span>3 videos</span></ytd-playlist-header-renderer><ytd-playlist-video-list-renderer>
  <ytd-playlist-video-renderer><span id="index">1</span><a id="video-title" href="/watch?v=abcdefghijk">First Song</a><div id="byline-container">First Artist</div><span class="duration">3:12</span></ytd-playlist-video-renderer>
  <ytd-playlist-video-renderer><span id="index">2</span><a id="video-title" href="/watch?v=BCDEFGHIJKL">Second Song</a><div id="byline-container">Second Artist</div></ytd-playlist-video-renderer>
  <ytd-playlist-video-renderer><span id="index">3</span><a id="video-title" href="/watch?v=abcdefghijk">First Song</a><div id="byline-container">First Artist</div></ytd-playlist-video-renderer>
 </ytd-playlist-video-list-renderer></main>`,'https://www.youtube.com/playlist?list=PLabcdefghijk');
 const result=mergeSnapshots([extract(page,'https://www.youtube.com/playlist?list=PLabcdefghijk')]);
 assert.equal(result.complete,true);assert.equal(result.expectedTotal,3);
 assert.deepEqual(result.entries.map(e=>[e.position,e.providerId]),[[1,'abcdefghijk'],[2,'BCDEFGHIJKL'],[3,'abcdefghijk']]);
 assert.deepEqual(result.entries.map(e=>e.artist),['First Artist','Second Artist','First Artist']);
 assert.equal(result.entries[0].durationSeconds,192);
 const partial=mergeSnapshots([extract(page,'https://www.youtube.com/playlist?list=PLabcdefghijk')]).entries.slice(0,2);
 assert.equal(mergeSnapshots([{...extract(page,'https://www.youtube.com/playlist?list=PLabcdefghijk'),entries:partial}]).complete,false);
});

test('YouTube current lockup playlist cards use explicit list indexes and ignore unrelated cards',()=>{
 const url='https://www.youtube.com/playlist?list=PLabcdefghijk';
 const card=(id,index,title,artist)=>`<yt-lockup-view-model><a href="/watch?v=${id}&list=PLabcdefghijk&index=${index}">3:55</a><a class="ytLockupMetadataViewModelTitle" aria-label="${title} 3 minutes, 55 seconds" href="/watch?v=${id}&list=PLabcdefghijk&index=${index}">${title}</a><a href="/channel/UC${artist}">${artist}</a><div class="ytBadgeShapeText">3:55</div></yt-lockup-view-model>`;
 const page=doc(`<main><ytd-playlist-header-renderer><h1>Public Mix</h1></ytd-playlist-header-renderer><yt-page-header-view-model><div>2 videos </div></yt-page-header-view-model>${card('abcdefghijk',1,'First Song','ArtistA')}${card('abcdefghijk',2,'First Song','ArtistA')}<yt-lockup-view-model><a class="ytLockupMetadataViewModelTitle" href="/watch?v=LMNOPQRSTUV&list=OTHERLIST123&index=1">Unrelated</a></yt-lockup-view-model></main>`,url);
 const result=mergeSnapshots([extract(page,url)]);
 assert.equal(result.expectedTotal,2);assert.equal(result.complete,true);
 assert.deepEqual(result.entries.map(e=>[e.position,e.providerId,e.title,e.artist,e.durationSeconds]),[[1,'abcdefghijk','First Song','ArtistA',235],[2,'abcdefghijk','First Song','ArtistA',235]]);
});

test('YouTube rejects unsupported pages and does not infer missing public channel credit',()=>{
 assert.throws(()=>extract(doc('<h1>Channel</h1>'),'https://www.youtube.com/@Artist'),/public YouTube video or playlist/);
 assert.throws(()=>extract(doc('<h1>Song</h1>'),'https://www.youtube.com/watch?v=bad'),/public YouTube video or playlist/);
 assert.throws(()=>extract(doc('<h1>Song</h1>'),'https://www.youtube.com/watch?v=abcdefghijk'),/channel credit/);
});

test('A visibly empty YouTube playlist with explicit zero count is complete',()=>{
 const page=doc('<main><ytd-playlist-header-renderer><h1>Empty mix</h1><span>0 videos</span></ytd-playlist-header-renderer><p>This playlist is empty.</p></main>','https://www.youtube.com/playlist?list=PLabcdefghijk');
 const result=mergeSnapshots([extract(page,'https://www.youtube.com/playlist?list=PLabcdefghijk')]);
 assert.equal(result.complete,true);assert.equal(result.entries.length,0);assert.equal(result.expectedTotal,0);
});

test('YouTube playlist row omissions prevent completeness even when declared count matches captured rows',()=>{
 const page=doc(`<main><ytd-playlist-header-renderer><h1>Changed playlist</h1><span>2 videos</span></ytd-playlist-header-renderer><ytd-playlist-video-list-renderer>
  <ytd-playlist-video-renderer><span id="index">1</span><a id="video-title" href="/watch?v=abcdefghijk">First</a><div id="byline-container">Artist</div></ytd-playlist-video-renderer>
  <ytd-playlist-video-renderer><span id="index">2</span><a id="video-title" href="/watch?v=BCDEFGHIJKL">Missing credit</a></ytd-playlist-video-renderer>
  <ytd-playlist-video-renderer><span id="index">2</span><a id="video-title" href="/watch?v=LMNOPQRSTUV">Second</a><div id="byline-container">Artist</div></ytd-playlist-video-renderer>
 </ytd-playlist-video-list-renderer></main>`,'https://www.youtube.com/playlist?list=PLabcdefghijk');
 const result=mergeSnapshots([extract(page,'https://www.youtube.com/playlist?list=PLabcdefghijk')]);
 assert.equal(result.entries.length,2);assert.equal(result.expectedTotal,2);assert.equal(result.complete,false);
 assert.ok(result.warnings.some(warning=>/rows lacked/.test(warning)));
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
test('Shazam JSON falls through empty primary fields to explicit alternate credits',()=>{
 const page=doc(`<main><h1>Top 200</h1><script type="application/json">${JSON.stringify({chart:{tracks:[{rank:'',position:2,track:{key:'',id:'12345',title:'',name:'Explicit title',subtitle:'',artists:[{artist:{name:'Nested artist'}}]}}]}})}</script></main>`);
 const result=extract(page,'https://www.shazam.com/charts/top-200/world');
 assert.deepEqual(result.entries.map(({providerId,title,artist,rank})=>({providerId,title,artist,rank})),[{providerId:'12345',title:'Explicit title',artist:'Nested artist',rank:2}]);
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

 test('Shazam import preserves a validated printed ISRC without inventing identifiers',()=>{
  const make=isrc=>doc(`<h1>Top 200</h1><script type="application/json">${JSON.stringify({tracks:[{rank:1,key:'12345',title:'Nighttime in the Backyard',subtitle:'Lucas Santtana & Arto Lindsay',isrc}]})}</script>`);
  assert.equal(extract(make('BR-L6M-09-00003'),'https://www.shazam.com/charts/top-200/world').entries[0].isrc,'BRL6M0900003');
  assert.equal(extract(make('bad-code'),'https://www.shazam.com/charts/top-200/world').entries[0].isrc,null);
 });
