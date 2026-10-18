if(!globalThis.BurrataSpotifyAuth&&typeof importScripts==='function')importScripts('spotify-auth.js');
if(!globalThis.BurrataSpotifyProvider&&typeof importScripts==='function')importScripts('provider-operations.js');
const api=globalThis.browser||chrome;
const sites=['https://www.beatport.com/*','https://www.traxsource.com/*','https://open.spotify.com/*','https://www.youtube.com/*','https://music.youtube.com/*','https://www.shazam.com/*','https://shazam.com/*','https://www.last.fm/*','https://last.fm/*','https://bandcamp.com/*','https://*.bandcamp.com/*'];
let port=null,waiters=[],nativeBusy=Promise.resolve(),pollBusy=false;
let lastError='',requestTabs=new Map(),enabled=false;
const providerOperations=new Map();
let cachedSpotifyHashes={};
let managedSessionId=null;
// Pairing target and account approval are intentionally memory-only. A worker
// restart discards approval and requires a fresh user confirmation.
let pendingYouTubeHandoff=null;
const HASH_REGISTRY='https://francescograzioso.github.io/Meld/spotify-gql-hashes.json';
const SPOTIFY_HASHES={profileAttributes:'53bcb064f6cd18c23f752bc324a791194d20df612d8e1239c735144ab0399ced',libraryV3:'973e511ca44261fda7eebac8b653155e7caee3675abb4fb110cc1b8c78b091c3',fetchPlaylist:'346811f856fb0b7e4f6c59f8ebea78dd081c6e2fb01b77c954b26259d5fc6763',searchDesktop:'4801118d4a100f756e833d33984436a3899cff359c532f8fd3aaf174b60b3b49',addToPlaylist:'47b2a1234b17748d332dd0431534f22450e9ecbb3d5ddcdacbd83368636a0990',removeFromPlaylist:'47b2a1234b17748d332dd0431534f22450e9ecbb3d5ddcdacbd83368636a0990',moveItemsInPlaylist:'47b2a1234b17748d332dd0431534f22450e9ecbb3d5ddcdacbd83368636a0990'};
// Chromium may suspend its MV3 worker while the user completes verification.
// Session storage survives worker restarts without retaining stale tab IDs after
// the browser itself restarts.
const ready=Promise.all([api.storage.local.get(['enabled','spotifyHashes','providerMutationOperations']),api.storage.session.get(['requestTabs','providerOperations'])]).then(async ([settings,session])=>{
  enabled=!!settings.enabled;
  cachedSpotifyHashes=settings.spotifyHashes||{};
  requestTabs=new Map(session.requestTabs||[]);
  for(const [id,record] of session.providerOperations||[])providerOperations.set(id,record);
  for(const [id,record] of settings.providerMutationOperations||[])providerOperations.set(id,record);
  // A read may be safely repeated if an MV3 worker died mid-request. Mutation
  // records remain running so the retry path reports an ambiguous outcome.
  let recoveredRead=false;
  for(const [id,record] of providerOperations){if(record?.state==='running'&&!record.mutation){providerOperations.delete(id);recoveredRead=true;}}
  if(recoveredRead)await persistProviderOperations();
});
const managedSessionReady=(async()=>{await ready;try{const response=await fetch(api.runtime.getURL('managed-session.json'));if(response.ok){const value=await response.json();if(typeof value.sessionId==='string'&&/^[A-Za-z0-9-]{16,128}$/.test(value.sessionId)){managedSessionId=value.sessionId;enabled=true;await api.storage.local.set({enabled:true});}}}catch{/* absent in the user-installed companion */}})();
const errorResult=(code,message)=>{const error=new Error(message);error.code=code;return error;};
async function fetchBounded(url,options={},timeoutMs=15000){
  const controller=new AbortController(),timeout=setTimeout(()=>controller.abort(),timeoutMs);
  try{const response=await fetch(url,{...options,signal:controller.signal}),body=await response.text();return {ok:response.ok,status:response.status,text:async()=>body,json:async()=>JSON.parse(body)};}
  catch(error){if(controller.signal.aborted)throw errorResult('REQUEST_TIMEOUT','Provider registry request timed out. Try again shortly.');throw error;}
  finally{clearTimeout(timeout);}
}
async function getSpotifyServerTimeMain(){
  const controller=new AbortController(),timeout=setTimeout(()=>controller.abort(),15000);
  try{const response=await fetch('/api/server-time',{credentials:'include',headers:{Accept:'application/json'},signal:controller.signal});if(!response.ok)return {__burrataProviderError:{code:response.status===429?'RATE_LIMITED':'PROVIDER_CHANGED',message:`Spotify server-time request failed (${response.status}).`}};const body=await response.json();return {serverTime:Number(body.serverTime)};}
  catch(error){return {__burrataProviderError:{code:controller.signal.aborted?'REQUEST_TIMEOUT':'PROVIDER_CHANGED',message:controller.signal.aborted?'Spotify server-time request timed out.':String(error?.message||'Spotify server time was unavailable.')}};}
  finally{clearTimeout(timeout);}
}
async function runProviderPageOperation(provider,operation,args,hashes){
  const fail=(code,message)=>{throw Object.assign(new Error(message),{code});};
  const fetchBounded=async(url,options={})=>{const controller=new AbortController(),timeout=setTimeout(()=>controller.abort(),15000);try{const response=await fetch(url,{...options,signal:controller.signal}),body=await response.text();return {ok:response.ok,status:response.status,text:async()=>body,json:async()=>JSON.parse(body)};}catch(error){if(controller.signal.aborted)fail('REQUEST_TIMEOUT','Provider request timed out. Try again shortly.');throw error;}finally{clearTimeout(timeout);}};
  const json=async(response)=>{let body={};try{body=await response.json();}catch{}const bodyText=JSON.stringify(body),providerErrors=[body?.error?.message,body?.error_description,body?.message&&(['ERROR','FAILED','UNAVAILABLE'].includes(String(body?.status||'').toUpperCase())?body.message:null),...(Array.isArray(body?.errors)?body.errors.map(error=>error?.message):[])].filter(Boolean).join(' ');if(/captcha|verification|challenge/i.test(providerErrors))fail('VERIFICATION_REQUIRED','Complete the provider verification in the browser, then retry.');if(response.status===401||response.status===403)fail('SIGN_IN_REQUIRED','Sign in to this provider in the browser, then retry.');if(response.status===429)fail('RATE_LIMITED','The provider is rate limiting requests. Try again shortly.');if(response.status===412||bodyText.includes('PersistedQueryNotFound'))fail('HASH_EXPIRED','Spotify rejected the cached GraphQL hash.');if(!response.ok)fail('PROVIDER_CHANGED',`Provider request failed (${response.status}).`);return body;};
  const first=(...values)=>values.find(value=>value!==undefined&&value!==null&&value!=='');
  const images=(value)=>{if(!value)return null;if(Array.isArray(value)){for(const item of value){const found=images(item);if(found)return found;}return null;}if(typeof value==='object'){if(typeof value.url==='string')return value.url;for(const key of ['sources','items','images','coverArt','avatar','image','thumbnail','thumbnailViewModel','collectionThumbnailViewModel','primaryThumbnail','contentPreviewImageViewModel']){const found=images(value[key]);if(found)return found;}}return null;};
  const cursor=(n)=>String(Math.max(0,Number(n)||0));
  const spotifyDurationSeconds=data=>{const milliseconds=[data.duration?.totalMilliseconds,data.trackDuration?.totalMilliseconds,data.duration?.milliseconds,data.trackDuration?.milliseconds,data.durationMs,data.duration_ms].map(Number).find(value=>Number.isFinite(value)&&value>0&&value<=86400000);if(milliseconds!==undefined)return milliseconds/1000;const seconds=[data.durationSeconds,data.trackDurationSeconds,data.duration?.totalSeconds,data.trackDuration?.totalSeconds,data.duration?.seconds,data.trackDuration?.seconds,typeof data.duration==='number'?data.duration:null,typeof data.trackDuration==='number'?data.trackDuration:null].map(Number).find(value=>Number.isFinite(value)&&value>0&&value<=86400);return seconds===undefined?null:seconds;};
  const spotifyTrack=(data,uid,uriOverride)=>{const uri=first(data.uri,data._uri,uriOverride);const id=first(data.id,uri?.split(':').pop());if(!id)return null;const artists=first(data.artists?.items,data.artists)||[];const album=data.albumOfTrack||data.album||{};return {entryId:uid||null,trackId:id,title:first(data.name,data.title)||'',artist:artists.map(x=>x.profile?.name||x.name).filter(Boolean).join(', '),album:first(album.name,album.title)||'',durationSeconds:spotifyDurationSeconds(data),url:`https://open.spotify.com/track/${id}`,artworkUrl:images(album.coverArt||album.images)||null,available:true};};
  const spotifyPlaylist=(data,owner,selfId)=>{const id=first(data.id,data.uri?.split(':').pop(),data._uri?.split(':').pop());if(!id)return null;const ownerData=owner||data.ownerV2?.data||data.owner||{};const ownerId=first(ownerData.id,ownerData.uri?.split(':').pop())||null;const attributes=data.attributes,attributeItems=Array.isArray(attributes)?attributes:Array.isArray(attributes?.items)?attributes.items:[],attributeValue=names=>first(...names.map(name=>attributes?.[name]),attributeItems.find(item=>names.includes(String(item?.key||item?.name||item?.label||'').toLowerCase()))?.value);const countCandidates=[data.content?.totalCount,data.tracks?.total,data.tracks?.totalCount,data.totalCount,data.trackCount,data.totalTracks,data.attributes?.totalCount,data.attributes?.trackCount,data.attributes?.totalTracks,data.playlistAttributes?.totalCount,data.playlistAttributes?.trackCount,attributeValue(['total','totalcount','trackcount','totaltracks'])];const total=countCandidates.filter(value=>value!==null&&value!==undefined&&value!=='').map(Number).find(value=>value>=0&&Number.isFinite(value));const rawVisibility=String(first(data.visibility,data.attributes?.visibility,attributeValue(['visibility']))||'').toLowerCase(),rawPublic=first(data.public,data.isPublic,data.attributes?.public,attributeValue(['public','ispublic'])),visibility=['public','private','unlisted'].includes(rawVisibility)?rawVisibility:rawPublic===true?'public':null;return {id,provider:'spotify',title:first(data.name,data.title)||'Untitled playlist',url:`https://open.spotify.com/playlist/${id}`,ownerId,ownerUrl:ownerId?`https://open.spotify.com/user/${encodeURIComponent(ownerId)}`:null,ownerName:first(ownerData.name,ownerData.displayName)||null,artworkUrl:images(data.images||data.coverArt)||null,total:total??null,visibility,isPublic:typeof rawPublic==='boolean'?rawPublic:null,canEdit:!!data.canEdit||!!(selfId&&ownerId===selfId)};};
  const spotifyArtist=data=>{const id=first(data.id,data.uri?.split(':').pop(),data._uri?.split(':').pop());if(!id)return null;const title=first(data.profile?.name,data.name)||'';return {id,kind:'artist',title,subtitle:'Artist',url:`https://open.spotify.com/artist/${id}`,artworkUrl:images(data.visuals?.avatarImage||data.avatar||data.images)||null,data:{provider:'spotify',artistId:id,name:title,uri:`spotify:artist:${id}`,genres:data.genres||[],popularity:Number.isFinite(data.popularity)?data.popularity:null,followers:data.followers?.total??null}};};
  const spotifyAlbum=data=>{const id=first(data.id,data.uri?.split(':').pop(),data._uri?.split(':').pop());if(!id)return null;const sourceArtists=Array.isArray(data.artists)?data.artists:(data.artists?.items||[]),artists=sourceArtists.map(x=>x.profile?.name||x.name).filter(Boolean);const title=data.name||'';return {id,kind:'album',title,subtitle:artists.join(', '),url:`https://open.spotify.com/album/${id}`,artworkUrl:images(data.coverArt||data.images)||null,data:{provider:'spotify',albumId:id,name:title,artists,releaseDate:data.release_date||data.date?.isoString||null,releaseYear:data.release_date?.slice?.(0,4)||data.date?.year||null,totalTracks:data.total_tracks??data.tracks?.total??null,albumType:data.album_type||null,uri:`spotify:album:${id}`}};};
  const spotifyAccount=async(auth,hashSet)=>{const body=await gql('profileAttributes',{},auth,hashSet);const profile=body.data?.me?.profile;const accountId=profile?.uri?.split(':').pop();if(!accountId)fail('SIGN_IN_REQUIRED','Spotify did not return a signed-in profile.');return {accountId,displayName:profile.name||accountId};};
  const gql=async(name,variables,auth,hashSet)=>{const entry=hashSet?.[name];const candidates=[entry?.hash,entry?.previous_hash,entry?.previousHash,entry?.fallback,entry].filter((v,i,a)=>typeof v==='string'&&/^[a-f0-9]{64}$/i.test(v)&&a.indexOf(v)===i);if(!candidates.length)fail('UNSUPPORTED',`Spotify operation ${name} has no verified hash.`);for(const hash of candidates){const response=await fetchBounded('https://api-partner.spotify.com/pathfinder/v2/query',{method:'POST',credentials:'include',headers:{Authorization:`Bearer ${auth}`,'Content-Type':'application/json','app-platform':'WebPlayer'},body:JSON.stringify({operationName:name,variables,extensions:{persistedQuery:{version:1,sha256Hash:hash}}})});let body={};try{body=await response.json();}catch{}if(response.status===412||JSON.stringify(body).includes('PersistedQueryNotFound'))continue;if(!response.ok)await json(new Response(JSON.stringify(body),{status:response.status}));if(body.errors?.length){if(body.errors.some(e=>String(e.message).includes('PersistedQueryNotFound')))continue;fail('PROVIDER_CHANGED','Spotify returned an unsupported GraphQL response.');}return body;}fail('HASH_EXPIRED',`Spotify rejected all cached hashes for ${name}.`);};
  const spotifyCatalog=async(url,auth,notFoundCode='API_DENIED')=>{const response=await fetchBounded(url,{credentials:'include',headers:{Authorization:`Bearer ${auth}`,Accept:'application/json'}});let body={};try{body=await response.json();}catch{}if(response.status===401)fail('SIGN_IN_REQUIRED','Spotify rejected the signed-in browser session. Sign in to Spotify, then retry.');if(response.status===403)fail('API_DENIED','Spotify denied this catalog request. The endpoint may require an approved Spotify developer app or may be unavailable to this account.');if(response.status===429)fail('RATE_LIMITED','Spotify is rate limiting catalog requests. Try again shortly.');if(response.status===404)fail(notFoundCode,'Spotify does not expose this catalog item through the current API.');if(!response.ok)fail('PROVIDER_CHANGED',`Spotify catalog request failed (${response.status}).`);return body;};
  const spotifyToken=async()=>{if(!hashes?.totp?.code||!Number.isInteger(hashes.totp.version))fail('PROVIDER_CHANGED','Spotify TOTP parameters are unavailable.');const query=new URLSearchParams({reason:'transport',productType:'web-player',totp:hashes.totp.code,totpServer:hashes.totp.code,totpVer:String(hashes.totp.version)});const response=await fetchBounded(`https://open.spotify.com/api/token?${query}`,{credentials:'include',headers:{Accept:'application/json'}});const body=await json(response);if(body.isAnonymous===true||!body.accessToken)fail('SIGN_IN_REQUIRED','Spotify returned an anonymous token. Sign in to Spotify in the browser, then retry.');return body.accessToken;};
  const spotify=async()=>{const auth=await spotifyToken(),me=await spotifyAccount(auth,hashes);if(operation==='status')return me;
    if(operation==='search'){
      const query=String(args.query||'').trim();if(!query)fail('UNSUPPORTED','Search query is empty.');
      const kind=['track','artist','album','playlist','profile'].includes(args.kind)?args.kind:'track';
      const limit=Math.min(50,Math.max(1,Number(args.limit)||20)),offset=Number(cursor(args.cursor));
      const body=await gql('searchDesktop',{searchTerm:query,offset,limit,numberOfTopResults:5,includeAudiobooks:false,includeArtistHasConcertsField:false,includePreReleases:false,includeLocalConcertsField:false,includeAuthors:false},auth,hashes);
      const section=body.data?.searchV2;if(!section)fail('PROVIDER_CHANGED','Spotify search response was not recognized.');
      const key={track:'tracksV2',artist:'artists',album:'albumsV2',playlist:'playlists',profile:'profiles'}[kind];
      const raw=section[key]?.items||section.people?.items||section.users?.items||[];
      const items=raw.map(wrapper=>{
        const item=wrapper.item||wrapper,data=item.data||item;
        if(kind==='track'){const track=spotifyTrack(data,null,item._uri||item.uri);return track?{id:track.trackId,kind,title:track.title,subtitle:track.artist,url:track.url,artworkUrl:track.artworkUrl,data:{...track,id:track.trackId,provider:'spotify'}}:null;}
        if(kind==='artist')return spotifyArtist(data);
        if(kind==='album')return spotifyAlbum(data);
        if(kind==='playlist'){const playlist=spotifyPlaylist(data);return playlist?{id:playlist.id,kind,title:playlist.title,subtitle:playlist.ownerName||'',url:playlist.url,artworkUrl:playlist.artworkUrl,data:playlist}:null;}
        const profile=data.profile||data,trimmed=(...values)=>values.map(value=>typeof value==='string'?value.trim():value).find(value=>value!==undefined&&value!==null&&value!=='');let id=trimmed(data.id,data.uri?.split(':').pop(),data._uri?.split(':').pop(),item.id,item.uri?.split(':').pop(),item._uri?.split(':').pop(),profile.id,profile.uri?.split(':').pop(),profile._uri?.split(':').pop(),profile.username,data.username);if(typeof id==='string'&&id.startsWith('spotify:user:'))id=id.slice('spotify:user:'.length);if(!id)return null;const title=trimmed(profile.displayName,profile.name,data.displayName,data.name,data.username,id);return {id,kind:'profile',title,subtitle:'Profile',url:`https://open.spotify.com/user/${encodeURIComponent(id)}`,artworkUrl:images(profile.avatar||data.avatar)||null,data:{provider:'spotify',accountId:id,displayName:title}};
      }).filter(Boolean);
      const rawTotal=section[key]?.totalCount??section.people?.totalCount??section.users?.totalCount;
      const total=rawTotal===null||rawTotal===undefined||rawTotal===''?null:Number(rawTotal);
      const hasMore=total===null?raw.length>=limit:offset+raw.length<total;
      return {accountId:me.accountId,items,nextCursor:hasMore?String(offset+raw.length):null};
    }
    if(operation==='listPlaylists'){const limit=Math.min(50,Math.max(1,Number(args.limit)||25)),offset=Number(cursor(args.cursor));if(args.ownerId){let owner=String(args.ownerId);if(/^https?:\/\//i.test(owner)){let parsed;try{parsed=new URL(owner);}catch{fail('UNSUPPORTED','Spotify playlist owner URL is invalid.');}if(parsed.protocol!=='https:'||parsed.hostname!=='open.spotify.com')fail('UNSUPPORTED','Only Spotify profile URLs are supported.');const match=parsed.pathname.replace(/\/+$/,'').replace(/\/playlists$/,'').match(/^\/user\/(.+)$/);if(!match)fail('UNSUPPORTED','Spotify owner URL must identify a user profile.');try{owner=decodeURIComponent(match[1]);}catch{fail('UNSUPPORTED','Spotify profile URL encoding is invalid.');}}if(owner.startsWith('spotify:user:'))owner=owner.slice('spotify:user:'.length);if(!owner||owner.length>180||/[\s\u0000-\u001f\u007f/\\?#]/u.test(owner))fail('UNSUPPORTED','Spotify user ID is invalid.');owner=encodeURIComponent(owner);const url=`https://api.spotify.com/v1/users/${owner}/playlists?limit=${limit}&offset=${offset}`;const body=await json(await fetchBounded(url,{credentials:'include',headers:{Authorization:`Bearer ${auth}`,Accept:'application/json'}}));const rawItems=Array.isArray(body.items)?body.items:[],items=rawItems.map(x=>spotifyPlaylist(x,x.owner)).filter(Boolean),consumed=rawItems.length;return {accountId:me.accountId,items,nextCursor:body.next?String(offset+consumed):null};}const body=await gql('libraryV3',{filters:['Playlists'],order:null,textFilter:'',features:['LIKED_SONGS','YOUR_EPISODES_V2','PRERELEASES','EVENTS'],limit,offset,flatten:true,expandedFolders:[],folderUri:null,includeFoldersWhenFlattening:false},auth,hashes);const library=body.data?.me?.libraryV3;if(!library)fail('PROVIDER_CHANGED','Spotify playlist library response was not recognized.');const rawItems=Array.isArray(library.items)?library.items:[],items=rawItems.map(x=>{const wrapper=x.item||{},data=wrapper.data||{};return /Playlist/i.test(wrapper.__typename||'')?spotifyPlaylist(data,null,me.accountId):null;}).filter(Boolean),consumed=rawItems.length,rawTotal=library.totalCount,total=rawTotal===null||rawTotal===undefined||rawTotal===''?null:Number(rawTotal),hasMore=total===null?consumed>=limit:offset+consumed<total;return {accountId:me.accountId,items,nextCursor:consumed>0&&hasMore?String(offset+consumed):null};}
    if(operation==='getArtist'||operation==='getArtistAlbums'||operation==='getAlbum'||operation==='getAlbumTracks'){
      const artistId=String(args.artistId||''),albumId=String(args.albumId||'');
      if(['getArtist','getArtistAlbums'].includes(operation)&&! /^[A-Za-z0-9]{22}$/.test(artistId))fail('UNSUPPORTED','Spotify artist ID is invalid.');
      if(['getAlbum','getAlbumTracks'].includes(operation)&&! /^[A-Za-z0-9]{22}$/.test(albumId))fail('UNSUPPORTED','Spotify album ID is invalid.');
      if(operation==='getArtist'){
        const body=await spotifyCatalog(`https://api.spotify.com/v1/artists/${encodeURIComponent(artistId)}`,auth);
        if(body.id!==artistId)fail('PROVIDER_CHANGED','Spotify returned a different artist than requested.');
        const artist=spotifyArtist(body);if(!artist)fail('PROVIDER_CHANGED','Spotify artist response was not recognized.');
        return {accountId:me.accountId,artist};
      }
      if(operation==='getArtistAlbums'){
        const limit=Math.min(30,Math.max(1,Number(args.limit)||30)),offset=Number(cursor(args.cursor));
        if(!Number.isSafeInteger(offset)||offset<0||offset>100000)fail('UNSUPPORTED','Spotify album page cursor is invalid.');
        const query=new URLSearchParams({limit:String(limit),offset:String(offset)});
        const body=await spotifyCatalog(`https://api.spotify.com/v1/artists/${encodeURIComponent(artistId)}/albums?${query}`,auth);
        const rawItems=Array.isArray(body.items)?body.items:[],items=rawItems.map(spotifyAlbum).filter(Boolean),consumed=rawItems.length,total=Number(body.total);
        return {accountId:me.accountId,artistId,items,total:Number.isFinite(total)?total:null,nextCursor:body.next&&consumed?String(offset+consumed):null};
      }
      if(operation==='getAlbum'){
        const body=await spotifyCatalog(`https://api.spotify.com/v1/albums/${encodeURIComponent(albumId)}`,auth);
        if(body.id!==albumId)fail('PROVIDER_CHANGED','Spotify returned a different album than requested.');
        const album=spotifyAlbum(body);if(!album)fail('PROVIDER_CHANGED','Spotify album response was not recognized.');
        return {accountId:me.accountId,album};
      }
      const limit=Math.min(50,Math.max(1,Number(args.limit)||50)),offset=Number(cursor(args.cursor));
      if(!Number.isSafeInteger(offset)||offset<0||offset>100000)fail('UNSUPPORTED','Spotify track page cursor is invalid.');
      const query=new URLSearchParams({limit:String(limit),offset:String(offset)});
      const body=await spotifyCatalog(`https://api.spotify.com/v1/albums/${encodeURIComponent(albumId)}/tracks?${query}`,auth);
      const rawItems=Array.isArray(body.items)?body.items:[],items=rawItems.map((data,index)=>{const track=spotifyTrack(data,null,data.uri);return track?{...track,provider:'spotify',id:track.trackId,position:offset+index+1}:null;}).filter(Boolean),consumed=rawItems.length,total=Number(body.total);
      return {accountId:me.accountId,albumId,items,total:Number.isFinite(total)?total:null,nextCursor:body.next&&consumed?String(offset+consumed):null};
    }
    if(operation==='getPlaylist'){const id=String(args.playlistId||'');if(!/^[A-Za-z0-9]{10,64}$/.test(id))fail('UNSUPPORTED','Spotify playlist ID is invalid.');const limit=Math.min(100,Math.max(1,Number(args.limit)||50)),offset=Number(cursor(args.cursor));const body=await gql('fetchPlaylist',{uri:`spotify:playlist:${id}`,offset,limit,enableWatchFeedEntrypoint:false},auth,hashes);const raw=body.data?.playlistV2;if(!raw?.content)fail('PROVIDER_CHANGED','Spotify playlist response was not recognized.');const playlist=spotifyPlaylist(raw,null,me.accountId),rawItems=Array.isArray(raw.content.items)?raw.content.items:[],entries=rawItems.map(x=>{const w=x.itemV2||{},d=w.data||{},uid=first(x.uid,w.uid)||null,uri=first(w._uri,w.uri,d._uri,d.uri);return spotifyTrack(d,uid,uri)||{entryId:uid,trackId:null,title:first(d.name,d.title)||'Unavailable track',artist:'',album:'',durationSeconds:null,url:null,artworkUrl:null,available:false};});const rawTotal=raw.content.totalCount,total=rawTotal===null||rawTotal===undefined||rawTotal===''?null:Number(rawTotal),consumed=rawItems.length,hasMore=total===null?consumed>=limit:offset+consumed<total;return {accountId:me.accountId,playlist,entries,nextCursor:consumed>0&&hasMore?String(offset+consumed):null,total,revision:first(raw.revisionId,raw.snapshotId,raw.revision)||null,complete:!hasMore};}
    if(['addTracks','removeTracks','moveTracks'].includes(operation)){const id=String(args.playlistId||'');if(!/^[A-Za-z0-9]{10,64}$/.test(id))fail('UNSUPPORTED','Spotify playlist ID is invalid.');let name,variables;if(operation==='addTracks'){const uris=args.trackUris;if(!Array.isArray(uris)||!uris.length||uris.length>100||uris.some(x=>!/^spotify:track:[A-Za-z0-9]{10,64}$/.test(x)))fail('UNSUPPORTED','Track URIs must be Spotify track IDs.');name='addToPlaylist';variables={playlistUri:`spotify:playlist:${id}`,playlistItemUris:uris,newPosition:{moveType:'BOTTOM_OF_PLAYLIST',fromUid:null}};}else if(operation==='removeTracks'){const ids=args.entryIds;if(!Array.isArray(ids)||!ids.length||ids.length>100||ids.some(x=>typeof x!=='string'||x.length>180))fail('UNSUPPORTED','Playlist entry IDs are invalid.');name='removeFromPlaylist';variables={playlistUri:`spotify:playlist:${id}`,uids:ids};}else{const ids=args.entryIds;if(!Array.isArray(ids)||!ids.length||ids.length>100||ids.some(x=>typeof x!=='string'||x.length>180))fail('UNSUPPORTED','Playlist entry IDs are invalid.');if(args.beforeEntryId!=null&&(typeof args.beforeEntryId!=='string'||args.beforeEntryId.length>180))fail('UNSUPPORTED','The destination entry ID is invalid.');name='moveItemsInPlaylist';variables={playlistUri:`spotify:playlist:${id}`,uids:ids,newPosition:args.beforeEntryId?{moveType:'BEFORE_UID',fromUid:args.beforeEntryId}:{moveType:'BOTTOM_OF_PLAYLIST',fromUid:null}};}await gql(name,variables,auth,hashes);return {accountId:me.accountId,receipt:{provider:'spotify',operation,playlistId:id,submittedAt:new Date().toISOString()}};}
    fail('UNSUPPORTED',`Unsupported Spotify operation: ${operation}`);
  };
  const youtube=async()=>{
    const cfg=globalThis.ytcfg?.data_||{};const read=(key)=>globalThis.ytcfg?.get?.(key)||cfg[key];
    const clientName=read('INNERTUBE_CLIENT_NAME')||'WEB';const clientVersion=read('INNERTUBE_CLIENT_VERSION')||'';const key=read('INNERTUBE_API_KEY');
    const sapisid=(document.cookie.match(/(?:^|;\s*)(?:__Secure-1PAPISID|__Secure-3PAPISID|SAPISID)=([^;]+)/)||[])[1];
    const accountData=()=>{const account=document.querySelector('ytd-topbar-menu-button-renderer #avatar-btn')||document.querySelector('ytmusic-nav-bar #avatar');const name=first(account?.getAttribute('aria-label'),document.querySelector('#account-name')?.textContent,document.querySelector('ytd-active-account-header-renderer #account-name')?.textContent);const href=document.querySelector('ytd-active-account-header-renderer a[href*="/channel/"],ytd-active-account-header-renderer a[href^="/@"]')?.getAttribute('href')||'',studioHref=document.querySelector('a[href*="studio.youtube.com/channel/"]')?.getAttribute('href')||'';const dataSync=String(read('DATASYNC_ID')||'').split('||')[0];const id=first(read('DELEGATED_SESSION_ID'),read('CHANNEL_ID'),read('ACCOUNT_ID'),studioHref.match(/\/channel\/([^/?]+)/)?.[1],href.match(/\/channel\/([^/?]+)/)?.[1],href.match(/^\/@([^/?]+)/)?.[1],dataSync);const loggedIn=!!id&&(!!sapisid||read('LOGGED_IN')===true||read('LOGGED_IN')==='true');return {accountId:id||null,displayName:name||id||null,isSignedIn:loggedIn};};
    const queryString=key?`?key=${encodeURIComponent(key)}&prettyPrint=false`:'?prettyPrint=false';
    const headers={'Content-Type':'application/json','X-YouTube-Client-Name':String(clientName),'X-YouTube-Client-Version':String(clientVersion),'X-Goog-AuthUser':String(read('SESSION_INDEX')||'0')};
    const visitor=read('VISITOR_DATA');if(visitor)headers['X-Goog-Visitor-Id']=String(visitor);
    if(sapisid){const ts=Math.floor(Date.now()/1000),digest=await crypto.subtle.digest('SHA-1',new TextEncoder().encode(`${ts} ${decodeURIComponent(sapisid)} ${location.origin}`));headers.Authorization=`SAPISIDHASH ${ts}_${[...new Uint8Array(digest)].map(x=>x.toString(16).padStart(2,'0')).join('')}`;}
    const endpoint=operation==='search'?'search':'browse';
    const call=async(body,apiEndpoint=endpoint)=>{const r=await fetchBounded(`/youtubei/v1/${apiEndpoint}${queryString}`,{method:'POST',credentials:'include',headers,body:JSON.stringify({context:{client:{clientName,clientVersion,hl:read('HL')||'en',gl:read('GL')||'US',visitorData:read('VISITOR_DATA')},user:{lockedSafetyMode:false}},...body})});return json(r);};
    const textOf=x=>x?.runs?.map(r=>r.text).join('')||x?.simpleText||'';
    const viewText=x=>typeof x==='string'?x:first(textOf(x),x?.content,x?.dynamicTextViewModel?.text?.content,x?.text?.content,x?.accessibility?.accessibilityData?.label)||'';
    const parseInitialData=html=>{for(const match of String(html).matchAll(/ytInitialData\s*=\s*/g)){let start=match.index+match[0].length;while(/\s/.test(html[start]||''))start++;if(html[start]!=='{')continue;let depth=0,inString=false,escaped=false;for(let i=start;i<html.length;i++){const ch=html[i];if(inString){if(escaped)escaped=false;else if(ch==='\\')escaped=true;else if(ch==='"')inString=false;continue;}if(ch==='"'){inString=true;continue;}if(ch==='{')depth++;else if(ch==='}'&&!--depth){try{return JSON.parse(html.slice(start,i+1));}catch{break;}}}}return null;};
    const durationOf=value=>{const n=String(value||'').split(':').reduce((total,part)=>total*60+(Number(part)||0),0);return n||null;};
    const decode=(root)=>{const out=[];const lockupText=value=>typeof value==='string'?value:first(value?.content,value?.text,value?.simpleText,value?.runs?.map(run=>run.text).join(''))||'';const walk=(v)=>{if(!v||typeof v!=='object')return;if(v.lockupViewModel){const x=v.lockupViewModel,metadata=x.metadata?.lockupMetadataViewModel||x.lockupMetadataViewModel||{},rows=metadata.metadata?.contentMetadataViewModel?.metadataRows||metadata.metadataRows||[],subtitle=rows.map(row=>(row.metadataParts||[]).map(part=>lockupText(part.text||part)).filter(Boolean).join('')).filter(Boolean).join(' • '),command=x.rendererContext?.commandContext?.onTap?.innertubeCommand||{},endpoint=command.browseEndpoint||command.watchEndpoint||{},rawId=first(x.contentId,endpoint.browseId,endpoint.playlistId),contentType=String(x.contentType||'').toLowerCase(),isChannel=contentType.includes('channel')||String(rawId||'').startsWith('UC'),isPlaylist=contentType.includes('playlist')||String(rawId||'').startsWith('PL')||String(rawId||'').startsWith('VL'),title=lockupText(metadata.title)||lockupText(x.title),artworkUrl=images(x.contentImage||x.thumbnail)||null;if(rawId&&title&&isChannel)out.push({id:rawId,kind:'channel',title,subtitle,url:`https://www.youtube.com/channel/${rawId}`,artworkUrl,data:{provider:'youtube',channelId:rawId}});else if(rawId&&title&&isPlaylist){const id=String(rawId).replace(/^VL/,'');out.push({id,kind:'playlist',title,subtitle,url:`https://www.youtube.com/playlist?list=${id}`,artworkUrl,data:{provider:'youtube',playlistId:id,total:null}});}}if(v.videoRenderer){const x=v.videoRenderer,id=x.videoId,title=textOf(x.title),artist=textOf(x.ownerText||x.shortBylineText),durationSeconds=durationOf(x.lengthText?.simpleText),url=`https://www.youtube.com/watch?v=${id}`,artworkUrl=x.thumbnail?.thumbnails?.slice(-1)[0]?.url||null;if(id)out.push({id,kind:'track',title,subtitle:artist,url,artworkUrl,data:{id,provider:'youtube',trackId:id,title,artist,album:'',durationSeconds,url,artworkUrl,available:true}});}if(v.channelRenderer){const x=v.channelRenderer,id=x.channelId;if(id)out.push({id,kind:'channel',title:textOf(x.title),subtitle:textOf(x.subscriberCountText),url:`https://www.youtube.com/channel/${id}`,artworkUrl:x.thumbnail?.thumbnails?.slice(-1)[0]?.url||null,data:{provider:'youtube',channelId:id}});}if(v.musicResponsiveListItemRenderer){const x=v.musicResponsiveListItemRenderer,id=x.playlistItemData?.videoId||x.navigationEndpoint?.watchEndpoint?.videoId||x.flexColumns?.[0]?.musicResponsiveListItemFlexColumnRenderer?.text?.runs?.[0]?.navigationEndpoint?.watchEndpoint?.videoId;const browseId=x.navigationEndpoint?.browseEndpoint?.browseId||x.flexColumns?.[0]?.musicResponsiveListItemFlexColumnRenderer?.text?.runs?.[0]?.navigationEndpoint?.browseEndpoint?.browseId;const columns=(x.flexColumns||[]).map(c=>textOf(c.musicResponsiveListItemFlexColumnRenderer?.text));const title=columns[0]||'';const artist=columns.slice(1).join(' • ');const durationSeconds=durationOf(x.fixedColumns?.[0]?.musicResponsiveListItemFlexColumnRenderer?.text);const url=id?`https://www.youtube.com/watch?v=${id}`:null;const artworkUrl=images(x.thumbnail);if(id)out.push({id,kind:'track',title,subtitle:artist,url,artworkUrl,data:{id,provider:'youtube',trackId:id,title,artist,album:'',durationSeconds,url,artworkUrl,available:true}});else if(browseId?.startsWith('UC'))out.push({id:browseId,kind:'channel',title,subtitle:artist,url:`https://www.youtube.com/channel/${browseId}`,artworkUrl,data:{provider:'youtube',channelId:browseId}});}if(v.playlistRenderer||v.gridPlaylistRenderer){const x=v.playlistRenderer||v.gridPlaylistRenderer,id=x.playlistId||x.navigationEndpoint?.watchEndpoint?.playlistId;if(id)out.push({id,kind:'playlist',title:textOf(x.title),subtitle:textOf(x.shortBylineText),url:`https://www.youtube.com/playlist?list=${id}`,artworkUrl:x.thumbnails?.[0]?.thumbnails?.slice(-1)[0]?.url||null,data:{provider:'youtube',playlistId:id,total:Number(x.videoCount)||null}});}if(v.musicTwoRowItemRenderer){const x=v.musicTwoRowItemRenderer,nav=x.navigationEndpoint||x.title?.runs?.[0]?.navigationEndpoint,id=nav?.browseEndpoint?.browseId||nav?.watchEndpoint?.playlistId;if(id&&String(id).startsWith('UC'))out.push({id,kind:'channel',title:textOf(x.title),subtitle:textOf(x.subtitle),url:`https://www.youtube.com/channel/${id}`,artworkUrl:images(x.thumbnailRenderer?.musicThumbnailRenderer?.thumbnail),data:{provider:'youtube',channelId:id}});else if(id&&(String(id).startsWith('VL')||String(id).startsWith('PL'))){const playlistId=String(id).replace(/^VL/,'');out.push({id:playlistId,kind:'playlist',title:textOf(x.title),subtitle:textOf(x.subtitle),url:`https://www.youtube.com/playlist?list=${playlistId}`,artworkUrl:images(x.thumbnailRenderer?.musicThumbnailRenderer?.thumbnail),data:{provider:'youtube',playlistId,total:null}});}}for(const k of Object.keys(v))walk(v[k]);};walk(root);const seen=new Set();return out.filter(x=>!seen.has(`${x.kind}:${x.id}`)&&(seen.add(`${x.kind}:${x.id}`),true));};
    const nextToken=root=>{let token=null;const walk=(value,inContinuation=false)=>{if(!value||typeof value!=='object'||token)return;if(value.nextContinuationData?.continuation){token=value.nextContinuationData.continuation;return;}if(inContinuation&&value.continuationCommand?.token){token=value.continuationCommand.token;return;}if(value.continuationItemRenderer?.continuationEndpoint?.continuationCommand?.token){token=value.continuationItemRenderer.continuationEndpoint.continuationCommand.token;return;}if(value.continuationItemViewModel?.continuationCommand?.innertubeCommand?.continuationCommand?.token){token=value.continuationItemViewModel.continuationCommand.innertubeCommand.continuationCommand.token;return;}for(const [key,child] of Object.entries(value))walk(child,inContinuation||/continuationItem(?:Renderer|ViewModel)/i.test(key));};walk(root);return token;};
    if(operation==='connect'){const account=accountData();if(!account.isSignedIn)fail('SIGN_IN_REQUIRED','YouTube is open. Sign in here, then retry status.');return {accountId:account.accountId,displayName:account.displayName};}
    if(operation==='status'){const account=accountData();if(!account.isSignedIn)fail('SIGN_IN_REQUIRED','Sign in to YouTube in the browser, then retry.');return {accountId:account.accountId,displayName:account.displayName};}
    if(operation==='search'){const q=String(args.query||'').trim();if(!q)fail('UNSUPPORTED','Search query is empty.');const kind=['track','playlist','channel'].includes(args.kind)?args.kind:'track';const body=args.cursor?await call({continuation:args.cursor}):await call({query:q,params:kind==='playlist'?'EgIQAw%3D%3D':kind==='channel'?'EgIQAg%3D%3D':undefined});const account=accountData(),decoded=decode(body),items=decoded.filter(x=>x.kind===kind).slice(0,Math.min(50,Number(args.limit)||20));if(!decoded.length){const renderers=new Set(),scan=value=>{if(!value||typeof value!=='object')return;for(const [key,child] of Object.entries(value)){if(/(?:Renderer|ViewModel)$/.test(key))renderers.add(key);scan(child);}};scan(body);if(renderers.size)fail('PROVIDER_CHANGED',`YouTube search used unsupported renderers: ${[...renderers].slice(0,20).join(', ')}.`);}return {accountId:account.accountId,items,nextCursor:nextToken(body)};}
    if(operation==='listPlaylists'){
      if(!args.ownerId&&!sapisid)fail('SIGN_IN_REQUIRED','Sign in to YouTube Music in the browser to open your playlist library.');
      let body;if(args.cursor){body=await call({continuation:args.cursor});}else if(args.ownerId){let owner=String(args.ownerId);if(/^https?:\/\//i.test(owner)){let parsed;try{parsed=new URL(owner);}catch{fail('UNSUPPORTED','YouTube playlist owner URL is invalid.');}if(!['www.youtube.com','music.youtube.com'].includes(parsed.hostname)||parsed.protocol!=='https:')fail('UNSUPPORTED','Only YouTube channel URLs are supported.');const path=parsed.pathname.replace(/\/+$/,'').replace(/\/playlists$/,'');const handle=path.match(/^\/@([A-Za-z0-9._-]+)$/),channel=path.match(/^\/channel\/(UC[A-Za-z0-9_-]+)$/);owner=handle?`@${handle[1]}`:channel?.[1]||'';if(!owner)fail('UNSUPPORTED','YouTube owner URL must identify a channel.');}if(owner.startsWith('@')){const resolved=await call({url:`https://www.youtube.com/${owner}`},'navigation/resolve_url');owner=resolved.endpoint?.browseEndpoint?.browseId||resolved.browseEndpoint?.browseId||'';if(!owner)fail('UNSUPPORTED','YouTube could not resolve that public channel handle.');}body=await call({browseId:owner.startsWith('UC')?owner:`UC${owner}`});}else if(location.hostname==='www.youtube.com'&&location.pathname==='/feed/playlists'&&globalThis.ytInitialData){body=globalThis.ytInitialData;}else{const response=await fetchBounded('/feed/playlists',{credentials:'include',headers:{Accept:'text/html'}});if(!response.ok)fail('PROVIDER_CHANGED','YouTube playlist library page could not be loaded.');body=parseInitialData(await response.text());if(!body)fail('PROVIDER_CHANGED','YouTube playlist library page was not recognized.');}
      const account=accountData(),listRoot=!args.ownerId&&!args.cursor?(body.contents?.twoColumnBrowseResultsRenderer?.tabs?.find(tab=>tab.tabRenderer?.selected)?.tabRenderer?.content||body.contents||body):body;const items=decode(listRoot).filter(x=>x.kind==='playlist').map(x=>({id:x.id,provider:'youtube',title:x.title||'Untitled playlist',url:x.url,ownerId:args.ownerId||null,ownerName:x.subtitle||null,artworkUrl:x.artworkUrl,total:x.data.total,canEdit:!args.ownerId}));return {accountId:account.accountId,items,nextCursor:nextToken(listRoot)};
    }
    if(operation==='getPlaylist'){
      const id=String(args.playlistId||'');if(!/^[A-Za-z0-9_-]{10,80}$/.test(id))fail('UNSUPPORTED','YouTube playlist ID is invalid.');let body;if(args.cursor)body=await call({continuation:args.cursor});else body=await call({browseId:`VL${id}`,params:'wgYCCAA='});const entries=[];const walk=(v)=>{if(!v||typeof v!=='object')return;if(v.lockupViewModel){const x=v.lockupViewModel,metadata=x.metadata?.lockupMetadataViewModel||x.lockupMetadataViewModel||{},contentType=String(x.contentType||'').toLowerCase(),command=x.rendererContext?.commandContext?.onTap?.innertubeCommand||{},endpoint=command.watchEndpoint||{},trackId=first(x.contentId,endpoint.videoId),entryId=first(endpoint.playlistSetVideoId,endpoint.playlistItemId,x.rendererContext?.commandContext?.onTap?.innertubeCommand?.watchEndpoint?.playlistSetVideoId)||null,title=viewText(metadata.title)||viewText(x.title),rows=metadata.metadata?.contentMetadataViewModel?.metadataRows||metadata.metadataRows||[],parts=rows.flatMap(row=>(row.metadataParts||[]).map(part=>viewText(part.text||part)).filter(Boolean)),artistParts=(rows[0]?.metadataParts||[]).map(part=>viewText(part.text||part)).filter(Boolean),durationText=parts.find(part=>/^\d{1,2}:\d{2}(?::\d{2})?$/.test(part))||'',artworkUrl=images(x.contentImage)||null;if(contentType.includes('video')&&(trackId||entryId))entries.push({entryId,trackId:trackId||null,title:title||'Unavailable video',artist:artistParts.filter(part=>part!==durationText).join(' • '),album:'',durationSeconds:durationOf(durationText),url:trackId?`https://www.youtube.com/watch?v=${trackId}`:null,artworkUrl,available:!!trackId&&!contentType.includes('unavailable')});}if(v.playlistVideoRenderer){const x=v.playlistVideoRenderer,trackId=x.videoId||null,entryId=x.setVideoId||x.playlistVideoId||x.playlistItemData?.playlistSetVideoId||null,available=!!trackId&&x.isPlayable!==false&&!x.unplayableText;if(trackId||entryId||x.isPlayable===false||x.unplayableText)entries.push({entryId,trackId,title:textOf(x.title)||textOf(x.unplayableText)||'Unavailable video',artist:textOf(x.shortBylineText),album:'',durationSeconds:durationOf(x.lengthText?.simpleText),url:trackId?`https://www.youtube.com/watch?v=${trackId}`:null,artworkUrl:x.thumbnail?.thumbnails?.slice(-1)[0]?.url||null,available});}if(v.musicResponsiveListItemRenderer){const x=v.musicResponsiveListItemRenderer,trackId=x.playlistItemData?.videoId||x.navigationEndpoint?.watchEndpoint?.videoId,columns=(x.flexColumns||[]).map(c=>textOf(c.musicResponsiveListItemFlexColumnRenderer?.text));if(trackId)entries.push({entryId:x.playlistItemData?.setVideoId||null,trackId,title:columns[0]||'',artist:columns.slice(1).join(' • '),album:'',durationSeconds:durationOf(textOf(x.fixedColumns?.[0]?.musicResponsiveListItemFlexColumnRenderer?.text)),url:`https://www.youtube.com/watch?v=${trackId}`,artworkUrl:images(x.thumbnail),available:true});}for(const k of Object.keys(v))walk(v[k]);};walk(body);const header=body.header||{},metadata=header.musicDetailHeaderRenderer||header.playlistHeaderRenderer||{},pageHeader=header.pageHeaderRenderer||body.pageHeaderRenderer||{},pageView=body.contents?.pageHeaderViewModel||header.pageHeaderViewModel||header.pageHeaderRenderer?.content?.pageHeaderViewModel||body.pageHeaderRenderer?.content?.pageHeaderViewModel||{},playlistMetadata=body.metadata?.playlistMetadataRenderer||body.playlistMetadataRenderer||{},title=first(viewText(metadata.title),viewText(pageView.title),viewText(pageHeader.pageTitle),viewText(playlistMetadata.title)),subtitle=first(viewText(metadata.strapline),viewText(pageHeader.subtitle),viewText(pageView.metadata),viewText(playlistMetadata.description))||'',total=Number(viewText(metadata.secondSubtitle).replace(/\D/g,''))||null;const next=nextToken(body),declaredTotal=Number(viewText(metadata.secondSubtitle).replace(/\D/g,''))||Number(viewText(pageView.metadata).replace(/\D/g,''))||Number(playlistMetadata.videoCount)||Number(playlistMetadata.trackCount)||null;const account=accountData();return {accountId:account.accountId,playlist:{id,provider:'youtube',title:title||'',url:`https://www.youtube.com/playlist?list=${id}`,ownerId:null,ownerName:subtitle||null,artworkUrl:images(metadata.thumbnail||pageView.image||pageView.contentImage||pageView.heroImage||playlistMetadata.thumbnail)||null,total:declaredTotal,canEdit:false},entries,nextCursor:next,total:declaredTotal,revision:null,complete:!next&&(!declaredTotal||entries.length>=declaredTotal)};
    }
    fail('UNSUPPORTED',`Unsupported YouTube operation: ${operation}`);
  };
  try{
    if(provider==='spotify'){if(!location.hostname.endsWith('spotify.com'))fail('PROVIDER_CHANGED','Spotify is not open in this tab.');return await spotify();}
    if(provider==='youtube'){if(!['www.youtube.com','music.youtube.com'].includes(location.hostname))fail('PROVIDER_CHANGED','YouTube is not open in this tab.');return await youtube();}
    fail('UNSUPPORTED',`Unsupported provider: ${provider}`);
  }catch(error){return {__burrataProviderError:{code:error?.code||'PROVIDER_CHANGED',message:String(error?.message||'Provider request failed.')}};}
}
const providerHost=provider=>provider==='spotify'?'open.spotify.com':provider==='youtube'?'www.youtube.com':null;
const providerRoot=provider=>provider==='spotify'?'https://open.spotify.com/':provider==='youtube'?'https://www.youtube.com/':null;
const providerMutation=operation=>['addTracks','removeTracks','moveTracks'].includes(operation);
async function refreshSpotifyHashes(force=false){
  if(!force&&cachedSpotifyHashes.__loadedAt&&Date.now()-cachedSpotifyHashes.__loadedAt<12*60*60*1000)return cachedSpotifyHashes;
  try{
    const response=await fetchBounded(HASH_REGISTRY,{headers:{Accept:'application/json'},cache:'no-store'});
    if(!response.ok)throw new Error('Hash registry unavailable');
    const body=await response.json(),remote=body.operations||{};
    const accepted={...cachedSpotifyHashes};
    for(const name of Object.keys(SPOTIFY_HASHES)){
      const value=remote[name];if(!value||typeof value!=='object')continue;
      const hash=typeof value.hash==='string'&&/^[a-f0-9]{64}$/i.test(value.hash)?value.hash:null;
      const previous=typeof value.previous_hash==='string'&&/^[a-f0-9]{64}$/i.test(value.previous_hash)?value.previous_hash:null;
      if(hash)accepted[name]={hash,previous_hash:previous,fallback:SPOTIFY_HASHES[name]};
    }
    accepted.__loadedAt=Date.now();cachedSpotifyHashes=accepted;await api.storage.local.set({spotifyHashes:accepted});
  }catch{/* Built-in hashes remain available while the remote data-only registry is offline. */}
  return cachedSpotifyHashes;
}
async function providerTab(provider,connectRequest,preferYoutubeWeb=false){
  const host=provider==='youtube'&&preferYoutubeWeb?'www.youtube.com':providerHost(provider),origin=`https://${host}/*`;
  if(!host||!await api.permissions.contains({origins:[origin]}))throw errorResult('BROWSER_REQUIRED',`Enable ${provider} permission in the Burrata extension, then retry.`);
  const tabs=await api.tabs.query({});
  let tab=tabs.find(x=>{try{return new URL(x.url).hostname===host;}catch{return false;}});
  if(!tab&&provider==='youtube'&&!preferYoutubeWeb)tab=tabs.find(x=>{try{return new URL(x.url).hostname==='www.youtube.com';}catch{return false;}});
  if(!tab){const root=provider==='youtube'&&preferYoutubeWeb?'https://www.youtube.com/':providerRoot(provider);tab=await api.tabs.create({url:root,active:!!connectRequest});await waitLoaded(tab.id);tab=await api.tabs.get(tab.id);}
  if(connectRequest){await api.tabs.update(tab.id,{active:true});const current=await api.tabs.get(tab.id);if(current.windowId)await api.windows.update(current.windowId,{focused:true});}
  const url=new URL(tab.url);if(url.protocol!=='https:'||![host,'www.youtube.com'].includes(url.hostname))throw errorResult('BROWSER_REQUIRED','Open the provider in a supported browser tab, then retry.');
  return tab;
}
async function executeProviderRequest(request){
  const {provider,operation,args={}}=request;
  if(!['spotify','youtube'].includes(provider))throw errorResult('UNSUPPORTED','This provider is not supported.');
  if(!['connect','status','search','listPlaylists','getPlaylist','getTrack','getArtist','getArtistAlbums','getArtistTracks','getAlbum','getAlbumTracks','addTracks','removeTracks','moveTracks'].includes(operation))throw errorResult('UNSUPPORTED',`Unsupported provider operation: ${operation}`);
  if(provider!=='spotify'&&['getTrack','getArtist','getArtistAlbums','getArtistTracks','getAlbum','getAlbumTracks'].includes(operation))throw errorResult('UNSUPPORTED','Spotify catalog browsing is not supported for this provider.');
  if(provider==='youtube'&&providerMutation(operation))throw errorResult('UNSUPPORTED','YouTube playlist writeback is not supported.');
  const tab=await providerTab(provider,operation==='connect',provider==='youtube'&&operation==='listPlaylists'&&!args.ownerId);
  const stored=provider==='spotify'?await refreshSpotifyHashes():{};
  const hashes={...Object.fromEntries(Object.entries(SPOTIFY_HASHES).map(([name,hash])=>[name,{hash,fallback:hash}])),...stored};
  if(provider==='spotify'){
    const nuance=await globalThis.BurrataSpotifyAuth.loadRegistry(api.storage.local,fetchBounded);
    const timeResult=await api.scripting.executeScript({target:{tabId:tab.id},world:'MAIN',func:getSpotifyServerTimeMain});
    const time=timeResult?.[0]?.result;
    if(time?.__burrataProviderError)throw errorResult(time.__burrataProviderError.code,time.__burrataProviderError.message);
    hashes.totp={code:await globalThis.BurrataSpotifyAuth.totp(nuance.secret,time?.serverTime),version:nuance.version};
  }
  const invoke=async(hashMap)=>{
    let result;
    if(provider==='spotify'){
      await api.scripting.executeScript({target:{tabId:tab.id},world:'MAIN',files:['provider-operations.js']});
      result=await api.scripting.executeScript({target:{tabId:tab.id},world:'MAIN',func:(providerName,operationName,operationArgs,hashes)=>globalThis.BurrataSpotifyProvider.run(providerName,operationName,operationArgs,hashes),args:[provider,operation,args,hashMap]});
    }else result=await api.scripting.executeScript({target:{tabId:tab.id},world:'MAIN',func:runProviderPageOperation,args:[provider,operation,args,hashMap]});
    const value=result?.[0]?.result;
    if(value?.__burrataProviderError)throw errorResult(value.__burrataProviderError.code,value.__burrataProviderError.message);
    return value;
  };
  try{return await invoke(hashes);}catch(error){
    if(provider==='spotify'&&error?.code==='HASH_EXPIRED'){
      const refreshed=await refreshSpotifyHashes(true);
      const retryHashes={...Object.fromEntries(Object.entries(SPOTIFY_HASHES).map(([name,hash])=>[name,{hash,fallback:hash}])),...refreshed};
      retryHashes.totp=hashes.totp;
      return invoke(retryHashes);
    }
    throw error;
  }
}
async function persistProviderOperations(){
  await api.storage.session.set({providerOperations:[...providerOperations].slice(-200)});
  await api.storage.local.set({providerMutationOperations:[...providerOperations].filter(([,record])=>record.mutation).slice(-200)});
}
function normalizedProviderError(error){
  const allowed=['SIGN_IN_REQUIRED','VERIFICATION_REQUIRED','BROWSER_REQUIRED','UNSUPPORTED','PROVIDER_CHANGED','RATE_LIMITED','REQUEST_TIMEOUT','API_DENIED','TRACK_NOT_FOUND','MUTATION_OUTCOME_UNKNOWN'];
  const code=allowed.includes(error?.code)?error.code:'PROVIDER_CHANGED';
  return {code,message:String(error?.message||'Provider request failed.').slice(0,300)};
}
function captureYouTubeAccountIdentity(){
  const origin=location.origin;
  if(!['https://www.youtube.com','https://music.youtube.com'].includes(origin))return {error:'Open a signed-in YouTube or YouTube Music tab, then retry.'};
  const cfg=globalThis.ytcfg?.data_||{},read=key=>globalThis.ytcfg?.get?.(key)||cfg[key];
  const getText=node=>node?.getAttribute?.('aria-label')||node?.textContent?.trim()||'';
  const activeLink=document.querySelector('ytd-active-account-header-renderer a[href*="/channel/"],ytd-active-account-header-renderer a[href^="/@"]');
  const studioHref=document.querySelector('a[href*="studio.youtube.com/channel/"]')?.getAttribute('href')||'';
  const href=activeLink?.getAttribute('href')||'';
  const channelId=[read('CHANNEL_ID'),studioHref.match(/\/channel\/([^/?]+)/)?.[1],href.match(/\/channel\/([^/?]+)/)?.[1]].find(value=>typeof value==='string'&&/^UC[A-Za-z0-9_-]{20,30}$/.test(value));
  const handle=href.match(/^\/@([A-Za-z0-9._-]{3,100})(?:\/|$)/)?.[1];
  const accountId=channelId||(handle?`@${handle}`:null);
  const rawName=getText(document.querySelector('ytd-active-account-header-renderer #account-name'))||getText(document.querySelector('#account-name'))||'';
  const displayName=/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(rawName)?handle||null:rawName||handle||null;
  if(!accountId||!displayName)return {error:'Select a YouTube channel account first, then retry the session handoff.'};
  return {account:{accountId,displayName:String(displayName).slice(0,120)}};
}
function captureYouTubeSessionHandoff(expectedAccountId){
  const origin=location.origin;
  if(!['https://www.youtube.com','https://music.youtube.com'].includes(origin))return {error:'Open a signed-in YouTube or YouTube Music tab, then retry.'};
  const cfg=globalThis.ytcfg?.data_||{},read=key=>globalThis.ytcfg?.get?.(key)||cfg[key];
  const getText=node=>node?.getAttribute?.('aria-label')||node?.textContent?.trim()||'';
  const activeLink=document.querySelector('ytd-active-account-header-renderer a[href*="/channel/"],ytd-active-account-header-renderer a[href^="/@"]');
  const studioHref=document.querySelector('a[href*="studio.youtube.com/channel/"]')?.getAttribute('href')||'';
  const href=activeLink?.getAttribute('href')||'';
  const channelId=[read('CHANNEL_ID'),studioHref.match(/\/channel\/([^/?]+)/)?.[1],href.match(/\/channel\/([^/?]+)/)?.[1]].find(value=>typeof value==='string'&&/^UC[A-Za-z0-9_-]{20,30}$/.test(value));
  const handle=href.match(/^\/@([A-Za-z0-9._-]{3,100})(?:\/|$)/)?.[1];
  const accountId=channelId||(handle?`@${handle}`:null);
  const rawName=getText(document.querySelector('ytd-active-account-header-renderer #account-name'))||getText(document.querySelector('#account-name'))||'';
  const displayName=/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(rawName)?handle||null:rawName||handle||null;
  if(!accountId||!displayName)return {error:'Select a YouTube channel account first, then retry the session handoff.'};
  if(accountId!==expectedAccountId)return {error:'The active YouTube account changed. Confirm the account again before sending.'};
  // Verify the user's second confirmation against the active account before touching cookies.
  const cookieSource=document.cookie;
  let cookieName=null,cookieValue=null;
  for(const name of ['__Secure-1PAPISID','__Secure-3PAPISID','SAPISID']){
    const match=new RegExp(`(?:^|;\\s*)${name}=([^;]+)`).exec(cookieSource);
    if(match){cookieName=name;cookieValue=match[1].trim();break;}
  }
  if(!cookieName||!cookieValue)return {error:'This YouTube tab does not expose a signing cookie for the session handoff. Reopen YouTube and retry.'};
  const clientName=read('INNERTUBE_CLIENT_NAME')||'WEB',clientVersion=read('INNERTUBE_CLIENT_VERSION')||'',apiKey=read('INNERTUBE_API_KEY')||'',authUser=String(read('SESSION_INDEX')||'0');
  if(!/^[A-Z0-9_]{2,32}$/.test(String(clientName))||!/^[A-Za-z0-9._-]{1,64}$/.test(String(clientVersion))||!/^\d{1,3}$/.test(authUser))return {error:'YouTube client context is incomplete. Refresh the signed-in YouTube tab and retry.'};
  return {session:{origin,cookieHeader:`${cookieName}=${cookieValue}`,accountId,displayName:String(displayName).slice(0,120),clientName:String(clientName),clientVersion:String(clientVersion),apiKey:String(apiKey),visitorData:String(read('VISITOR_DATA')||''),authUser,hl:String(read('HL')||'en'),gl:String(read('GL')||'US')}};
}
function parseYouTubePairingCode(value){
  if(typeof value!=='string')return null;
  const match=/^([1-9]\d{0,4})-([a-f0-9]{32})$/.exec(value.trim());
  if(!match)return null;
  const port=Number(match[1]);
  return Number.isInteger(port)&&port>=1&&port<=65535?`${port}-${match[2]}`:null;
}
async function activeYouTubeTab(){
  const [tab]=await api.tabs.query({active:true,currentWindow:true});
  if(!tab?.id)throw errorResult('BROWSER_REQUIRED','Open a signed-in YouTube tab, then retry.');
  let origin;
  try{const url=new URL(tab.url);if(url.protocol!=='https:'||!['www.youtube.com','music.youtube.com'].includes(url.hostname))throw new Error();origin=url.origin;}catch{throw errorResult('BROWSER_REQUIRED','Open a signed-in YouTube or YouTube Music tab, then retry.');}
  if(!await api.permissions.contains({origins:[`${origin}/*`]}))throw errorResult('BROWSER_REQUIRED','Enable the YouTube site permission, then retry the session handoff.');
  return {tab,origin};
}
async function confirmYouTubeHandoffAccount(pairingCode){
  const pairing=parseYouTubePairingCode(pairingCode);
  if(!pairing)throw errorResult('UNSUPPORTED','Enter the current Burrata pairing code in the form shown by the app.');
  const {tab}=await activeYouTubeTab();
  const captured=await api.scripting.executeScript({target:{tabId:tab.id},world:'MAIN',func:captureYouTubeAccountIdentity});
  const value=captured?.[0]?.result;
  if(!value?.account?.accountId||!value.account.displayName)throw errorResult('SIGN_IN_REQUIRED',value?.error||'Could not verify the active YouTube account.');
  pendingYouTubeHandoff={pairingCode:pairing,tabId:tab.id,accountId:value.account.accountId,displayName:value.account.displayName,checkedAt:Date.now()};
  return {account:value.account};
}
async function handoffYouTubeSession(pairingCode,confirmedAccountId){
  const pairing=parseYouTubePairingCode(pairingCode),pending=pendingYouTubeHandoff;
  if(!pairing||!pending||pending.pairingCode!==pairing||pending.accountId!==confirmedAccountId||Date.now()-pending.checkedAt>120_000){
    pendingYouTubeHandoff=null;
    throw errorResult('UNSUPPORTED','Pairing or account confirmation expired. Recheck the account and enter the current Burrata code.');
  }
  // Treat approval as one-use even if extraction or transport fails.
  pendingYouTubeHandoff=null;
  const {tab}=await activeYouTubeTab();
  if(tab.id!==pending.tabId)throw errorResult('ACCOUNT_CHANGED','The active tab changed. Confirm its YouTube account again.');
  const captured=await api.scripting.executeScript({target:{tabId:tab.id},world:'MAIN',func:captureYouTubeSessionHandoff,args:[pending.accountId]});
  const value=captured?.[0]?.result;
  if(!value||value.error||!value.session)throw errorResult('SIGN_IN_REQUIRED',value?.error||'Could not prepare the YouTube session handoff.');
  if(value.session.accountId!==pending.accountId)throw errorResult('ACCOUNT_CHANGED','The active YouTube account changed. Confirm the account again before sending.');
  const requestId=`youtube-session-${crypto.randomUUID()}`;
  const reply=await native({type:'youtubeSessionHandoff',requestId,pairingCode:pairing,session:value.session});
  if(!reply?.ok)throw errorResult(reply?.error?.code||'SIGN_IN_REQUIRED',reply?.error?.message||'Burrata could not verify the YouTube session.');
  return {account:reply.account||{displayName:value.session.displayName}};
}
async function finishProviderRequest(request,result,error){
  const response={type:'providerResult',requestId:request.id,operationId:request.operationId,ok:!error,...(managedSessionId?{managedSessionId}:{}),...(error?{error:normalizedProviderError(error)}:{result})};
  const record={state:'done',mutation:providerMutation(request.operation),response};
  providerOperations.set(request.operationId,record);await persistProviderOperations();
  await native(response);
}
async function handleProviderRequest(request){
  const operationId=String(request.operationId||'');if(!operationId||operationId.length>200){await native({type:'providerResult',requestId:request.id,operationId:request.operationId,ok:false,error:{code:'UNSUPPORTED',message:'Provider operation ID is invalid.'}});return;}
  let record=providerOperations.get(operationId);
  if(record?.state==='done'){await native(record.response);return;}
  if(record?.state==='running'&&record.mutation){await finishProviderRequest(request,null,errorResult('MUTATION_OUTCOME_UNKNOWN','The previous write may have reached the provider. Refresh this playlist before retrying.'));return;}
  if(record?.state==='running')return;
  record={state:'running',mutation:providerMutation(request.operation)};providerOperations.set(operationId,record);await persistProviderOperations();
  let result,error;
  try{result=await executeProviderRequest(request);const text=JSON.stringify(result);if(new TextEncoder().encode(text).length>1024*1024)throw errorResult('PROVIDER_CHANGED','Provider page exceeded the 1 MiB bridge limit; request a smaller page.');}
  catch(cause){error=cause;}
  await finishProviderRequest(request,result,error);
}
async function rememberRequest(tabId,requestId){requestTabs.set(tabId,requestId);await api.storage.session.set({requestTabs:[...requestTabs]});}
async function forgetRequest(tabId){requestTabs.delete(tabId);await api.storage.session.set({requestTabs:[...requestTabs]});}
function badge(text){api.action.setBadgeText({text});api.action.setBadgeBackgroundColor({color:'#bd5734'});}
function disconnect(error){port=null;lastError=error;badge(error?'!':'');for(const w of waiters.splice(0)){clearTimeout(w.timeout);w.reject(new Error(error));}}
function connect(){
  if(port)return;
  port=api.runtime.connectNative('org.burrata.browser');
  port.onMessage.addListener(reply=>{const w=waiters.shift();if(w){clearTimeout(w.timeout);reply.ok?w.resolve(reply):w.reject(new Error(reply.error||'Native host failed'));}});
  port.onDisconnect.addListener(()=>disconnect(!enabled&&!waiters.length?'':api.runtime.lastError?.message||'Native host disconnected. Open Burrata and install its browser host.'));
}
function native(message){
  const payload=message.type==='poll'?{...message,providerProtocolVersion:2,...(managedSessionId?{managedSessionId}:{})}:message.type==='heartbeat'?{...message,providerProtocolVersion:enabled?2:0,...(managedSessionId?{managedSessionId}:{})}:message;
  const run=()=>new Promise((resolve,reject)=>{
      try{if(new TextEncoder().encode(JSON.stringify(payload)).length>1024*1024)throw new Error('Capture exceeds the 1 MiB bridge limit. Import a smaller chart, playlist, or album.');connect();const limit=payload.type==='importAlbumCollection'?120000:payload.type==='providerResult'?90000:payload.type==='youtubeSessionHandoff'?45000:30000;const w={resolve,reject,timeout:setTimeout(()=>{port?.disconnect();disconnect(`Burrata did not reply within ${Math.round(limit/1000)} seconds.`);},limit)};waiters.push(w);port.postMessage(payload);}catch(e){reject(e);}
  });
  const result=nativeBusy.then(run,run);nativeBusy=result.catch(()=>{});return result;
}
async function verification(tabId,requestId){
  await api.tabs.update(tabId,{active:true});const tab=await api.tabs.get(tabId);await api.windows.update(tab.windowId,{focused:true});
  if(requestId)await native({type:'status',requestId,state:'verification',message:'Complete provider login or verification in the browser, then click Import current page.'});
  throw new Error('Complete login or verification in this tab, then click Import current page.');
}
async function capture(tabId, requestId){
  const current=await api.tabs.get(tabId);
  if(!supported(current.url)||/\/(?:login|signin|authorize)(?:\/|$)/i.test(new URL(current.url).pathname))return verification(tabId,requestId);
  await api.scripting.executeScript({target:{tabId},files:['content.js']});
  let result;
  for(let i=0;i<5;i++){
    try{result=await api.tabs.sendMessage(tabId,{type:'capture'});break;}catch(e){
      if(i===4)throw new Error('The extension could not connect to this page. Reload the extension and the provider tab, then retry.');
      // Retry listener startup only. Content capture already owns its bounded loading wait.
      await new Promise(r=>setTimeout(r,250));
    }
  }
  if(result?.verification){
    return verification(tabId,requestId);
  }
  if(result?.error)throw new Error(result.error);
  let response;
  if(result?.albumCollection){
    const collection=result.albumCollection;
    response=await native({type:'importAlbumCollection',...(requestId?{requestId}:{}),sourceUrl:collection.sourceUrl,title:collection.title,albumUrls:collection.albumUrls});
  }else if(result?.snapshot){
    response=await native({type:'import',...(requestId?{requestId}:{}),snapshot:result.snapshot});
  }else throw new Error('Page did not return a chart, playlist, album, or Bandcamp album collection.');
  await forgetRequest(tabId);badge('✓');lastError='';return response.result;
}
async function waitLoaded(tabId){
  for(let i=0;i<40;i++){const t=await api.tabs.get(tabId);if(t.status==='complete'){await new Promise(r=>setTimeout(r,1200));return;}await new Promise(r=>setTimeout(r,500));}
  throw new Error('Provider page took too long to load.');
}
function supported(url){try{const u=new URL(url),host=u.hostname.toLowerCase();return u.protocol==='https:'&&(['www.beatport.com','beatport.com','www.traxsource.com','traxsource.com','open.spotify.com','www.youtube.com','music.youtube.com','www.shazam.com','shazam.com','www.last.fm','last.fm','bandcamp.com'].includes(host)||host.endsWith('.bandcamp.com'));}catch{return false;}}
async function poll(){
  await Promise.all([ready,managedSessionReady]);
  if(pollBusy)return;pollBusy=true;
  try{
    const reply=await native({type:enabled?'poll':'heartbeat'});
    if(!enabled)return;
    for(const request of reply.requests||[]){
      if(request.kind==='providerOperation'){
        try{await handleProviderRequest(request);}catch(error){lastError=error.message;badge('!');}
        continue;
      }
      if(!supported(request.url)){await native({type:'status',requestId:request.id,state:'failed',message:'Unsupported provider URL.'});continue;}
      if(!await api.permissions.contains({origins:[new URL(request.url).origin+'/*']})){await native({type:'status',requestId:request.id,state:'failed',message:'Enable provider permissions in the Burrata extension.'});continue;}
      let tab;
      try{
        await native({type:'status',requestId:request.id,state:'loading',message:'Loading chart in the browser.'});
        tab=await api.tabs.create({url:request.url,active:false});await rememberRequest(tab.id,request.id);
        await waitLoaded(tab.id);await capture(tab.id,request.id);
      }catch(e){
        lastError=e.message;badge('!');
        // Verification remains pending for a user-initiated capture.
        if(!/verification/i.test(e.message)) {if(tab)await forgetRequest(tab.id);await native({type:'status',requestId:request.id,state:'failed',message:e.message});}
      }
    }
  }catch(e){lastError=e.message;badge('!');}finally{pollBusy=false;}
}
api.runtime.onMessage.addListener((message,sender,respond)=>{
  const handle=async()=>{
    await ready;
    if(message.type==='youtubeCheckHandoffAccount'||message.type==='youtubeSessionHandoff'){
      if(sender?.tab||sender?.url!==api.runtime.getURL('popup.html'))throw errorResult('UNSUPPORTED','Start the session handoff from the Burrata extension popup.');
      if(message.type==='youtubeCheckHandoffAccount')return await confirmYouTubeHandoffAccount(message.pairingCode);
      return await handoffYouTubeSession(message.pairingCode,message.confirmedAccountId);
    }
    if(message.type==='status'){
      try{await native({type:'heartbeat'});lastError='';}catch(error){lastError=error.message;}
      return {enabled,error:lastError,connected:!!port};
    }
    if(message.type==='enable'){enabled=message.enabled;await api.storage.local.set({enabled});if(enabled)await poll();else port?.disconnect();return {enabled};}
    if(message.type==='diagnosticCurrent'){
      const [tab]=await api.tabs.query({active:true,currentWindow:true});
      if(!tab?.id||!supported(tab.url))throw new Error('Open a supported provider chart, playlist, or album page.');
      await api.scripting.executeScript({target:{tabId:tab.id},files:['content.js']});
      let result;
      for(let i=0;i<5;i++){
        try{result=await api.tabs.sendMessage(tab.id,{type:'diagnostic'});break;}catch(error){
          if(i===4)throw new Error('Could not read this page. Reload the extension and provider tab, then retry.');
          await new Promise(resolve=>setTimeout(resolve,250));
        }
      }
      if(result?.error)throw new Error(result.error);
      if(!result?.diagnostic)throw new Error('This page did not return a parser diagnostic.');
      // Diagnostic export is local to the popup. Never forward it to native messaging.
      return {diagnostic:result.diagnostic};
    }
    if(message.type==='importCurrent'){
      const [tab]=await api.tabs.query({active:true,currentWindow:true});if(!tab?.id||!supported(tab.url))throw new Error('Open a supported provider page.');
      return {result:await capture(tab.id,requestTabs.get(tab.id))};
    }
    throw new Error('Unknown action');
  };
  handle().then(r=>respond({ok:true,...r}),e=>{lastError=e.message;badge('!');respond({ok:false,error:e.message});});return true;
});
api.tabs.onRemoved.addListener(tabId=>{
  void ready.then(async()=>{
    const requestId=requestTabs.get(tabId);
    if(!requestId)return;
    await forgetRequest(tabId);
    await native({type:'status',requestId,state:'failed',message:'The import tab was closed. Submit the URL again to retry.'});
  }).catch(e=>{lastError=e.message;badge('!');});
});
// MV3 native connection keeps the service worker alive; timer never polls faster than 2 seconds.
setInterval(poll,2000);
api.alarms.create('burrata-wake',{periodInMinutes:1});
api.alarms.onAlarm.addListener(()=>{poll();});
