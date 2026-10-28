// Only explicitly selected public catalog fields leave the browser.
export const sites = ['https://www.beatport.com/*', 'https://www.traxsource.com/*', 'https://open.spotify.com/*', 'https://www.youtube.com/*', 'https://music.youtube.com/*', 'https://www.shazam.com/*', 'https://www.last.fm/*', 'https://last.fm/*', 'https://bandcamp.com/*', 'https://*.bandcamp.com/*'];
export function providerFor(url) {
  const u = new URL(url);
  if (u.protocol !== 'https:') throw new Error('Use an HTTPS provider page.');
  const host=u.hostname.toLowerCase();
  if(host==='www.shazam.com'||host==='shazam.com')return 'shazam';
  if(host==='www.last.fm'||host==='last.fm')return 'lastfm';
  if(host==='bandcamp.com'||host.endsWith('.bandcamp.com'))return 'bandcamp';
  if(host==='www.youtube.com'||host==='music.youtube.com')return 'youtube';
  return ({'www.beatport.com':'beatport','beatport.com':'beatport','www.traxsource.com':'traxsource','traxsource.com':'traxsource','open.spotify.com':'spotify'})[host] || null;
}
const text = (root, selector) => root.querySelector(selector)?.textContent.trim() || null;
const num = value => { const n = Number(value); return Number.isFinite(n) && n > 0 ? n : null; };
const rankNumber = value => {const n=num(value);return Number.isInteger(n)?n:null;};
const time = value => {const m = value?.match(/\b(\d+):(\d{2})\b/); return m ? Number(m[1])*60+Number(m[2]) : null;};
const link = (value, base) => { if(typeof value!=='string'||!value.trim())return null; try { const u = new URL(value,base); return u.protocol === 'https:' ? u.href : null; } catch { return null; } };
function canonicalSpotifyUrl(value) {
  const u = new URL(value);
  if (u.hostname === 'open.spotify.com') {
    u.pathname = u.pathname.replace(/^\/intl-[a-z]{2}(?:-[a-z]{2})?(?=\/)/i, '');
    u.search = '';
    u.hash = '';
  }
  return u.href;
}
function entry(providerId, sourceUrl, artist, title, position, extra={}) {
  return {providerId, sourceUrl, artist, title, position, rank:null, version:null,durationSeconds:null,bpm:null,key:null,album:null,label:null,genre:null,releaseDate:null,artworkUrl:null,isrc:null,catalogNumber:null,metadata:null,...extra};
}
function pageLink(value,base){const raw=link(value,base);if(!raw)return null;const u=new URL(raw);if(u.username||u.password)return null;u.search='';u.hash='';return u.href;}
function rowMetadata(row,base){
 const links=selector=>[...row.querySelectorAll(selector)].map(a=>pageLink(a.getAttribute('href'),base)).filter(Boolean);
 return {artistUrls:[...new Set(links('a[href*="/artist/"]'))],albumUrl:links('a[href*="/album/"],a[href*="/release/"]')[0]||null,labelUrl:links('a[href*="/label/"]')[0]||null,remixers:[...row.querySelectorAll('.remixers a,.remixer a')].map(a=>a.textContent.trim()).filter(Boolean)};
}
function bandcampArtwork(data,doc,base){
 const raw=data.art_url||(typeof data.art==='string'&&/^https:/.test(data.art)?data.art:null)||doc.querySelector('meta[property="og:image"]')?.getAttribute('content');
 if(raw)return pageLink(raw,base);
 const id=String(data.art_id??data.art??'');return /^\d{1,20}$/.test(id)?`https://f4.bcbits.com/img/a${id}_10.jpg`:null;
}
function bandcampMetadata(data,doc,base){
 const bounded=(value,max=12000)=>typeof value==='string'?value.trim().slice(0,max)||null:null;
 const current=data.current||{};
 return {albumUrl:pageLink(base,base),artistUrl:new URL(base).origin+'/',albumArtist:bounded(data.artist,2000),description:bounded(current.about||text(doc,'.tralbumData.tralbum-about,.tralbum-about')),credits:bounded(current.credits||text(doc,'.tralbum-credits')),lyrics:bounded(current.lyrics),location:bounded(text(doc,'.location'),2000),artistBio:bounded(text(doc,'#bio-text'),12000),tags:[...doc.querySelectorAll('a.tag')].slice(0,100).map(a=>a.textContent.trim().slice(0,500)).filter(Boolean)};
}
function safeTitle(doc){return text(doc,'main h1,[role="main"] h1,h1')||doc.title||'Imported tracks';}
function boundedTotal(value){const n=Number(String(value??'').replaceAll(',',''));return Number.isInteger(n)&&n>0&&n<=10000?n:null;}
function rankedPosition(row){
  for(const node of row.querySelectorAll('[data-rank], [data-position], [aria-label*="rank" i], .rank, .chartlist-rank, .chartlist-index, .position, .chart-rank')) {
    const value=node.getAttribute('data-rank')||node.getAttribute('data-position')||node.getAttribute('aria-label')||node.textContent;
    const rank=rankNumber(String(value||'').trim().replace(/^#\s*/,''));if(rank&&rank<=10000)return rank;
  }
  // Last.fm chart rows use a dedicated first rank cell without rank attributes.
  // Read only that structural cell; never infer a rank from DOM order.
  const first=row.querySelector('td,[role="cell"],[role="gridcell"]');
  if(first&&!first.querySelector('a')){const rank=rankNumber(first.textContent.trim().replace(/^#\s*/,''));if(rank&&rank<=10000)return rank;}
  return null;
}
function chartTotal(doc,title,url,limit=1000){
  const textValue=`${title} ${(doc.querySelector('main')||doc.body).textContent||''}`;
  const matches=[...textValue.matchAll(/\b(?:top|top\s+tracks|top\s+songs)\s*(?:-|#)?\s*(\d{1,4})\b/gi)];
  const path=new URL(url).pathname.match(/\btop[-/](\d{1,4})\b/i);
  const value=path?.[1]||matches.map(match=>match[1]).find(n=>Number(n)<=limit);
  const total=boundedTotal(value);return total&&total<=limit?total:null;
}
function parseJsonScripts(doc, selectors='script#__NEXT_DATA__,script[type="application/json"],script[data-tralbum]'){
  const values=[];
  for(const script of doc.querySelectorAll(selectors)){
    const raw=script.getAttribute('data-tralbum')||script.textContent||'';
    if(raw.length>2_000_000)continue;
    try{values.push(JSON.parse(raw));continue;}catch{/* may be an assignment rather than bare JSON */}
    const match=raw.match(/(?:window\.)?TralbumData\s*=\s*(\{[\s\S]*?\})\s*;?\s*$/);
    if(match)try{values.push(JSON.parse(match[1]));}catch{/* unrelated script */}
  }
  return values;
}
function normalizedIsrc(value){
  if(typeof value!=='string')return null;
  const trimmed=value.trim().toUpperCase();
  const code=/^[A-Z]{2}-[A-Z0-9]{3}-[0-9]{2}-[0-9]{5}$/.test(trimmed)?trimmed.replaceAll('-',''):trimmed;
  return /^[A-Z]{2}[A-Z0-9]{3}[0-9]{7}$/.test(code)?code:null;
}
function shazamTrack(value){
  const track=value?.track&&typeof value.track==='object'?value.track:value;
  const id=[track?.key,track?.id,track?.shazamId].map(value=>String(value??'').trim()).find(value=>/^\d{1,20}$/.test(value))||'';
  const title=[track?.title,track?.name].find(value=>typeof value==='string'&&value.trim())?.trim();
  const artist=[track?.subtitle,track?.artist?.name,track?.artist?.['#text'],track?.artist?.artistName,
    ...(Array.isArray(track?.artists)?[track.artists.map(a=>typeof a==='string'?a:a?.name??a?.artist?.name).filter(v=>typeof v==='string'&&v.trim()).join(', ')]:[])]
    .find(value=>typeof value==='string'&&value.trim())?.trim();
  const rank=[value?.rank,value?.position,value?.chart_position,value?.chartRank,track?.rank,track?.position,track?.chart_position,track?.chartRank]
    .map(rankNumber).find(value=>value!==null);
  const href=track?.url??value?.url;
  if(!id||!rank||rank>10000||!title||!artist)return null;
  return {id,title,artist,rank,isrc:normalizedIsrc(track?.isrc),href:typeof href==='string'?href:null,artwork:track?.images?.coverart??track?.images?.coverArt??null};
}
function shazamJsonEntries(doc,url){
  const found=[],seen=new Set(),scripts=parseJsonScripts(doc);
  let visited=0;
  const walk=(value,depth=0)=>{
    if(!value||typeof value!=='object'||depth>24||visited++>50000)return;
    if(Array.isArray(value)){for(const item of value)walk(item,depth+1);return;}
    const item=shazamTrack(value);
    if(item){const key=`${item.rank}:${item.id}`;if(!seen.has(key)){seen.add(key);found.push(item);}}
    for(const child of Object.values(value))walk(child,depth+1);
  };
  for(const value of scripts)walk(value);
  return found.map(item=>entry(item.id,link(item.href,url)||`https://www.shazam.com/song/${item.id}`,item.artist,item.title,item.rank,{rank:item.rank,artworkUrl:link(item.artwork,url),isrc:item.isrc}));
}
function shazamDomEntries(doc,url){
  const found=[],seen=new Set();
  for(const anchor of doc.querySelectorAll('a[href*="/song/"]')){
    const href=link(anchor.getAttribute('href'),url),id=href?.match(/\/song\/(\d{1,20})(?:\/|$)/)?.[1];
    if(!id||!anchor.textContent.trim())continue;
    let row=null;
    for(let parent=anchor.parentElement,depth=0;parent&&depth<9;parent=parent.parentElement,depth++){
      const songs=[...parent.querySelectorAll('a[href*="/song/"]')];
      if(songs.length>1)break;
      if(songs.length===1&&rankedPosition(parent)&&parent.querySelector('a[href*="/artist/"]')){row=parent;break;}
    }
    if(!row)continue;
    const rank=rankedPosition(row),artist=[...row.querySelectorAll('a[href*="/artist/"]')].map(a=>a.textContent.trim()).filter(Boolean).join(', ');
    const key=`${rank}:${id}`;if(!rank||!artist||seen.has(key))continue;seen.add(key);
    found.push(entry(id,href,artist,anchor.textContent.trim(),rank,{rank,artworkUrl:link(row.querySelector('img')?.getAttribute('src'),url),metadata:rowMetadata(row,url)}));
  }
  return found;
}
function extractShazam(doc,url){
  const path=new URL(url).pathname;
  if(!/^\/charts\/(?:top-\d+|viral)(?:\/|$)/i.test(path))throw new Error('Open a Shazam chart page.');
  const title=safeTitle(doc), entries=shazamJsonEntries(doc,url);
  if(!entries.length)entries.push(...shazamDomEntries(doc,url));
  if(!entries.length)throw new Error('Shazam chart rows were not available. Load the chart in this browser tab and retry.');
  entries.sort((a,b)=>(a.rank||0)-(b.rank||0));
  const total=chartTotal(doc,title,url,1000);
  return {schemaVersion:1,provider:'shazam',kind:'chart',title,sourceUrl:url,capturedAt:new Date().toISOString(),complete:false,expectedTotal:total,warnings:[],entries};
}
function lastfmTrackHref(value,url){
  const href=link(value,url);if(!href)return null;
  const parsed=new URL(href);
  return /^\/(?:music\/[^/]+\/_\/[^/]+|user\/[^/]+\/library\/[^/]+)\/?$/i.test(parsed.pathname)?parsed.href:null;
}
function lastfmRows(doc,chart){
  const main=doc.querySelector('main,[role="main"]')||doc.body;
  let scope=main;
  if(chart){
    const heading=[...main.querySelectorAll('h1,h2,h3')].find(node=>/^top tracks(?:\b|$)/i.test(node.textContent.trim()));
    if(!heading)return [];
    for(let parent=heading.parentElement;parent&&parent!==main.parentElement;parent=parent.parentElement){
      if(parent.querySelector('a[href*="/music/"][href*="/_/"]')){scope=parent;break;}
      if(parent===main)break;
    }
  }
  const rows=[...scope.querySelectorAll('.chartlist-row,[data-testid="chartlist-row"],tr,[role="row"]')];
  const found=[],seen=new Set();
  for(const row of rows){
    const titleAnchor=[...row.querySelectorAll('a[href*="/music/"]')].find(a=>lastfmTrackHref(a.getAttribute('href'),doc.URL)&&a.textContent.trim());
    if(!titleAnchor)continue;
    const artistAnchor=[...row.querySelectorAll('a[href*="/music/"]')].find(a=>a!==titleAnchor&&!/\/_\//.test(new URL(a.href,doc.URL).pathname)&&a.textContent.trim());
    if(!artistAnchor)continue;
    const rank=rankedPosition(row);if(chart&&!rank)continue;
    const href=lastfmTrackHref(titleAnchor.getAttribute('href'),doc.URL);if(!href)continue;
    const position=rank||found.length+1,key=`${position}:${href}`;if(seen.has(key))continue;seen.add(key);
    found.push(entry(null,href,artistAnchor.textContent.trim(),titleAnchor.textContent.trim(),position,{rank:chart?rank:null,artworkUrl:link(row.querySelector('img[src]')?.getAttribute('src'),doc.URL),metadata:{artistUrl:pageLink(artistAnchor.getAttribute('href'),doc.URL),musicbrainzId:row.getAttribute('data-mbid')||null,...Object.fromEntries(['plays','listeners'].map(key=>[key,Number(row.getAttribute('data-'+key))||null]))}}));
  }
  return found;
}
function lastfmExpectedTotal(doc){
  const rangePattern=/\b\d[\d,]*\s*[-–—]\s*\d[\d,]*\s+of\s+([\d,]+)\b/i;
  const rangeTotal=boundedTotal((doc.querySelector('main')||doc.body).textContent.match(rangePattern)?.[1]);if(rangeTotal)return rangeTotal;
  for(const node of doc.querySelectorAll('p,small,[class*="pagination" i],[data-testid*="pagination" i]')){
    const raw=node.textContent||'';
    const total=boundedTotal(raw.match(/of\s+([\d,]+)\s+(?:tracks?|songs?)/i)?.[1]||raw.match(/([\d,]+)\s+(?:tracks?|songs?)/i)?.[1]);
    if(total)return total;
  }
  for(const node of doc.querySelectorAll('[data-total],[data-count],[aria-label*="of" i],.pagination,.pagination-pages')){
    const raw=node.getAttribute('data-total')||node.getAttribute('data-count')||node.getAttribute('aria-label')||node.textContent||'';
    const match=String(raw).match(/(?:\b\d[\d,]*\s*[-–—]\s*\d[\d,]*\s+of\s+|\bof\s+)([\d,]+)\s+(?:tracks?|songs?)?/i);
    const total=boundedTotal(match?.[1]);if(total)return total;
  }
  const textContent=(doc.querySelector('main')||doc.body).textContent||'';
  const inline=textContent.match(/of\s+([\d,]+)\s+(?:tracks?|songs?)\b/i)?.[1]||textContent.match(/([\d,]+)\s+(?:tracks?|songs?)\b/i)?.[1]||textContent.match(/(?:tracks?|songs?)\s*\(\s*([\d,]+)\s*\)/i)?.[1];
  return boundedTotal(inline);
}
function extractLastfm(doc,url){
  const path=new URL(url).pathname;
  const chart=/^\/charts\/?$/.test(path),tag=/^\/tag\/[^/]+\/tracks\/?$/i.test(path),library=/^\/user\/[^/]+\/library(?:\/tracks)?\/?$/i.test(path);
  if(!chart&&!tag&&!library)throw new Error('Open a Last.fm chart, tag track chart, or user track library page.');
  const title=safeTitle(doc),entries=lastfmRows(doc,chart||tag);
  if(!entries.length)throw new Error('Last.fm track rows were not available. Open the Top Tracks chart, tag track chart, or user track library page and retry.');
  if(chart||tag)entries.sort((a,b)=>(a.rank||0)-(b.rank||0));
  return {schemaVersion:1,provider:'lastfm',kind:chart||tag?'chart':'playlist',title,sourceUrl:url,capturedAt:new Date().toISOString(),complete:false,expectedTotal:lastfmExpectedTotal(doc),warnings:[],entries};
}
function bandcampData(doc){
  for(const value of parseJsonScripts(doc,'script[data-tralbum],#pagedata[data-tralbum],script[type="application/ld+json"],script:not([src])')){
    const data=value?.trackinfo?value:value?.['@graph']?.find?.(item=>item?.trackinfo)||value;
    if(Array.isArray(data?.trackinfo))return data;
  }
  return null;
}
function bandcampAlbum(doc,url){
  const data=bandcampData(doc);
  if(!data||!Array.isArray(data.trackinfo)||!data.trackinfo.length)throw new Error('Bandcamp did not expose an explicit album tracklist. Open the album page and retry.');
  const title=String(data.album_title||data.current?.title||safeTitle(doc)),artist=String(data.artist||'').trim();
  if(!artist)throw new Error('Bandcamp album artist metadata was unavailable.');
  const rows=[...doc.querySelectorAll('#track_table .track_row_view,.track_row_view,[data-testid="track-row"]')];
  const entries=[];
  data.trackinfo.slice(0,1000).forEach((track,index)=>{
    const name=String(track?.title||'').trim(),id=String(track?.track_id??'');
    if(!name||!/^\d{1,20}$/.test(id))return;
    const row=rows.find(item=>item.getAttribute('data-track-id')===id||[...item.querySelectorAll('[data-track-id]')].some(node=>node.getAttribute('data-track-id')===id))||rows[index];
    const anchor=row&&[...row.querySelectorAll('a[href*="/track/"]')].find(a=>a.textContent.trim()===name||!a.textContent.trim());
    const position=rankNumber(track.track_num)||index+1;
    const duration=num(track.duration);
    const trackUrl=link(track.title_link,url)||link(anchor?.getAttribute('href'),url)||url;
    entries.push(entry(id,trackUrl,artist,name,position,{rank:null,durationSeconds:duration,album:title,releaseDate:data.album_release_date||data.release_date||null,artworkUrl:bandcampArtwork(data,doc,url),metadata:{...bandcampMetadata(data,doc,url),trackNumber:position,lyrics:typeof track.lyrics==='string'?track.lyrics.slice(0,12000):null}}));
  });
  if(!entries.length)throw new Error('Bandcamp album metadata did not contain importable songs.');
  return {schemaVersion:1,provider:'bandcamp',kind:'album',title,sourceUrl:url,capturedAt:new Date().toISOString(),complete:false,expectedTotal:data.trackinfo.length,warnings:entries.length===data.trackinfo.length?[]:['Some album tracks lacked a stable provider ID or title and were omitted.'],entries};
}
function bandcampCollection(doc,url){
  const rows=[...doc.querySelectorAll('#track_table .track_row_view,.track_row_view,[data-testid="track-row"]')],entries=[];
  for(const row of rows){
    const anchor=[...row.querySelectorAll('a[href*="/track/"]')].find(a=>a.textContent.trim());
    if(!anchor)continue;
    const artistNode=row.querySelector('.artist,.track_artist,.artist_name,[data-testid="track-artist"]');
    const artist=artistNode?.textContent.trim()||[...row.querySelectorAll('a')].find(a=>a!==anchor)?.textContent.trim();
    if(!artist)continue;
    const href=link(anchor.getAttribute('href'),url);
    if(!href)continue;
    const rawId=row.getAttribute('data-track-id')||row.getAttribute('data-item-id')||row.querySelector('[data-track-id]')?.getAttribute('data-track-id');
    const id=/^\d{1,20}$/.test(String(rawId||''))?String(rawId):null;
    entries.push(entry(id,href,artist,anchor.textContent.trim(),entries.length+1,{album:text(doc,'h1')}));
  }
  if(!entries.length)throw new Error('Bandcamp collections list albums, not song tracks. Open an album tracklist to import its songs.');
  return {schemaVersion:1,provider:'bandcamp',kind:'playlist',title:safeTitle(doc),sourceUrl:url,capturedAt:new Date().toISOString(),complete:false,expectedTotal:null,warnings:['Only rendered song rows were captured; album cards were not treated as tracks.'],entries};
}
function bandcampDiscover(doc,url){
  const page=new URL(url);
  if(!/^\/discover\/[^/]+\/?$/i.test(page.pathname))throw new Error('Open a Bandcamp Discover tag page.');
  const albumUrls=[],seen=new Set();
  for(const anchor of doc.querySelectorAll('main a[href*="/album/"]')){
    const href=link(anchor.getAttribute('href'),url);if(!href)continue;
    const album=new URL(href);
    if(album.hostname==='bandcamp.com'||!album.hostname.endsWith('.bandcamp.com')||!/^\/album\/[^/]+\/?$/i.test(album.pathname)||album.username||album.password||album.port)continue;
    album.search='';album.hash='';
    if(seen.has(album.href))continue;
    seen.add(album.href);albumUrls.push(album.href);
  }
  if(!albumUrls.length)throw new Error('Bandcamp Discover did not show rendered album cards.');
  const warnings=albumUrls.length>20?['Only the first 20 rendered album cards can be imported at once.']:[];
  return {albumCollection:{sourceUrl:`${page.origin}${page.pathname}${['top','new','rec','date'].includes(page.searchParams.get('sort'))?'?sort='+page.searchParams.get('sort'):''}`,title:safeTitle(doc),albumUrls:albumUrls.slice(0,20),warnings}};
}
export function isVerification(doc) {
  const challengePattern = /just a moment|verify you are human|security verification|security check|checking your browser|complete the security check/i;
  const challengeNode=doc.querySelector('#challenge-running, #challenge-stage, [data-testid*="challenge"], [class*="challenge"], [id*="challenge"]');
  const challengeHeading=doc.querySelector('h1,h2,[role="heading"]');
  return challengePattern.test(doc.title || '') || !!doc.querySelector('#challenge-running, #challenge-stage, iframe[src*="challenges.cloudflare.com"]') ||
    !!challengeNode && challengePattern.test(challengeNode.textContent || '') ||
    !!challengeHeading && challengePattern.test(challengeHeading.textContent || '');
}
function youtubeText(node){return String(node?.textContent||node?.getAttribute?.('title')||'').trim().replace(/\s+/g,' ');}
function youtubeVideoId(value){return typeof value==='string'&&/^[A-Za-z0-9_-]{11}$/.test(value)?value:null;}
function youtubeDuration(value){
  if(typeof value!=='string')return null;
  const iso=value.match(/^PT(?:(\d+)H)?(?:(\d+)M)?(?:(\d+(?:\.\d+)?)S)?$/i);
  if(iso){const seconds=Number(iso[1]||0)*3600+Number(iso[2]||0)*60+Number(iso[3]||0);return Number.isFinite(seconds)&&seconds>0?seconds:null;}
  const parts=value.trim().split(':');
  if(parts.length<2||parts.length>3||parts.some(part=>!/^\d+$/.test(part)))return null;
  const seconds=parts.map(Number).reduce((total,part)=>total*60+part,0);
  return Number.isSafeInteger(seconds)&&seconds>0?seconds:null;
}
function youtubeExpectedTotal(root){
  const value=(root?.textContent||'').match(/([\d,]+)\s+(?:videos?|songs?|tracks?)\b/i)?.[1];
  if(!value)return null;
  const total=Number(value.replaceAll(',',''));
  return Number.isInteger(total)&&total>=0&&total<=10000?total:null;
}
function youtubePlaylistEntry(row,_index,base){
  const page=new URL(base),playlistId=page.searchParams.get('list');
  const watchAnchors=[...row.querySelectorAll('a[href*="/watch"],a[href*="youtube.com/watch"]')];
  // The current YouTube playlist page uses yt-lockup-view-model cards. Their
  // time badge is the first watch link; the title link carries an accessible
  // label and every link carries the explicit playlist index.
  const indexed=watchAnchors.filter(a=>{try{const u=new URL(a.getAttribute('href'),base);return u.searchParams.get('list')===playlistId&&u.searchParams.has('index');}catch{return false;}});
  const anchor=indexed.find(a=>a.hasAttribute('aria-label')&&!/^\s*\d{1,2}:\d{2}\s*$/.test(youtubeText(a)))||watchAnchors.find(a=>!/^\s*\d{1,2}:\d{2}\s*$/.test(youtubeText(a)))||watchAnchors[0];
  let videoId=youtubeVideoId(row.getAttribute('data-video-id')||row.dataset?.videoId||null);
  if(!videoId&&anchor){try{videoId=youtubeVideoId(new URL(anchor.getAttribute('href'),base).searchParams.get('v'));}catch{/* invalid rendered link */}}
  const titleNode=row.querySelector('#video-title, a#video-title, yt-formatted-string#video-title, [data-testid="video-title"], a.ytLockupMetadataViewModelTitle, [class*="ytLockupMetadataViewModelTitle"]')||anchor;
  const title=youtubeText(titleNode)||youtubeText(anchor);
  const artistNode=row.querySelector('#byline-container, #channel-name, ytd-channel-name, .byline, #subtitle, a[href*="/channel/"], a[href*="/@"]');
  const artist=youtubeText(artistNode);
  const ordinalNode=row.querySelector('#index, [id="index"], [data-index], [aria-label*="position" i]');
  const ordinalValue=ordinalNode?.getAttribute('data-index')||ordinalNode?.getAttribute('aria-label')||youtubeText(ordinalNode);
  const ordinalMatch=String(ordinalValue||'').match(/\d+/);
  let linkIndex=null;
  if(anchor){try{const u=new URL(anchor.getAttribute('href'),base);if(u.searchParams.get('list')===playlistId)linkIndex=Number(u.searchParams.get('index'));}catch{/* noncanonical link */}}
  const position=ordinalMatch?Number(ordinalMatch[0]):Number.isInteger(linkIndex)&&linkIndex>0?linkIndex:null;
  if(!Number.isInteger(position)||position<1||position>10000||!title||!artist)return null;
  const durationNode=row.querySelector('#text.ytd-thumbnail-overlay-time-status-renderer, .duration, .ytBadgeShapeText, yt-thumbnail-bottom-overlay-view-model, [aria-label*="duration" i]');
  const durationSeconds=youtubeDuration(durationNode?.getAttribute('aria-label')||youtubeText(durationNode));
  const image=row.querySelector('img');
  const artworkUrl=link(image?.getAttribute('src')||image?.getAttribute('srcset')?.split(',').at(-1)?.trim().split(/\s+/)[0],base);
  const url=videoId?`https://www.youtube.com/watch?v=${videoId}`:null;
  return entry(videoId,url,artist,title,position,{durationSeconds,artworkUrl,metadata:{videoTitle:title,channelName:artist,channelUrl:pageLink(row.querySelector('a[href*="/channel/"],a[href*="/@"]')?.getAttribute('href'),base),availability:'available'}});
}
function extractYouTube(doc,url){
  const page=new URL(url),videoId=youtubeVideoId(page.searchParams.get('v'));
  if(page.pathname==='/watch'&&videoId){
    // YouTube can render empty placeholder headings ahead of its populated
    // watch-metadata heading. A comma-selector querySelector returns the first
    // DOM match even when it has no text, so scan the bounded heading set.
    const title=[...doc.querySelectorAll('ytd-watch-metadata h1, #above-the-fold h1, h1.title, h1')].map(youtubeText).find(Boolean)||'';
    const owner=doc.querySelector('#owner #channel-name a, #upload-info #channel-name a, ytd-video-owner-renderer #channel-name a, ytd-channel-name a');
    const artist=youtubeText(owner)||youtubeText(doc.querySelector('#owner #channel-name, #upload-info #channel-name, ytd-video-owner-renderer #channel-name'));
    if(!title||!artist)throw new Error('YouTube video title or channel credit is not rendered. Wait for the public video metadata to load and retry.');
    const durationNode=doc.querySelector('meta[itemprop="duration"], .ytp-time-duration');
    const durationRaw=durationNode?.getAttribute('content')||youtubeText(durationNode);
    const image=doc.querySelector('meta[property="og:image"]')?.getAttribute('content')||doc.querySelector('#thumbnail img, ytd-video-primary-info-renderer img')?.getAttribute('src');
    return {schemaVersion:1,provider:'youtube',kind:'playlist',title:`${artist} — ${title}`,sourceUrl:`https://www.youtube.com/watch?v=${videoId}`,capturedAt:new Date().toISOString(),complete:true,expectedTotal:1,warnings:[],entries:[entry(videoId,`https://www.youtube.com/watch?v=${videoId}`,artist,title,1,{durationSeconds:youtubeDuration(durationRaw),artworkUrl:link(image,url),metadata:{videoTitle:title,channelName:artist,channelUrl:pageLink(doc.querySelector('#owner a[href*="/channel/"],#owner a[href*="/@"]')?.getAttribute('href'),url),availability:'available'}})]};
  }
  const playlistId=page.searchParams.get('list');
  if(page.pathname==='/playlist'&&typeof playlistId==='string'&&/^[A-Za-z0-9_-]{10,80}$/.test(playlistId)){
    const root=doc.querySelector('ytd-playlist-video-list-renderer')||doc.querySelector('ytmusic-playlist-shelf-renderer')||doc.querySelector('main')||doc.body;
    const rows=[...root.querySelectorAll('ytd-playlist-video-renderer, ytmusic-responsive-list-item-renderer, yt-lockup-view-model')]
      .filter(row=>row.tagName.toLowerCase()!=='yt-lockup-view-model'||[...row.querySelectorAll('a[href*="/watch"]')].some(anchor=>{
        try{const linked=new URL(anchor.getAttribute('href'),url);return linked.searchParams.get('list')===playlistId&&linked.searchParams.has('index');}catch{return false;}
      })).slice(0,10000);
    const entries=[],warnings=[];
    rows.forEach((row,index)=>{const parsed=youtubePlaylistEntry(row,index,url);if(parsed)entries.push(parsed);});
    if(entries.length<rows.length)warnings.push('Some rendered playlist rows lacked a stable order, title, or channel credit and were omitted.');
    const header=doc.querySelector('ytd-playlist-header-renderer')||doc.querySelector('ytmusic-playlist-header-renderer')||doc.querySelector('main')||doc.body;
    const expectedTotal=youtubeExpectedTotal(header)??youtubeExpectedTotal(doc.body);
    if(!entries.length&&expectedTotal!==0)throw new Error('YouTube playlist videos were not available. Wait for the public playlist rows to load and retry.');
    const title=youtubeText(doc.querySelector('ytd-playlist-header-renderer h1, ytmusic-playlist-header-renderer h1, main h1, h1'))||doc.title||'YouTube playlist';
    return {schemaVersion:1,provider:'youtube',kind:'playlist',title,sourceUrl:`https://www.youtube.com/playlist?list=${playlistId}`,capturedAt:new Date().toISOString(),complete:expectedTotal===0&&entries.length===0,expectedTotal,warnings,entries};
  }
  throw new Error('Open a public YouTube video or playlist page.');
}
export function extract(doc, url) {
  const provider = providerFor(url);
  if (!provider) throw new Error('Open a supported Beatport, Traxsource, Spotify, YouTube, Shazam, Last.fm or Bandcamp page.');
  if (isVerification(doc)) throw new Error('Complete provider verification in this browser tab, then import again.');
  if(provider==='youtube')return extractYouTube(doc,url);
  if(provider==='shazam')return extractShazam(doc,url);
  if(provider==='lastfm')return extractLastfm(doc,url);
  if(provider==='bandcamp'){
    const path=new URL(url).pathname;
    if(/^\/discover\/[^/]+\/?$/i.test(path))return bandcampDiscover(doc,url);
    if(/^\/album\/[^/]+\/?$/i.test(path))return bandcampAlbum(doc,url);
    if(/^\/(?:collection|collections)(?:\/[^/]+)?\/?$/i.test(path)||/^\/music\/?$/i.test(path))return bandcampCollection(doc,url);
    throw new Error('Open a Bandcamp Discover tag page, album, or collection page with a rendered song tracklist.');
  }
  const sourceUrl=provider==='spotify' ? canonicalSpotifyUrl(url) : url;
  const spotifyKind=provider==='spotify' ? (new URL(sourceUrl).pathname.match(/^\/(playlist|album)\/[^/]+\/?$/)?.[1] || null) : null;
  if (provider==='spotify' && !spotifyKind) throw new Error('Open a Spotify playlist or album.');
  // Use album-level artist credits only when Spotify explicitly provides them
  // in the album heading. They are a safe fallback for rows whose own artist
  // links are absent, but must never leak into playlist entries.
  let spotifyAlbumArtist='';
  if(provider==='spotify'&&spotifyKind==='album') {
    const heading=doc.querySelector('main h1,[role="main"] h1,h1');
    for(let node=heading?.parentElement;node&&!spotifyAlbumArtist;node=node.parentElement) {
      if(node.querySelector('[data-testid="tracklist-row"], [role="grid"] [role="row"]'))break;
      const artists=[...node.querySelectorAll('a[href*="/artist/"]')]
        .map(a=>a.textContent.trim())
        .filter(name=>name&&!/^(?:various(?:\s+artists?)?|v\.?\s*a\.?)$/i.test(name));
      spotifyAlbumArtist=[...new Set(artists)].join(', ');
    }
  }
  let entries=[];
  if (provider==='traxsource') {
    for (const row of doc.querySelectorAll('.trk-row')) {
      const a=row.querySelector('.title a[href*="/track/"]');
      const sourceUrl=link(a?.getAttribute('href'),url); const providerId=sourceUrl?.match(/\/track\/(\d+)/)?.[1];
      const artists=[...row.querySelectorAll('.artists a')].filter(a=>!a.closest('.remixers,.remixer')).map(a=>a.textContent.trim()).filter(Boolean).join(', ');
      if (!providerId || !artists || !a.textContent.trim()) continue;
      entries.push(entry(providerId,sourceUrl,artists,a.textContent.trim(),entries.length+1,{rank:rankNumber(text(row,'.tnum,.position,.rank')?.replace(/^#/,'')),version:text(row,'.version,.mix,.mix-name'),durationSeconds:time(text(row,'.title')),label:text(row,'.label'),genre:text(row,'.genre'),releaseDate:text(row,'.r-date'),bpm:num(text(row,'.bpm')),key:text(row,'.key'),artworkUrl:link(row.querySelector('img')?.getAttribute('src'),url),isrc:normalizedIsrc(row.getAttribute('data-isrc')),catalogNumber:text(row,'.catalog-number,.catalog'),metadata:rowMetadata(row,url)}));
    }
  } else if (provider==='beatport') {
    const named=v=>{
      if(typeof v==='string')return v.trim()||null;
      const candidate=v?.name??v?.artist?.name??v?.artist?.artist_name??v?.artist_name;
      return typeof candidate==='string'?candidate.trim()||null:null;
    };
    const walk=(v,depth=0)=>{
      if(depth>30 || !v || typeof v!=='object') return;
      if(Array.isArray(v)){v.forEach(x=>walk(x,depth+1));return;}
      const t=v.track||v; const rank=rankNumber(v.position??v.rank??t.chart_position);
      const trackTitle=[t.name,t.title].find(value=>typeof value==='string'&&value.trim());
      // Require explicit chart rank, avoiding related tracks and recommendations.
      if(rank && /^\d+$/.test(String(t.id)) && trackTitle && Array.isArray(t.artists)) {
        const artist=t.artists.filter(a=>!/remix/i.test(a.role?.name||a.role||'')).map(named).filter(Boolean).join(', ');
        if(artist) entries.push(entry(String(t.id),`https://www.beatport.com/track/${t.slug||'track'}/${t.id}`,artist,trackTitle.trim(),rank,{rank,version:t.mix_name||null,durationSeconds:num(t.length_ms)?t.length_ms/1000:num(t.duration),bpm:num(t.bpm),key:named(t.key),label:named(t.release?.label??t.label),album:named(t.release),genre:named(t.genre),releaseDate:t.publish_date||null,artworkUrl:link(t.release?.image?.uri??t.image?.uri,url),isrc:t.isrc||null,catalogNumber:t.release?.catalog_number||null,metadata:{artistUrls:t.artists.map(a=>{const v=a.artist||a;return v.id?`https://www.beatport.com/artist/${v.slug||'artist'}/${v.id}`:null;}).filter(Boolean),albumUrl:t.release?.id?`https://www.beatport.com/release/${t.release.slug||'release'}/${t.release.id}`:null,remixers:[...(t.remixers||[]),...t.artists.filter(a=>/remix/i.test(a.role?.name||a.role||''))].map(named).filter(Boolean)}})); return;
      }
      Object.values(v).forEach(x=>walk(x,depth+1));
    };
    for(const s of doc.querySelectorAll('script#__NEXT_DATA__,script[type="application/json"]')) {try{walk(JSON.parse(s.textContent));}catch{/* unrelated script */}}
    // Resolve rows before generic test IDs: a track-title link may itself carry
    // data-testid and must never be mistaken for its surrounding table row.
    const strictRank = value => rankNumber(typeof value==='string' ? value.trim().replace(/^#\s*/, '') : value);
    const visible = node => !node.closest('[hidden],[aria-hidden="true"]');
    const rows = new Set();
    for(const a of doc.querySelectorAll('a[href*="/track/"]')) {
      let row=a.closest('[data-testid="tracks-table-row"], [data-testid="tracks-list-item"], [data-testid="track-row"]') || a.closest('tr, [role="row"]') || a.closest('[data-track-id]');
      if(!row) {
        // Styled-component names vary; keep this fallback local to one recording.
        for(let parent=a.parentElement, depth=0;parent && depth<7;parent=parent.parentElement,depth++) {
          const ids=new Set([...parent.querySelectorAll('a[href*="/track/"]')].map(v=>v.getAttribute('href')?.match(/\/track\/[^/]+\/(\d+)/)?.[1]).filter(Boolean));
          if(ids.size>1)break;
          if(ids.size===1 && parent.querySelector('a[href*="/artist/"]') && parent.querySelector('.cell,[data-testid*="rank"],.rank,.position')) {row=parent;break;}
        }
      }
      if(row && visible(row))rows.add(row);
    }
    const domEntries=[];
    for(const row of rows) {
      const anchors=[...row.querySelectorAll('a[href*="/track/"]')].filter(a=>a.textContent.trim());
      const a=anchors.find(a=>a.closest('.cell.title,[data-testid="track-title"]')) || anchors[0];
      if(!a)continue;
      const sourceUrl=link(a.getAttribute('href'),url), id=sourceUrl?.match(/\/track\/[^/]+\/(\d+)/)?.[1];
      let rank=null;
      for(const node of row.querySelectorAll('[data-testid="track-rank"],[data-testid*="rank"],.rank,.position,.cell.num,.cell.number,.cell.index,[class*="__TrackNo-"],[data-rank]')) {
        rank=strictRank(node.getAttribute('data-rank')) || strictRank(node.textContent);if(rank)break;
      }
      // Beatport's rendered table can expose its source rank solely as the first
      // numeric cell. Never invent ranks from array order or use BPM/duration cells.
      if(!rank) {
        const first=row.querySelector('.cell,[role="cell"],[role="gridcell"],td');
        if(first && !first.querySelector('a[href*="/track/"],a[href*="/artist/"]'))rank=strictRank(first.textContent);
      }
      // The compact chart layout exposes its source rank as a numeric button.
      // Only read that explicit control; never derive rank from list order.
      if(!rank && row.matches('[data-testid="tracks-list-item"]')) {
        for(const button of row.querySelectorAll('button')) {
          rank=strictRank(button.textContent) || strictRank(button.getAttribute('aria-label'));if(rank)break;
        }
      }
      const artists=[...row.querySelectorAll('a[href*="/artist/"]')].filter(v=>!v.closest('.remixers,.remixer')).map(v=>v.textContent.trim()).filter(Boolean);
      const artist=[...new Set(artists)].join(', ') || text(row,'[data-testid="track-artists"],.artists');
      if(!rank || !artist || !id)continue;
      const releaseName=a.querySelector('[class*="__ReleaseName-"],[class*="__ItemName-"]');
      const nestedMix=releaseName?.querySelector(':scope > span:last-child');
      const mixName=nestedMix?.textContent.trim() || text(row,'[data-testid="track-mix"],.mix-name,.mix,.version');
      const title=releaseName ? [...releaseName.childNodes].filter(n=>n!==nestedMix).map(n=>n.textContent).join('').trim() : text(a,'[data-testid="track-name"],.track-name') || a.textContent.trim();
      const bpmKey=text(row,'.cell.bpm-key,[data-testid="track-bpm-key"]');
      const bpmText=text(row,'.cell.bpm,[data-testid="track-bpm"],.bpm') || bpmKey;
      const combined=bpmText?.match(/^\s*(\d+(?:\.\d+)?)\s*BPM(?:\s*[-–—]\s*(.+))?\s*$/i);
      domEntries.push(entry(id,sourceUrl,artist,title,rank,{rank,
        version:mixName,
        durationSeconds:time(text(row,'.cell.length,.cell.duration,[data-testid="track-length"],[data-testid="track-duration"]')),
        bpm:num(combined?.[1] || bpmText),key:text(row,'.cell.key,[data-testid="track-key"],.key') || combined?.[2]?.trim() || null,
        label:text(row,'.cell.label a,a[href*="/label/"],.label'),genre:text(row,'.cell.genre a,.cell.genre,[data-testid="track-genre"]'),
        releaseDate:text(row,'.cell.release-date,.cell.released,.cell.date,[data-testid="track-release-date"]'),
        artworkUrl:link(row.querySelector('img')?.getAttribute('src'),url),metadata:rowMetadata(row,url)}));
    }
    // Visible rows define membership when available. Page JSON can also contain a
    // different ranked chart in recommendations; only enrich these same IDs.
    if(domEntries.length) {
      entries=domEntries.map(e=>{
        const metadata=entries.find(v=>v.providerId===e.providerId);
        if(!metadata)return e;
        return {...metadata,...Object.fromEntries(Object.entries(e).filter(([,v])=>v!==null)),metadata:{...(metadata.metadata||{}),...Object.fromEntries(Object.entries(e.metadata||{}).filter(([,v])=>v!=null&&(!Array.isArray(v)||v.length)))},title:metadata.title || e.title,artist:metadata.artist || e.artist};
      });
    }
    entries.sort((a,b)=>a.rank-b.rank);
  } else {
    // A live Spotify row wraps its presentation tracklist-row in an ARIA row.
    // Parse the inner row once, retaining wrapper indexes as a fallback.
    const spotifyRows=new Set([...doc.querySelectorAll('[data-testid="tracklist-row"], [role="row"][aria-rowindex]')].map(row=>row.querySelector('[data-testid="tracklist-row"]') || row));
    for(const row of spotifyRows) {
      const a=row.querySelector('a[href*="/track/"]');const sourceUrl=link(a?.getAttribute('href'),url);const id=sourceUrl?.match(/\/track\/([a-zA-Z0-9]+)/)?.[1];
      const artist=[...row.querySelectorAll('a[href*="/artist/"]')].map(a=>a.textContent.trim()).filter(Boolean).join(', ') || spotifyAlbumArtist;
      // aria-rowindex counts every row in its grid, including a column header.
      // Spotify commonly marks that header as row 1, making the first track 2.
      const grid=row.closest('[role="grid"], [role="table"]');
      const hasHeader=!!grid?.querySelector('[role="row"] [role="columnheader"]');
      const ariaIndex=rankNumber(row.getAttribute('aria-rowindex') || row.closest('[role="row"][aria-rowindex]')?.getAttribute('aria-rowindex'));
      const accessiblePosition=ariaIndex && !(hasHeader && ariaIndex===1) ? ariaIndex-(hasHeader?1:0) : null;
      const explicit=rankNumber(text(row,'[data-testid="tracklist-row-position"]')) || rankNumber(row.querySelector('[role="gridcell"]')?.textContent.trim()) || accessiblePosition;
      if(!id||!artist||!explicit)continue;
      entries.push(entry(id,sourceUrl,artist,a.textContent.trim(),explicit,{metadata:{...rowMetadata(row,url),availability:row.getAttribute('aria-disabled')==='true'?'unavailable':'available'},artworkUrl:link(row.querySelector('img')?.getAttribute('src'),url),album:text(row,'a[href*="/album/"]'),durationSeconds:time(text(row,'[aria-colindex="5"]') || [...row.querySelectorAll('[role="gridcell"]')].at(-1)?.textContent || row.textContent)}));
    }
  }
  let title=text(doc,'main h1,[role="main"] h1')||text(doc,'h1')||doc.title;
  // Spotify album pages expose the album artist beside the album heading.
  // Keep this strictly tied to that header metadata; track-row artists describe
  // individual recordings and must never be used as an album-level fallback.
  if(spotifyAlbumArtist)title=`${spotifyAlbumArtist} — ${title}`;
  const albumArtwork=provider==='spotify'&&spotifyKind==='album' ? link(doc.querySelector('main img[src], [role="main"] img[src]')?.getAttribute('src'),url) : null;
  // A chart title Top 100 describes its extent; playlists need explicit track-count text.
  const metadataText=[...(doc.querySelector('main')||doc.body).querySelectorAll('p,span')].map(e=>e.textContent).join(' ');
  const count=provider==='spotify' ? metadataText.match(/\b([\d,]+)\s+(?:songs?|tracks?)\b/i) : title.match(/\btop[\s-]+(\d+)\b/i) || doc.title.match(/\btop[\s-]+(\d+)\b/i) || new URL(url).pathname.match(/\/top-(\d+)\/?$/i);
  const expectedTotal=count?num(count[1].replaceAll(',','')):null;
  if(provider==='traxsource')entries=entries.map(e=>({...e,position:e.rank||e.position}));
  entries=entries.map(e=>Object.fromEntries(Object.entries({...e,...(albumArtwork?{artworkUrl:albumArtwork,album:title}:{})}).map(([k,v])=>[k,typeof v==='string'?v.slice(0,2000):typeof v==='number'?(Number.isFinite(v)?v:null):k==='metadata'?v:null])));
  return {schemaVersion:1,provider,kind:provider==='spotify'?spotifyKind:'chart',title,sourceUrl,capturedAt:new Date().toISOString(),complete:false,expectedTotal,warnings:[],entries};
}
export function mergeSnapshots(snapshots) {
  const snapshot={...snapshots[0], entries:[],warnings:[...new Set(snapshots.flatMap(s=>s.warnings||[]))]};const positions=new Map();
  // Keep repeated occurrences. Native app deduplicates IDs preserving occurrence provenance.
  for(const s of snapshots)for(const e of s.entries)positions.set(`${e.position}:${e.providerId}`,e);
  snapshot.entries=[...positions.values()].sort((a,b)=>a.position-b.position);
  const uniquePositions=new Set(snapshot.entries.map(e=>e.position));
  const emptyPlaylist=snapshot.kind==='playlist'&&snapshot.expectedTotal===0&&snapshot.entries.length===0;
  snapshot.complete=snapshot.warnings.length===0&&(emptyPlaylist||((snapshot.kind!=='chart'||snapshot.entries.every(e=>e.rank===e.position)) && Number.isInteger(snapshot.expectedTotal) && snapshot.expectedTotal>0 && snapshot.entries.length===snapshot.expectedTotal && uniquePositions.size===snapshot.expectedTotal && Math.min(...uniquePositions)===1 && Math.max(...uniquePositions)===snapshot.expectedTotal));
  if(!snapshot.complete)snapshot.warnings.push(snapshot.expectedTotal ? `Captured ${snapshot.entries.length} of ${snapshot.expectedTotal} entries; import may be incomplete.`:'Provider total unavailable; completeness is unknown.');
  snapshot.warnings=[...new Set(snapshot.warnings)];
  return snapshot;
}

// Opt-in local diagnostics contain one public recording row, never page JSON,
// authentication attributes, form fields, scripts, URLs with query strings, or cookies.
export function diagnosticSample(doc, url) {
  const provider=providerFor(url);
  if(!provider)throw new Error('Open a supported chart or playlist.');
  if(isVerification(doc))throw new Error('Complete provider verification before saving a row diagnostic.');
  const canonicalPath=value=>{
    try {
      const u=new URL(value,url);
      if(u.protocol!=='https:' || providerFor(u.href)!==provider)return null;
      const valid=provider==='shazam'?/^\/song\/\d+(?:\/|$)/.test(u.pathname):provider==='lastfm'?/^\/music\/[^/]+\/_\/[^/]+\/?$/.test(u.pathname):provider==='bandcamp'?/^\/track\/[^/]+\/?$/.test(u.pathname):/^\/(?:track|artist|label|genre|release|album)\//.test(u.pathname);
      return valid ? u.pathname : null;
    }catch{return null;}
  };
  const selector=provider==='shazam'?'a[href*="/song/"]':provider==='lastfm'?'a[href*="/music/"]':'a[href*="/track/"]';
  const ids=node=>new Set([...node.querySelectorAll(selector)].map(a=>canonicalPath(a.getAttribute('href'))).filter(Boolean));
  const first=[...doc.querySelectorAll(selector)].find(a=>canonicalPath(a.getAttribute('href'))&&a.textContent.trim());
  if(!first)throw new Error('No public track link found to diagnose.');
  let sample=first.parentElement, ancestors=[], rankedSample=false;
  for(let node=first.parentElement, depth=0;node && depth<10;node=node.parentElement,depth++) {
    const count=ids(node).size;
    ancestors.push({tag:node.tagName.toLowerCase(),className:(node.getAttribute('class')||'').slice(0,500),testId:(node.getAttribute('data-testid')||'').slice(0,200),role:node.getAttribute('role'),trackCount:count,artistLinkCount:node.querySelectorAll('a[href*="/artist/"]').length,childCount:node.children.length,numericLeaves:(count===1?[...node.querySelectorAll('span,div,td,button')]:[]).filter(e=>!e.children.length && /^\s*#?\s*\d{1,5}\s*$/.test(e.textContent)).slice(0,12).map(e=>({tag:e.tagName.toLowerCase(),className:(e.getAttribute('class')||'').slice(0,200),testId:e.getAttribute('data-testid'),text:e.textContent.trim()}))});
    if(count>1)break;
    if(!rankedSample && count===1 && !['MAIN','BODY','HTML'].includes(node.tagName) && node.querySelector('a[href*="/artist/"]') && node.querySelectorAll('*').length<=200 && !node.querySelector('form,input,nav,header')) {
      sample=node;
      rankedSample=ancestors.at(-1).numericLeaves.length>0;
    }
  }
  const output=doc.implementation.createHTMLDocument('');let remaining=500;
  function clean(node,depth=0) {
    if(depth>14 || remaining--<=0)return null;
    if(node.nodeType===3)return output.createTextNode(node.textContent.slice(0,300));
    if(node.nodeType!==1 || !/^(?:DIV|SPAN|P|A|BUTTON|TABLE|TBODY|TR|TD|TH|OL|UL|LI|SECTION|H[1-6]|SMALL|STRONG|EM|B|I|IMG)$/.test(node.tagName))return null;
    if(node.tagName==='BUTTON' && !/^\s*#?\s*\d{1,5}\s*$/.test(node.textContent) && !/^\s*#?\s*\d{1,5}\s*$/.test(node.getAttribute('aria-label')||''))return null;
    if(node.hasAttribute('hidden') || node.getAttribute('aria-hidden')==='true')return null;
    const clone=output.createElement(node.tagName.toLowerCase());
    if(node.tagName==='BUTTON' && /^\s*#?\s*\d{1,5}\s*$/.test(node.getAttribute('aria-label')||''))clone.setAttribute('aria-label',node.getAttribute('aria-label').trim());
    for(const attr of ['class','data-testid','role','aria-rowindex','aria-colindex','data-rank','data-track-id']) {
      const value=node.getAttribute(attr);if(value!==null)clone.setAttribute(attr,value.slice(0,500));
    }
    if(node.tagName==='A'){const path=canonicalPath(node.getAttribute('href'));if(path)clone.setAttribute('href',path);}
    for(const child of node.childNodes){const safe=clean(child,depth+1);if(safe)clone.appendChild(safe);}
    return clone;
  }
  const sanitized=clean(sample);
  return {schemaVersion:1,provider,sourcePath:new URL(url).pathname,trackLinkPath:canonicalPath(first.getAttribute('href')),ancestors,sampleHtml:sanitized?.outerHTML||'',truncated:remaining<=0};
}
