import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import {webcrypto} from 'node:crypto';

const source=readFileSync(new URL('../background.js',import.meta.url),'utf8');
const sharedSpotifySource=readFileSync(new URL('../provider-operations.js',import.meta.url),'utf8');
const youtubeSessionSource=readFileSync(new URL('../youtube-session-provider.js',import.meta.url),'utf8');

test('Spotify playlist rows resolve wrapper URIs, preserve duplicate UIDs, and retain unavailable rows',async()=>{
 const trackUri='spotify:track:0123456789abcdefghij';
 const item=(uid,data,uri)=>({uid,itemV2:{...(uri?{_uri:uri}:{}),data}});
 const playlist={
  id:'playlist1234567890',name:'Fixture playlist',revisionId:'revision-1',
  content:{totalCount:4,items:[
   item('entry-a',{name:'Same track',artists:{items:[{profile:{name:'Artist'}}]},albumOfTrack:{name:'Album'},trackDuration:{totalMilliseconds:215501},playcount:0},trackUri),
   item('entry-b',{name:'Same track',artists:{items:[{profile:{name:'Artist'}}]},albumOfTrack:{name:'Album'},trackDuration:{totalMilliseconds:215000},playCount:'1,234'},trackUri),
   item('entry-unavailable',{name:'Removed song'},null),
   item('entry-seconds',{name:'Duration in seconds',duration:180.25},'spotify:track:abcdefghij1234567890'),
  ]},
 };
 const fetch=async(url,options={})=>{
  if(String(url).includes('/api/token'))return {ok:true,status:200,text:async()=>JSON.stringify({accessToken:'TEST-FIXTURE-CREDENTIAL-578BC7AF1F-page-owned-token',isAnonymous:false})};
  const request=JSON.parse(options.body);
  const body=request.operationName==='profileAttributes'
   ?{data:{me:{profile:{uri:'spotify:user:account-123',name:'Fixture account'}}}}
   :{data:{playlistV2:playlist}};
  return {ok:true,status:200,text:async()=>JSON.stringify(body)};
 };
 const chrome={
  storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},
  runtime:{onMessage:{addListener(){}},lastError:null},action:{setBadgeText(){},setBadgeBackgroundColor(){}},
  tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}},
 };
 const context=vm.createContext({chrome,fetch,crypto:webcrypto,URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'open.spotify.com',origin:'https://open.spotify.com'},setInterval(){},setTimeout,clearTimeout});
 vm.runInContext(source,context);vm.runInContext(sharedSpotifySource,context);
 const hashes=Object.fromEntries(['profileAttributes','fetchPlaylist'].map(name=>[name,{hash:'a'.repeat(64)}]));
 hashes.totp={code:'123456',version:8};
 const result=await context.BurrataSpotifyProvider.run('spotify','getPlaylist',{playlistId:'playlist1234567890'},hashes);
 const normalized=JSON.parse(JSON.stringify(result));

 assert.equal(normalized.accountId,'account-123');
 assert.equal(normalized.entries.length,4);
 assert.deepEqual(normalized.entries.slice(0,2).map(entry=>entry.trackId),['0123456789abcdefghij','0123456789abcdefghij']);
 assert.deepEqual(normalized.entries.slice(0,2).map(entry=>entry.entryId),['entry-a','entry-b']);
 assert.equal(normalized.entries[0].artist,'Artist');
 assert.equal(normalized.entries[0].durationSeconds,215.501);
 assert.equal(normalized.entries[0].plays,0);
 assert.equal(normalized.entries[1].plays,1234);
 assert.equal(Object.hasOwn(normalized.entries[3],'plays'),false);
 assert.equal(normalized.entries[3].durationSeconds,180.25);
 assert.deepEqual(normalized.entries[2],{
  entryId:'entry-unavailable',trackId:null,title:'Removed song',artist:'',album:'',
  durationSeconds:null,url:null,artworkUrl:null,available:false,
 });
 assert.equal(normalized.complete,true);
});

test('Spotify artist ListPlatform duration preserves fractional nanoseconds and defaults missing nanos to whole seconds',async()=>{
 const artistId='A'.repeat(22),trackA='B'.repeat(22),trackB='C'.repeat(22),calls=[];
 const artistRow=(id,name,duration)=>({uid:`uid-${id}`,entity:{__typename:'EntityResponseWrapper',_uri:`spotify:track:${id}`,data:{__typename:'Entity',uri:`spotify:track:${id}`,entityTypeTrait:{__typename:'EntityTypeTrait',type:'ENTITY_TYPE_TRACK'},identityTrait:{__typename:'IdentityTrait',name,contributors:{items:[{name:'Fixture Artist',uri:`spotify:artist:${artistId}`}] }},consumptionExperienceTrait:{__typename:'ConsumptionExperienceTrait',duration},visualIdentityTrait:{__typename:'VisualIdentityTrait',squareCoverImage:{image:{data:{__typename:'ImageV2',sources:[{url:'https://i.scdn.co/image/fixture'}]}}}}}}});
 const rows=[artistRow(trackA,'Fractional', {seconds:131,nanoSeconds:706000000}),artistRow(trackB,'Whole second',{seconds:131})];
 const fetch=async(url,options={})=>{
  const target=String(url);if(target.includes('/api/token'))return {ok:true,status:200,text:async()=>JSON.stringify({accessToken:'fixture-token',isAnonymous:false})};
  const request=JSON.parse(options.body);calls.push(request);
  if(request.operationName==='profileAttributes')return {ok:true,status:200,text:async()=>JSON.stringify({data:{me:{profile:{uri:'spotify:user:fixture-account'}}}})};
  if(request.operationName==='getListsContents')return {ok:true,status:200,text:async()=>JSON.stringify({data:{lists:[{__typename:'List',items:{totalCount:2,items:rows}}]}})};
  throw new Error(`unexpected operation ${request.operationName}`);
 };
 const chrome={storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}}};
 const context=vm.createContext({chrome,fetch,crypto:webcrypto,URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'open.spotify.com',origin:'https://open.spotify.com'},setInterval(){},setTimeout,clearTimeout});vm.runInContext(source,context);vm.runInContext(sharedSpotifySource,context);
 const hashes={profileAttributes:{hash:'a'.repeat(64)},getListsContents:{hash:'b'.repeat(64)},totp:{code:'123456',version:61}};
 const result=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getArtistTracks',{artistId},hashes)));
 assert.deepEqual(result.items.map(item=>item.durationSeconds),[131.706,131]);
 assert.deepEqual(result.items.map(item=>item.trackId),[trackA,trackB]);
 assert.equal(result.items[0].position,1);assert.equal(result.items[1].position,2);
 const request=calls.find(item=>item.operationName==='getListsContents');assert.deepEqual(request.variables,{uris:[`spotify:list:popular-release-segments-main-roles:artist_${artistId}`],offset:0,limit:30});
});

test('Spotify artist releases and rich album tracks use pinned Pathfinder queries with offset pages',async()=>{
 const artistId='artist1234567890123456',albumId='album12345678901234567',secondAlbumId='otherAlbum123456789012',trackA='track12345678901234567',trackB='anotherTrack1234567890',calls=[];
 const release=(id,name)=>({__typename:'Album',uri:`spotify:album:${id}`,name,type:'ALBUM',date:{year:2024,isoString:'2024-01-01'},artists:{items:[{uri:`spotify:artist:${artistId}`,profile:{name:'Fixture Artist'}}]},coverArt:{sources:[{url:`https://i.scdn.co/image/${id}`,maxWidth:640,maxHeight:640}]},sharingInfo:{shareUrl:`https://open.spotify.com/album/${id}`},tracks:{totalCount:0}});
 const artistUnion={__typename:'Artist',uri:`spotify:artist:${artistId}`,profile:{name:'Fixture Artist'},visuals:{avatarImage:{sources:[{url:'https://i.scdn.co/image/artist',maxWidth:640,maxHeight:640}]}},stats:{followers:0},discography:{all:{totalCount:3}}};let discographyIdentityMode='omitted',albumIdentityMode='match';
 const track=(id,name,duration,number)=>({__typename:'Track',uri:`spotify:track:${id}`,name,artists:{items:[{uri:`spotify:artist:${artistId}`,profile:{name:'Fixture Artist'}}]},duration:{totalMilliseconds:duration},albumOfTrack:{uri:`spotify:album:${albumId}`,name:'First release',coverArt:{sources:[{url:'https://i.scdn.co/image/cover',maxWidth:640,maxHeight:640}]}},discNumber:1,trackNumber:number,playability:{playable:true},contentRating:{label:'EXPLICIT'},saved:false,...(number===1?{playcount:'0'}:number===2?{playCount:'1,234'}:number===3?{plays:'9007199254740992'}:{}),relinkingInformation:{linkedTrack:{__typename:'Track',uri:`spotify:track:${id}`}},associationsV3:{videoAssociations:{totalCount:number===4?999:0}}});
 const fetch=async(url,options={})=>{
  const target=String(url);calls.push({url:target,options});
  if(target.includes('/api/token'))return {ok:true,status:200,text:async()=>JSON.stringify({accessToken:'TEST-FIXTURE-CREDENTIAL-578BC7AF1F-page-owned-token',isAnonymous:false})};
  const request=JSON.parse(options.body),variables=request.variables||{};
  if(target.includes('/pathfinder/v2/query')){
   if(request.operationName==='profileAttributes')return {ok:true,status:200,text:async()=>JSON.stringify({data:{me:{profile:{uri:'spotify:user:account-123',name:'Fixture account'}}}})};
   if(request.operationName==='queryArtistMinimal')return {ok:true,status:200,text:async()=>JSON.stringify({data:{artistUnion}})};
   if(request.operationName==='queryArtistDiscographyAll'){
    const items=variables.offset===0?[{releases:{items:[release(albumId,'First release')]}},{releases:{items:[]}}]:[{releases:{items:[release(secondAlbumId,'Second release')]}}];
    const {uri:omittedArtistUri,...discographyArtistUnion}=artistUnion;
    const responseArtistUnion=discographyIdentityMode==='mismatch'?{...discographyArtistUnion,uri:'spotify:artist:BBBBBBBBBBBBBBBBBBBBBB'}:discographyArtistUnion;
    return {ok:true,status:200,text:async()=>JSON.stringify({data:{artistUnion:{...responseArtistUnion,discography:{all:{totalCount:3,items}}}}})};
   }
   if(request.operationName==='getAlbum'){
    const id=variables.uri.split(':').pop();
    const items=id!==albumId?[]:variables.offset===0?Array.from({length:50},(_,index)=>index===25?{track:null}:{track:track(index===0?trackA:String(index).padStart(22,'0'),index===0?'Opening':`Track ${index+1}`,index===0?183250:index===1?183333:180000,index+1)}):[{track:track(trackB,'Finale',200000,51)}];
    const responseId=albumIdentityMode==='mismatch'?secondAlbumId:id;
    return {ok:true,status:200,text:async()=>JSON.stringify({data:{albumUnion:{...release(responseId,id===albumId?'First release':'Empty release'),...(id===albumId?{playcount:'2,001'}:{}),artists:{items:[{uri:`spotify:artist:${artistId}`,profile:{name:'Fixture Artist'}}]},tracksV2:{totalCount:id===albumId?51:0,items}}}})};
   }
   throw new Error(`unexpected GraphQL operation ${request.operationName}`);
  }
  throw new Error(`unexpected ${target}`);
 };
 const chrome={storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}}};
 const context=vm.createContext({chrome,fetch,crypto:webcrypto,URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'open.spotify.com',origin:'https://open.spotify.com'},setInterval(){},setTimeout,clearTimeout});vm.runInContext(source,context);vm.runInContext(sharedSpotifySource,context);
 const hashes={profileAttributes:{hash:'a'.repeat(64)},totp:{code:'123456',version:61}};
 const artist=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getArtist',{artistId},hashes)));
 assert.equal(artist.artist?.title,'Fixture Artist',JSON.stringify(artist));assert.equal(artist.artist.artworkUrl,'https://i.scdn.co/image/artist');assert.equal(artist.artist.data.followers,0);
 const pageOne=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getArtistAlbums',{artistId,limit:30},hashes)));
 assert.equal(pageOne.items.length,1);assert.equal(pageOne.items[0].title,'First release');assert.equal(pageOne.items[0].artworkUrl,`https://i.scdn.co/image/${albumId}`);assert.equal(pageOne.total,3);assert.equal(pageOne.nextCursor,'2');assert.equal(pageOne.unrecognizedCount,1);assert.equal(Object.hasOwn(pageOne,'complete'),false);
 const pageTwo=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getArtistAlbums',{artistId,limit:30,cursor:pageOne.nextCursor},hashes)));
 assert.equal(pageTwo.items[0].id,secondAlbumId);assert.equal(pageTwo.total,3);assert.equal(pageTwo.nextCursor,null);
 discographyIdentityMode='mismatch';const mismatchedDiscography=await context.BurrataSpotifyProvider.run('spotify','getArtistAlbums',{artistId,limit:30},hashes);assert.equal(mismatchedDiscography.__burrataProviderError.code,'PROVIDER_CHANGED');
 const album=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getAlbum',{albumId},hashes)));
 assert.equal(album.album.title,'First release');assert.equal(album.album.data.totalTracks,51);assert.equal(album.album.data.plays,2001);
 const emptyAlbum=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getAlbum',{albumId:'emptyAlbum123456789012'},hashes)));
 assert.equal(emptyAlbum.album.data.totalTracks,0);
 assert.equal(Object.hasOwn(emptyAlbum.album.data,'plays'),false);
 const firstTrackPage=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getAlbumTracks',{albumId,limit:50},hashes)));
 assert.equal(firstTrackPage.items.length,49);assert.equal(firstTrackPage.items[0].durationSeconds,183.25);assert.equal(firstTrackPage.items[1].durationSeconds,183.333);assert.equal(firstTrackPage.items[0].position,1);assert.equal(firstTrackPage.items[0].trackNumber,1);assert.equal(firstTrackPage.items[0].isExplicit,true);assert.equal(firstTrackPage.items[0].plays,0);assert.equal(firstTrackPage.items[1].plays,1234);assert.equal(Object.hasOwn(firstTrackPage.items[2],'plays'),false);assert.equal(Object.hasOwn(firstTrackPage.items[3],'plays'),false);assert.equal(firstTrackPage.items[3].hasAssociatedVideo,true);assert.equal(firstTrackPage.total,51);assert.equal(firstTrackPage.nextCursor,'50');assert.equal(firstTrackPage.unrecognizedCount,1);assert.equal(Object.hasOwn(firstTrackPage,'complete'),false);assert.equal(Object.hasOwn(firstTrackPage,'pagingQualification'),false);
 const secondTrackPage=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getAlbumTracks',{albumId,limit:50,cursor:firstTrackPage.nextCursor},hashes)));
 assert.equal(secondTrackPage.items[0].trackId,trackB);assert.equal(secondTrackPage.nextCursor,null);assert.equal(secondTrackPage.pagingQualification,'experimental-offset');
 albumIdentityMode='mismatch';const mismatchedAlbumTracks=await context.BurrataSpotifyProvider.run('spotify','getAlbumTracks',{albumId,limit:50},hashes);assert.equal(mismatchedAlbumTracks.__burrataProviderError.code,'PROVIDER_CHANGED');albumIdentityMode='match';
 const gqlCalls=calls.filter(call=>call.url.includes('/pathfinder/v2/query')).map(call=>JSON.parse(call.options.body));
 const expectedHashes={queryArtistMinimal:'53d3f76582c49ad0a05dc685955f20dc2a5f2209b192e5446e5e4e623ce23a48',queryArtistDiscographyAll:'5e07d323febb57b4a56a42abbf781490e58764aa45feb6e3dc0591564fc56599',getAlbum:'11c9cad9cb24c4f9e25ef9456f68621cfd6006a90f679e729c1d2f2fd70dfe16'};
 const artistOverviewCall=gqlCalls.find(request=>request.operationName==='queryArtistMinimal');assert.deepEqual(JSON.parse(JSON.stringify(artistOverviewCall.variables)),{uri:`spotify:artist:${artistId}`});
 for(const request of gqlCalls.filter(request=>Object.hasOwn(expectedHashes,request.operationName)))assert.equal(request.extensions.persistedQuery.sha256Hash,expectedHashes[request.operationName]);
 const artistPageCall=gqlCalls.find(request=>request.operationName==='queryArtistDiscographyAll');assert.deepEqual(JSON.parse(JSON.stringify(artistPageCall.variables)),{uri:`spotify:artist:${artistId}`,offset:0,limit:20,order:'DATE_DESC'});
 const albumTrackCalls=gqlCalls.filter(request=>request.operationName==='getAlbum'&&request.variables?.locale==='');assert.ok(albumTrackCalls.some(request=>request.variables?.offset===0&&request.variables?.limit===50));assert.ok(albumTrackCalls.some(request=>request.variables?.offset===50&&request.variables?.limit===50));assert.equal(gqlCalls.some(request=>request.operationName==='queryAlbumTracks'),false);
 assert.ok(calls.every(call=>!call.url.includes('api.spotify.com/v1/')));assert.ok(calls.filter(call=>call.url.includes('api-partner.spotify.com')).every(call=>call.options.credentials==='include'&&call.options.headers.Authorization==='Bearer TEST-FIXTURE-CREDENTIAL-578BC7AF1F-page-owned-token'));
});

test('Spotify native catalog rejects typed not-found and mismatched pinned identities',async()=>{
 const artistId='artist1234567890123456',albumId='album12345678901234567';let mode='artist-not-found';
 const fetch=async(url,options={})=>{
  if(String(url).includes('/api/token'))return {ok:true,status:200,text:async()=>JSON.stringify({accessToken:'TEST-FIXTURE-CREDENTIAL-578BC7AF1F-page-owned-token',isAnonymous:false})};
  const request=JSON.parse(options.body);
  const data=request.operationName==='profileAttributes'?{me:{profile:{uri:'spotify:user:account-123',name:'Fixture account'}}}:request.operationName==='queryArtistMinimal'?{artistUnion:mode==='artist-not-found'?{__typename:'NotFound'}:{__typename:'Artist',uri:`spotify:artist:${mode==='artist-mismatch'?'differentArtist12345678':artistId}`,profile:{name:'Fixture Artist'}}}:{albumUnion:{__typename:'NotFound'}};
  return {ok:true,status:200,text:async()=>JSON.stringify({data})};
 };
 const chrome={storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}}};
 const context=vm.createContext({chrome,fetch,crypto:webcrypto,URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'open.spotify.com',origin:'https://open.spotify.com'},setInterval(){},setTimeout,clearTimeout});vm.runInContext(source,context);vm.runInContext(sharedSpotifySource,context);
 const hashes={profileAttributes:{hash:'a'.repeat(64)},totp:{code:'123456',version:61}};
 const notFound=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getArtist',{artistId},hashes)));assert.equal(notFound.__burrataProviderError.code,'ARTIST_NOT_FOUND');
 mode='artist-mismatch';const mismatch=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getArtist',{artistId},hashes)));assert.equal(mismatch.__burrataProviderError.code,'PROVIDER_CHANGED');
 mode='album-not-found';const albumNotFound=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getAlbum',{albumId},hashes)));assert.equal(albumNotFound.__burrataProviderError.code,'ALBUM_NOT_FOUND');
});

test('Spotify GraphQL catalog does not retry rate limits or fall back to catalog REST',async()=>{
 const artistId='artist1234567890123456',calls=[];
 const fetch=async(url,options={})=>{
  calls.push(String(url));
  if(String(url).includes('/api/token'))return {ok:true,status:200,text:async()=>JSON.stringify({accessToken:'TEST-FIXTURE-CREDENTIAL-578BC7AF1F-page-owned-token',isAnonymous:false})};
  const request=JSON.parse(options.body);if(request.operationName==='profileAttributes')return {ok:true,status:200,text:async()=>JSON.stringify({data:{me:{profile:{uri:'spotify:user:account-123',name:'Fixture account'}}}})};
  return {ok:false,status:429,text:async()=>JSON.stringify({error:{message:'rate limited'}})};
 };
 const chrome={storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}}};
 const context=vm.createContext({chrome,fetch,crypto:webcrypto,URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'open.spotify.com',origin:'https://open.spotify.com'},setInterval(){},setTimeout,clearTimeout});vm.runInContext(source,context);vm.runInContext(sharedSpotifySource,context);
 const hashes={profileAttributes:{hash:'a'.repeat(64)},totp:{code:'123456',version:61}};
 const denied=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getArtist',{artistId},hashes)));assert.equal(denied.__burrataProviderError.code,'RATE_LIMITED');
 assert.equal(calls.filter(url=>url.includes('/pathfinder/v2/query')).length,2);assert.ok(calls.every(url=>!url.includes('api.spotify.com/v1/')));
});

test('Spotify getTrack fetches only the exact requested ID and normalizes verified duration',async()=>{
 const calls=[];let returnedId='track12345678901234567';
 const fetch=async(url,options={})=>{
  const target=String(url);calls.push({target,options});
  if(target.includes('/api/token'))return {ok:true,status:200,text:async()=>JSON.stringify({accessToken:'TEST-FIXTURE-CREDENTIAL-578BC7AF1F-page-owned-token',isAnonymous:false})};
  if(target.includes('/pathfinder/v2/query'))return {ok:true,status:200,text:async()=>JSON.stringify({data:{me:{profile:{uri:'spotify:user:account-123',name:'Fixture account'}}}})};
  if(target.endsWith('/tracks/track12345678901234567'))return {ok:true,status:200,text:async()=>JSON.stringify({id:returnedId,name:'Exact track',duration_ms:245480,artists:[{name:'Fixture Artist'}],album:{name:'Fixture album',images:[{url:'https://i.scdn.co/image/cover'}]}})};
  throw new Error(`unexpected ${target}`);
 };
 const chrome={storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}}};
 const context=vm.createContext({chrome,fetch,URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'open.spotify.com',origin:'https://open.spotify.com'},setInterval(){},setTimeout,clearTimeout});vm.runInContext(sharedSpotifySource,context);
 const hashes={profileAttributes:{hash:'a'.repeat(64)},totp:{code:'123456',version:61}};
 const result=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getTrack',{trackId:'track12345678901234567'},hashes)));
 assert.equal(result.accountId,'account-123');assert.equal(result.items[0].id,'track12345678901234567');assert.equal(result.items[0].data.durationSeconds,245.48);
 const trackCall=calls.find(call=>call.target.includes('/tracks/track12345678901234567'));
 assert.equal(trackCall.options.credentials,'include');assert.equal(trackCall.options.headers.Authorization,'Bearer TEST-FIXTURE-CREDENTIAL-578BC7AF1F-page-owned-token');
 returnedId='differentTrack1234567890';
 const mismatch=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getTrack',{trackId:'track12345678901234567'},hashes)));
 assert.equal(mismatch.__burrataProviderError.code,'PROVIDER_CHANGED');
 const invalid=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','getTrack',{trackId:'short'},hashes)));
 assert.equal(invalid.__burrataProviderError.code,'UNSUPPORTED');
});

test('Spotify profile search uses display names, usernames, and ID fallback without empty titles',async()=>{
 let mode='normal';
 const fetch=async(url,options={})=>{
  if(String(url).includes('/api/token'))return {ok:true,status:200,text:async()=>JSON.stringify({accessToken:'TEST-FIXTURE-CREDENTIAL-578BC7AF1F-page-owned-token',isAnonymous:false})};
  const request=JSON.parse(options.body);
  assert.ok(['profileAttributes','searchUsers'].includes(request.operationName));
  if(request.operationName==='searchUsers'){assert.equal(request.variables.searchTerm,'Andrew');assert.equal(request.extensions.persistedQuery.sha256Hash,'8f358dd82e62f61dd4ceaa9f8cd0889e644c9b707f1b724fbfb356a757cb7e5a');}
  let body=request.operationName==='profileAttributes'
   ?{data:{me:{profile:{uri:'spotify:user:account-123',name:'Fixture account'}}}}
   :{data:{searchV2:{users:{totalCount:5,pagingInfo:{nextOffset:4},items:[
    {item:{_uri:'spotify:user:display-user',data:{profile:{displayName:'  Display Name  '}}}},
    {item:{data:{username:'üser+name',profile:{username:'üser+name'}}}},
   {item:{_uri:'spotify:user:id-fallback',data:{profile:{name:'   '}}}},
   {item:{data:{profile:{displayName:'Missing stable ID'}}}},
   ]}}}};
  if(request.operationName==='searchUsers'&&mode==='missing')body={data:{searchV2:{profiles:{items:[],totalCount:0}}}};
  if(request.operationName==='searchUsers'&&mode==='repeat')body.data.searchV2.users.pagingInfo.nextOffset=0;
  if(request.operationName==='searchUsers'&&mode==='empty')body={data:{searchV2:{users:{items:[],totalCount:0,pagingInfo:{nextOffset:null}}}}};
  return {ok:true,status:200,text:async()=>JSON.stringify(body)};
 };
 const chrome={storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}}};
 const context=vm.createContext({chrome,fetch,crypto:webcrypto,URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'open.spotify.com',origin:'https://open.spotify.com'},setInterval(){},setTimeout,clearTimeout});vm.runInContext(source,context);vm.runInContext(sharedSpotifySource,context);
 const hashes=Object.fromEntries(['profileAttributes','searchDesktop'].map(name=>[name,{hash:'a'.repeat(64)}]));hashes.totp={code:'123456',version:61};
 const result=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','search',{query:'Andrew',kind:'profile'},hashes)));
 assert.deepEqual(result.items.map(({id,title,url})=>({id,title,url})),[
  {id:'display-user',title:'Display Name',url:'https://open.spotify.com/user/display-user'},
  {id:'üser+name',title:'üser+name',url:'https://open.spotify.com/user/%C3%BCser%2Bname'},
  {id:'id-fallback',title:'id-fallback',url:'https://open.spotify.com/user/id-fallback'},
 ]);
 assert.ok(result.items.every(item=>item.title.trim().length>0));
 assert.equal(result.nextCursor,'4');
 mode='missing';const missing=await context.BurrataSpotifyProvider.run('spotify','search',{query:'Andrew',kind:'profile'},hashes);assert.equal(missing.__burrataProviderError.code,'PROVIDER_CHANGED');
 mode='repeat';const repeated=await context.BurrataSpotifyProvider.run('spotify','search',{query:'Andrew',kind:'profile'},hashes);assert.equal(repeated.__burrataProviderError.code,'PROVIDER_CHANGED');
 mode='empty';const empty=await context.BurrataSpotifyProvider.run('spotify','search',{query:'Andrew',kind:'profile'},hashes);assert.equal(empty.items.length,0);assert.equal(empty.nextCursor,null);
});

test('Spotify track search retains exact artist refs and contributor count for safe navigation',async()=>{
 const artistA='aaaaaaaaaaaaaaaaaaaaaa',artistB='bbbbbbbbbbbbbbbbbbbbbb',trackId='cccccccccccccccccccccc';
 const fetch=async(url,options={})=>{
  if(String(url).includes('/api/token'))return {ok:true,status:200,text:async()=>JSON.stringify({accessToken:'TEST-FIXTURE-CREDENTIAL-578BC7AF1F-page-owned-token',isAnonymous:false})};
  const request=JSON.parse(options.body);
  if(request.operationName==='profileAttributes')return {ok:true,status:200,text:async()=>JSON.stringify({data:{me:{profile:{uri:'spotify:user:account-123',name:'Fixture account'}}}})};
  return {ok:true,status:200,text:async()=>JSON.stringify({data:{searchV2:{tracksV2:{totalCount:1,items:[{item:{_uri:`spotify:track:${trackId}`,data:{name:'Fixture song',artists:{items:[]},identityTrait:{contributors:{items:[{uri:`spotify:artist:${artistA}`,name:'Primary Artist'},{uri:`spotify:artist:${artistB}`,name:'Featured Artist'},{uri:`untrusted:artist:${'c'.repeat(22)}`,name:'Untrusted identity'}]}},albumOfTrack:{name:'Fixture album'}}}}]}}}})};
 };
 const chrome={storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}},},runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}}};
 const context=vm.createContext({chrome,fetch,URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'open.spotify.com',origin:'https://open.spotify.com'},setInterval(){},setTimeout,clearTimeout});vm.runInContext(sharedSpotifySource,context);
 const hashes={profileAttributes:{hash:'a'.repeat(64)},searchDesktop:{hash:'b'.repeat(64)},totp:{code:123456,version:61}};
 const result=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','search',{query:'Fixture song',kind:'track'},hashes)));
 assert.equal(result.items[0].data.artistCount,3);
 assert.deepEqual(result.items[0].data.artistRefs,[{provider:'spotify',id:artistA,name:'Primary Artist'},{provider:'spotify',id:artistB,name:'Featured Artist'}]);
});

test('signed-out YouTube public playlist search returns normalized items and continuation',async()=>{
 const calls=[];
 const responseBody={
  contents:{sectionListRenderer:{contents:[{itemSectionRenderer:{contents:[{
   playlistRenderer:{playlistId:'PLpublic123456789',title:{runs:[{text:'The Challenge'}]},shortBylineText:{simpleText:'Public channel'},thumbnails:[{thumbnails:[{url:'https://i.ytimg.com/public.jpg'}]}],videoCount:'12'},
  }]}}]}},
  continuationContents:{sectionListContinuation:{continuations:[{nextContinuationData:{continuation:'next-public-page'}}]}},
 };
 const fetch=async(url,options={})=>{
  calls.push({url:String(url),options});
  return {ok:true,status:200,text:async()=>JSON.stringify(responseBody)};
 };
 const chrome={
  storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},
  runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},
  tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}},
 };
 const context=vm.createContext({
  chrome,fetch,URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'music.youtube.com',origin:'https://music.youtube.com'},
  document:{cookie:'',querySelector:()=>null},
  ytcfg:{data_:{INNERTUBE_API_KEY:'public-api-key',INNERTUBE_CLIENT_NAME:'WEB_REMIX',INNERTUBE_CLIENT_VERSION:'1.20261001.00.00',VISITOR_DATA:'public-visitor'}},
  setInterval(){},setTimeout,clearTimeout,
 });
 vm.runInContext(source,context);
 const result=await context.runProviderPageOperation('youtube','search',{query:'synthwave',kind:'playlist',limit:10},{});
 const normalized=JSON.parse(JSON.stringify(result));
 assert.equal(normalized.accountId,null);
 assert.deepEqual(normalized.items,[{
  id:'PLpublic123456789',kind:'playlist',title:'The Challenge',subtitle:'Public channel',
  url:'https://www.youtube.com/playlist?list=PLpublic123456789',artworkUrl:'https://i.ytimg.com/public.jpg',
  data:{provider:'youtube',playlistId:'PLpublic123456789',total:12},
 }]);
 assert.equal(normalized.nextCursor,'next-public-page');
 assert.equal(calls.length,1);
 assert.match(calls[0].url,/youtubei\/v1\/search\?/);
 assert.equal(calls[0].options.credentials,'include');
 assert.equal(calls[0].options.headers.Authorization,undefined);
});

test('modern YouTube lockup view models decode public playlists and channels',async()=>{
 const calls=[];
 const lockup=(contentType,contentId,title,subtitle,image)=>({lockupViewModel:{
  contentType,contentId,
  metadata:{lockupMetadataViewModel:{title:{content:title},metadata:{contentMetadataViewModel:{metadataRows:[{metadataParts:[{text:{content:subtitle}}]}]}}}},
  contentImage:{thumbnailViewModel:{image:{sources:[{url:image}]}}},
 }});
 const fetch=async(url,options={})=>{
  const request=JSON.parse(options.body);calls.push(request);
  const isChannel=request.params==='EgIQAg%3D%3D';
  const result=lockup(isChannel?'LOCKUP_CONTENT_TYPE_CHANNEL':'LOCKUP_CONTENT_TYPE_PLAYLIST',isChannel?'UCmodernChannel123':'VLPLmodernPlaylist123',isChannel?'Public channel':'Public playlist',isChannel?'14K subscribers':'42 videos',`https://i.ytimg.com/${isChannel?'channel':'playlist'}.jpg`);
  return {ok:true,status:200,text:async()=>JSON.stringify({contents:{sectionListRenderer:{contents:[{itemSectionRenderer:{contents:[result]}}]}}})};
 };
 const chrome={storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}}};
 const context=vm.createContext({chrome,fetch,URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'music.youtube.com',origin:'https://music.youtube.com'},document:{cookie:'',querySelector:()=>null},ytcfg:{data_:{INNERTUBE_API_KEY:'public-api-key',INNERTUBE_CLIENT_NAME:'WEB_REMIX',INNERTUBE_CLIENT_VERSION:'1.20261001.00.00'}},setInterval(){},setTimeout,clearTimeout});
 vm.runInContext(source,context);
 const playlist=JSON.parse(JSON.stringify(await context.runProviderPageOperation('youtube','search',{query:'Gipsy Kings',kind:'playlist'},{})));
 assert.equal(playlist.items[0].id,'PLmodernPlaylist123');
 assert.equal(playlist.items[0].title,'Public playlist');
 assert.equal(playlist.items[0].subtitle,'42 videos');
 assert.equal(playlist.items[0].artworkUrl,'https://i.ytimg.com/playlist.jpg');
 const channel=JSON.parse(JSON.stringify(await context.runProviderPageOperation('youtube','search',{query:'Gipsy Kings',kind:'channel'},{})));
 assert.equal(channel.items[0].id,'UCmodernChannel123');
 assert.equal(channel.items[0].kind,'channel');
 assert.equal(channel.items[0].subtitle,'14K subscribers');
 assert.equal(calls.length,2);
});

test('signed-in YouTube status binds identity from studio channel link and DATASYNC_ID fallback',async()=>{
 const chrome={storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}}};
 const link='https://studio.youtube.com/channel/UCstableOwnedChannel/content';
 const makeContext=(data_,studio=true)=>{const context=vm.createContext({chrome,fetch:async()=>{throw new Error('status must not fetch');},crypto:globalThis.crypto,URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'music.youtube.com',origin:'https://music.youtube.com'},document:{cookie:'SAPISID=browser-session-cookie',querySelector(selector){return studio&&selector.includes('studio.youtube.com/channel/')?{getAttribute:()=>link}:null;}},ytcfg:{data_},setInterval(){},setTimeout,clearTimeout});vm.runInContext(source,context);return context;};
 const fromStudio=await makeContext({LOGGED_IN:true}).runProviderPageOperation('youtube','status',{},{});
 assert.equal(fromStudio.accountId,'UCstableOwnedChannel',JSON.stringify(fromStudio));
 const fromDataSync=await makeContext({LOGGED_IN:true,DATASYNC_ID:'V1-stableDataSync||opaque-token'},false).runProviderPageOperation('youtube','status',{},{});
 assert.equal(fromDataSync.accountId,'V1-stableDataSync');
});

test('modern YouTube playlist browse decodes lockup video rows, page header, and continuation item',async()=>{
 const request=[];
 const row={lockupViewModel:{
  contentId:'videoModern123',contentType:'LOCKUP_CONTENT_TYPE_VIDEO',
  metadata:{lockupMetadataViewModel:{title:{content:'Bamboleo'},metadata:{contentMetadataViewModel:{metadataRows:[
   {metadataParts:[{text:{content:'Gipsy Kings'}}]},
   {metadataParts:[{text:{content:'3:26'}}]},
  ]}}}},
  contentImage:{thumbnailViewModel:{image:{sources:[{url:'https://i.ytimg.com/track.jpg'}]}}},
 }};
 const unavailable={playlistVideoRenderer:{setVideoId:'hidden-entry-1',isPlayable:false,unplayableText:{simpleText:'Video unavailable'}}};
 const continuation={continuationItemViewModel:{trigger:'CONTINUATION_TRIGGER_ON_ITEM_SHOWN',continuationCommand:{innertubeCommand:{continuationCommand:{token:'next-playlist-page',request:'CONTINUATION_REQUEST_TYPE_NEXT'}}}}};
 const body={
  pageHeaderRenderer:{pageTitle:'Legacy playlist title',content:{pageHeaderViewModel:{title:{dynamicTextViewModel:{text:{content:'Modern playlist title'}}},heroImage:{contentPreviewImageViewModel:{thumbnail:{sources:[{url:'https://i.ytimg.com/playlist-art.jpg'}]}}}}}},
  playlistMetadataRenderer:{title:'Metadata playlist title'},
  contents:{sectionListRenderer:{contents:[{musicPlaylistShelfRenderer:{contents:[row,unavailable,continuation]}}]}},
  playlistMetadataRenderer:{title:'Metadata playlist title',videoCount:'2'},
 };
 const chrome={storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}}};
 const context=vm.createContext({chrome,fetch:async(_url,options)=>{request.push(JSON.parse(options.body));return {ok:true,status:200,text:async()=>JSON.stringify(body)};},URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'music.youtube.com',origin:'https://music.youtube.com'},document:{cookie:'',querySelector:()=>null},ytcfg:{data_:{INNERTUBE_API_KEY:'public-api-key',INNERTUBE_CLIENT_NAME:'WEB_REMIX',INNERTUBE_CLIENT_VERSION:'1.20261001.00.00'}},setInterval(){},setTimeout,clearTimeout});
 vm.runInContext(source,context);
 const result=JSON.parse(JSON.stringify(await context.runProviderPageOperation('youtube','getPlaylist',{playlistId:'PLmodernPlaylist123'},{})));
 assert.equal(result.playlist.title,'Modern playlist title');
 assert.equal(result.playlist.artworkUrl,'https://i.ytimg.com/playlist-art.jpg');
 assert.equal(result.entries.length,2);
 assert.deepEqual(result.entries[0],{entryId:null,trackId:'videoModern123',title:'Bamboleo',artist:'Gipsy Kings',album:'',durationSeconds:206,url:'https://www.youtube.com/watch?v=videoModern123',artworkUrl:'https://i.ytimg.com/track.jpg',available:true});
 assert.deepEqual(result.entries[1],{entryId:'hidden-entry-1',trackId:null,title:'Video unavailable',artist:'',album:'',durationSeconds:null,url:null,artworkUrl:null,available:false});
 assert.equal(result.total,2);
 assert.equal(result.nextCursor,'next-playlist-page');
 assert.equal(result.complete,false);
 assert.deepEqual(request[0],{context:request[0].context,browseId:'VLPLmodernPlaylist123',params:'wgYCCAA='});
});

const profileVarint=value=>{let n=BigInt(value),bytes=[];while(n>127n){bytes.push(Number(n&127n)|128);n>>=7n;}bytes.push(Number(n));return bytes;};
const profileField=(number,wire,payload)=>[...profileVarint((number<<3)|wire),...payload];
const profileString=(number,value)=>{const bytes=[...new TextEncoder().encode(value)];return profileField(number,2,[...profileVarint(bytes.length),...bytes]);};
const profileInt=(number,value)=>profileField(number,0,profileVarint(value));
function profilePlaylistProto(item){const uri=item.uri||item._uri||'',name=item.name||item.title||'',image=item.imageUrl||item.image_url||'',ownerName=item.ownerName||item.owner_name||'',ownerUri=item.ownerUri||item.owner_uri||'',parts=[];if(uri)parts.push(...profileString(1,uri));if(name)parts.push(...profileString(2,name));if(image)parts.push(...profileString(3,image));if(Number.isSafeInteger(Number(item.followersCount??item.followers_count))&&Number(item.followersCount??item.followers_count)>0)parts.push(...profileInt(4,Number(item.followersCount??item.followers_count)));if(ownerName)parts.push(...profileString(5,ownerName));if(ownerUri)parts.push(...profileString(6,ownerUri));if(item.isFollowing||item.is_following)parts.push(...profileInt(7,1));return parts;}
function profilePageProto(profile,offset,limit){const rows=Array.isArray(profile.public_playlists)?profile.public_playlists.slice(offset,offset+limit):[],bytes=rows.flatMap(item=>{const message=profilePlaylistProto(item);return profileField(1,2,[...profileVarint(message.length),...message]);}),hasTotal=Object.prototype.hasOwnProperty.call(profile,'total_public_playlists_count')||Object.prototype.hasOwnProperty.call(profile,'totalPublicPlaylistsCount'),total=Number(profile.total_public_playlists_count??profile.totalPublicPlaylistsCount??0);if(hasTotal)bytes.push(...profileInt(2,total));return new Uint8Array(bytes);}
function profileSummaryProto(profile){const rows=Array.isArray(profile.public_playlists)?profile.public_playlists:[],bytes=[];if(profile.uri)bytes.push(...profileString(1,profile.uri));if(profile.name)bytes.push(...profileString(2,profile.name));for(const item of rows){const message=profilePlaylistProto(item);bytes.push(...profileField(8,2,[...profileVarint(message.length),...message]));}const hasTotal=Object.prototype.hasOwnProperty.call(profile,'total_public_playlists_count')||Object.prototype.hasOwnProperty.call(profile,'totalPublicPlaylistsCount');if(hasTotal)bytes.push(...profileInt(9,Number(profile.total_public_playlists_count??profile.totalPublicPlaylistsCount)));return new Uint8Array(bytes);}

test('Spotify own library preserves unnamed playlists with a safe display title',async()=>{
 const requests=[];let profileOverride=null,profileSummaryOverride=null;
 const fetch=async(url,options={})=>{
  if(String(url).includes('/api/token'))return {ok:true,status:200,text:async()=>JSON.stringify({accessToken:'TEST-FIXTURE-CREDENTIAL-578BC7AF1F-page-owned-token',isAnonymous:false})};
  if(String(url).includes('spclient.wg.spotify.com/user-profile-view/')){requests.push({url:String(url),options});const parsedUrl=new URL(url),summary=!parsedUrl.pathname.endsWith('/playlists'),path=parsedUrl.pathname.split('/'),requested=decodeURIComponent(path[summary?path.length-1:path.length-2]),offset=Number(parsedUrl.searchParams.get('offset')),limit=Number(parsedUrl.searchParams.get('limit')),body=profileOverride||{uri:`spotify:user:${requested}`,name:'Owner',total_public_playlists_count:2,public_playlists:[{uri:'spotify:playlist:37i9dQZF1DXcBWIGoYBM5M',name:'Public playlist',image_url:'https://i.scdn.co/image/profile-playlist',owner_name:'Owner',owner_uri:`spotify:user:${requested}`},{uri:'spotify:playlist:0000000000000000000001',name:'Second public playlist',image_url:'https://i.scdn.co/image/profile-playlist-2',owner_name:'Owner',owner_uri:`spotify:user:${requested}`} ]};const bytes=summary?(profileSummaryOverride||profileSummaryProto(body)):profilePageProto(body,offset,limit);return {ok:true,status:200,headers:{get:name=>String(name).toLowerCase()==='content-type'?'application/x-protobuf':null},arrayBuffer:async()=>bytes.buffer};}
  const request=JSON.parse(options.body);
  const libraryItems=Array.from({length:48},(_,index)=>({item:{__typename:'Playlist',data:{uri:`spotify:playlist:playlist${String(index).padStart(32,'0')}`,...(index?{name:`Playlist ${index}`}:{}) ,...(index===0?{attributes:[{key:'trackCount',value:'0'}],public:false}:index===1?{attributes:{totalCount:7,visibility:'private'}}:{})}}}));
  libraryItems.push({item:{__typename:'PseudoPlaylist',data:{uri:'spotify:collection:tracks',name:'Liked Songs',owner:{id:'account-123'},canEdit:true}}},{item:{__typename:'Artist',data:{name:'Not a playlist'}}});
  const body=request.operationName==='profileAttributes'?{data:{me:{profile:{uri:'spotify:user:account-123',name:'Fixture account'}}}}:{data:{me:{libraryV3:{totalCount:100,items:libraryItems}}}};
  return {ok:true,status:200,text:async()=>JSON.stringify(body)};
 };
 const chrome={storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}}};
 const context=vm.createContext({chrome,fetch,crypto:webcrypto,URL,URLSearchParams,Response,TextEncoder,TextDecoder,AbortController,location:{hostname:'open.spotify.com',origin:'https://open.spotify.com'},setInterval(){},setTimeout,clearTimeout});vm.runInContext(source,context);vm.runInContext(sharedSpotifySource,context);
 const hashes=Object.fromEntries(['profileAttributes','libraryV3'].map(name=>[name,{hash:'a'.repeat(64)}]));hashes.totp={code:'123456',version:61};
 const result=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{},hashes)));
 assert.equal(result.items.length,48);assert.equal(result.items[0].title,'Untitled playlist');
 assert.equal(result.items[0].id,'playlist00000000000000000000000000000000');assert.equal(result.accountId,'account-123');
 assert.equal(result.items[0].total,0);assert.equal(result.items[0].isPublic,false);assert.equal(result.items[0].visibility,null);
 assert.equal(result.items[1].total,7);assert.equal(result.items[1].visibility,'private');assert.equal(result.items[2].total,null);
 assert.equal(result.nextCursor,'50');
 assert.equal(result.unsupportedCollectionCount,1);assert.equal(result.complete,false);assert.equal(result.truncated,true);
 assert.equal(result.items.some(item=>item.id==='tracks'||item.url?.includes('/playlist/tracks')),false);
 const publicResult=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'https://open.spotify.com/user/owner123/playlists'},hashes)));
 assert.equal(publicResult.items[0].id,'37i9dQZF1DXcBWIGoYBM5M');
 assert.equal(publicResult.items[0].ownerId,'owner123');
 assert.equal(publicResult.items[0].ownerUrl,'https://open.spotify.com/user/owner123');
 assert.equal(publicResult.items[0].artworkUrl,'https://i.scdn.co/image/profile-playlist');
 assert.equal(publicResult.totalCount,2);assert.equal(publicResult.truncated,false);assert.equal(publicResult.complete,true);assert.equal(publicResult.nextCursor,null);
 assert.match(requests[0].url,/spclient\.wg\.spotify\.com\/user-profile-view\/v3\/profile\/owner123\/playlists\?/);
 assert.match(requests[0].url,/offset=0/);assert.match(requests[0].url,/limit=25/);
 assert.equal(requests[0].options.credentials,'omit');assert.equal(requests[0].options.redirect,'error');assert.match(new Headers(requests[0].options.headers).get('authorization'),/^Bearer /);assert.equal(new Headers(requests[0].options.headers).has('cookie'),false);
 await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'https://open.spotify.com/user/%C3%BCser%2Bname/playlists'},hashes);
 assert.match(requests[1].url,/profile\/%C3%BCser%2Bname\/playlists\?/);
 await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'spotify:user:üser+name'},hashes);
 assert.match(requests[2].url,/profile\/%C3%BCser%2Bname\/playlists\?/);
 const invalid=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'https://example.com/user/owner123'},hashes)));
 assert.equal(invalid.__burrataProviderError.code,'UNSUPPORTED');
 assert.equal(requests.length,3);
 profileOverride={uri:'spotify:user:owner123',name:'Owner',total_public_playlists_count:110,public_playlists:Array.from({length:99},(_,index)=>({uri:`spotify:playlist:${String(index).padStart(22,'0')}`,name:`Playlist ${index}`}))};
 const summary=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',inspectProfileResponse:true,profileSummary:true,playlistLimit:200},hashes)));
 assert.deepEqual(summary.items,[]);assert.equal(summary.profileResponseShape.responseMode,'summary');assert.equal(summary.profileResponseShape.parseClass,'valid');assert.equal(summary.profileResponseShape.decodedRows,99);assert.equal(summary.profileResponseShape.decodedTotal,110);assert.equal(summary.profileResponseShape.failureClass,'page_count');assert.equal(summary.profileResponseShape.playlistIdHashes.length,99);
 const summaryUrl=new URL(requests.at(-1).url);assert.equal(summaryUrl.pathname,'/user-profile-view/v3/profile/owner123');assert.equal(summaryUrl.searchParams.get('playlist_limit'),'200');assert.equal(summaryUrl.searchParams.get('artist_limit'),'0');assert.equal(summaryUrl.searchParams.get('episode_limit'),'0');assert.equal(summaryUrl.searchParams.get('market'),'from_token');assert.equal(requests.at(-1).options.credentials,'omit');assert.equal(new Headers(requests.at(-1).options.headers).has('cookie'),false);
 assert.doesNotMatch(JSON.stringify(summary),/owner123|fixture-account|cached-access-token|spotify:playlist|https?:/i);
 profileSummaryOverride=new Uint8Array([...profileInt(8,1),...profileInt(9,110)]);const malformedSummary=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',inspectProfileResponse:true,profileSummary:true,playlistLimit:200},hashes)));assert.equal(malformedSummary.profileResponseShape.parseClass,'malformed');assert.equal(malformedSummary.profileResponseShape.failureClass,'response_shape');
 profileSummaryOverride=new Uint8Array();const emptySummary=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',inspectProfileResponse:true,profileSummary:true,playlistLimit:200},hashes)));assert.equal(emptySummary.profileResponseShape.parseClass,'valid');assert.equal(emptySummary.profileResponseShape.decodedRows,0);assert.equal(emptySummary.profileResponseShape.decodedTotal,0);
 profileSummaryOverride=null;
 const requestCountBeforeRejectedSummary=requests.length;const rejectedSummary=await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',inspectProfileResponse:true,profileSummary:true,playlistLimit:201},hashes);assert.equal(rejectedSummary.__burrataProviderError.code,'UNSUPPORTED');assert.equal(requests.length,requestCountBeforeRejectedSummary);
 profileOverride={uri:'spotify:user:owner123',total_public_playlists_count:1,public_playlists:[{uri:'spotify:user:owner123:playlist:37i9dQZF1DXcBWIGoYBM5M',name:'Legacy playlist'}]};const legacy=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123'},hashes)));
 assert.equal(legacy.items.length,1);assert.equal(legacy.items[0].id,'37i9dQZF1DXcBWIGoYBM5M');assert.equal(legacy.items[0].url,'https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M');assert.equal(legacy.items[0].canEdit,false);assert.equal(legacy.complete,true);
 profileOverride={uri:'spotify:user:owner123',name:'Profile name',total_public_playlists_count:4,public_playlists:[{uri:'spotify:playlist:37i9dQZF1DXcBWIGoYBM5M',name:'Followed playlist'},{uri:'spotify:playlist:0000000000000000000000',name:'Explicit owner',owner_uri:'spotify:user:creator456',owner_name:'Creator'},{uri:'spotify:user:legacy789:playlist:1111111111111111111111',name:'Legacy owner'},{uri:'spotify:playlist:2222222222222222222222',owner_uri:'https://open.spotify.com/user/wrong'}]};const followed=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123'},hashes)));
 assert.deepEqual(followed.items.map(item=>({ownerId:item.ownerId,ownerUrl:item.ownerUrl,ownerName:item.ownerName})),[{ownerId:null,ownerUrl:null,ownerName:null},{ownerId:'creator456',ownerUrl:'https://open.spotify.com/user/creator456',ownerName:'Creator'},{ownerId:'legacy789',ownerUrl:'https://open.spotify.com/user/legacy789',ownerName:null},{ownerId:null,ownerUrl:null,ownerName:null}]);
 profileOverride={uri:'spotify:user:owner123',total_public_playlists_count:2,public_playlists:[{uri:'spotify:playlist:37i9dQZF1DXcBWIGoYBM5M',name:'Known'}, {uri:'spotify:playlist:malformed'}]};const incomplete=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123'},hashes)));
 assert.equal(incomplete.__burrataProviderError.code,'PROVIDER_CHANGED');
 const unsupportedCursor=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',cursor:'25'},hashes)));
 assert.equal(unsupportedCursor.__burrataProviderError.code,'UNSUPPORTED');assert.equal(requests.length,9);
 const manyRows=Array.from({length:90},(_,index)=>({uri:`spotify:playlist:${String(index).padStart(22,'0')}`,name:`Playlist ${index}`}));profileOverride={uri:'spotify:user:owner123',total_public_playlists_count:90,public_playlists:manyRows};
 const page1=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',limit:30},hashes)));
 assert.equal(page1.items.length,30);assert.equal(page1.complete,false);assert.ok(page1.nextCursor.startsWith('sp-profile-v5:30:90:'));assert.equal(page1.snapshotConsistency,'progressive');
 const originalFirst=manyRows[0].uri;manyRows[0]={...manyRows[0],uri:'spotify:playlist:9999999999999999999999'};const olderMutation=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',limit:30,cursor:page1.nextCursor},hashes)));assert.equal(olderMutation.__burrataProviderError.code,'PROVIDER_CHANGED');manyRows[0]={...manyRows[0],uri:originalFirst};
 const originalBoundary=manyRows[29].uri;manyRows[29]={...manyRows[29],uri:'spotify:playlist:9999999999999999999998'};const changedBoundary=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',limit:30,cursor:page1.nextCursor},hashes)));assert.equal(changedBoundary.__burrataProviderError.code,'PROVIDER_CHANGED');manyRows[29]={...manyRows[29],uri:originalBoundary};
 const page2=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',limit:30,cursor:page1.nextCursor},hashes)));
 assert.equal(page2.items.length,30);assert.equal(page2.items[0].id,String(30).padStart(22,'0'));assert.ok(page2.nextCursor.startsWith('sp-profile-v5:60:90:'));
 const page3=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',limit:30,cursor:page2.nextCursor},hashes)));
 assert.equal(page3.items.length,30);assert.equal(page3.items[0].id,String(60).padStart(22,'0'));assert.equal(page3.nextCursor,null);assert.equal(page3.complete,true);
 profileOverride={uri:'spotify:user:owner123',total_public_playlists_count:90,public_playlists:manyRows.slice(0,40)};
 const shortGrowing=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',limit:30,cursor:page1.nextCursor},hashes)));
 assert.equal(shortGrowing.__burrataProviderError,undefined);assert.equal(shortGrowing.items.length,10);assert.equal(shortGrowing.truncated,true);assert.equal(shortGrowing.complete,false);
 profileOverride={uri:'spotify:user:owner123',total_public_playlists_count:90,public_playlists:manyRows.slice(0,30)};
 const stopped=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',limit:30,cursor:page1.nextCursor},hashes)));
 assert.equal(stopped.__burrataProviderError,undefined);assert.equal(stopped.items.length,0);assert.equal(stopped.truncated,true);assert.equal(stopped.complete,false);
 const omittedRows=manyRows.slice(0,60).map(row=>({...row}));omittedRows[0].uri='spotify:playlist:malformed';
 profileOverride={uri:'spotify:user:owner123',total_public_playlists_count:60,public_playlists:omittedRows};
 const omittedFirst=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',limit:30},hashes)));
 assert.equal(omittedFirst.__burrataProviderError.code,'PROVIDER_CHANGED');
 const wrongOwner=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'other-owner',limit:30,cursor:page1.nextCursor},hashes)));
 assert.equal(wrongOwner.__burrataProviderError.code,'CURSOR_RESET_REQUIRED');
 const badCursor=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',cursor:'30'},hashes)));
 assert.equal(badCursor.__burrataProviderError.code,'UNSUPPORTED');
 const legacyCursor=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',cursor:`sp-profile-v2:30:90:${'a'.repeat(64)}:${'b'.repeat(64)}:${'c'.repeat(64)}`},hashes)));assert.equal(legacyCursor.__burrataProviderError.code,'CURSOR_RESET_REQUIRED');
 profileOverride={uri:'spotify:user:owner123',total_public_playlists_count:1200,public_playlists:Array.from({length:1200},(_,index)=>({uri:`spotify:playlist:${String(index).padStart(22,'0')}`}))};let progressive=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',limit:50},hashes)));for(let page=1;page<24;page++)progressive=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',limit:50,cursor:progressive.nextCursor},hashes)));assert.equal(progressive.items.length,50);assert.equal(progressive.nextCursor,null);assert.equal(progressive.truncated,false);assert.equal(progressive.complete,true);
 profileOverride={uri:'spotify:user:owner123',public_playlists:[{uri:'spotify:playlist:37i9dQZF1DXcBWIGoYBM5M'}]};const unknownTotal=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',limit:1},hashes)));assert.equal(unknownTotal.items.length,1);assert.equal(unknownTotal.totalCount,null);assert.ok(unknownTotal.nextCursor);assert.equal(unknownTotal.complete,false);
 profileOverride={uri:'spotify:user:owner123',total_public_playlists_count:1,public_playlists:[{uri:'spotify:playlist:37i9dQZF1DXcBWIGoYBM5M'},{uri:'spotify:playlist:0000000000000000000000'}]};const inconsistent=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123',limit:50},hashes)));assert.equal(inconsistent.items.length,2);assert.equal(inconsistent.totalCount,1);assert.equal(inconsistent.truncated,true);assert.equal(inconsistent.complete,false);
 // The paged endpoint does not echo profile identity. Identity binding is enforced by the owner-bound cursor.
 profileOverride={uri:'spotify:user:owner123'};const emptyWithoutTotal=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123'},hashes)));assert.equal(emptyWithoutTotal.items.length,0);assert.equal(emptyWithoutTotal.totalCount,0);assert.equal(emptyWithoutTotal.complete,true);
 profileOverride={uri:'spotify:user:owner123',public_playlists:[{uri:'spotify:playlist:bad-id'}]};const invalidRows=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123'},hashes)));assert.equal(invalidRows.__burrataProviderError.code,'PROVIDER_CHANGED');
 profileOverride={uri:'spotify:user:owner123',total_public_playlists_count:1,public_playlists:[{uri:'spotify:playlist:37i9dQZF1DXcBWIGoYBM5M',image_url:'http://insecure.example/art.jpg'}]};const unsafeArtwork=JSON.parse(JSON.stringify(await context.BurrataSpotifyProvider.run('spotify','listPlaylists',{ownerId:'owner123'},hashes)));assert.equal(unsafeArtwork.items[0].artworkUrl,null);
});

test('YouTube own library parses the fixed playlist feed and resolves public handles',async()=>{
 const calls=[];
 const response=body=>({ok:true,status:200,text:async()=>JSON.stringify(body)});
 const playlistResult={header:{menu:{continuationItemRenderer:{continuationEndpoint:{continuationCommand:{token:'unrelated-menu-token'}}}}},contents:{sectionListRenderer:{contents:[{itemSectionRenderer:{contents:[{lockupViewModel:{contentId:'PLuserPlaylist123',contentType:'LOCKUP_CONTENT_TYPE_PLAYLIST',metadata:{lockupMetadataViewModel:{title:{content:'User playlist'}}}}}] }},{continuationItemViewModel:{continuationCommand:{innertubeCommand:{continuationCommand:{token:'next-own-library'}}}}}]}}};
 const fetch=async(url,options={})=>{
  calls.push({url:String(url),options,body:options.body?JSON.parse(options.body):null});
  if(String(url)==='/feed/playlists')return {ok:true,status:200,text:async()=>`var ytInitialData = ${JSON.stringify(playlistResult)};`};
  return String(url).includes('navigation/resolve_url')?response({endpoint:{browseEndpoint:{browseId:'UCresolvedPublicChannel'}}}):response(playlistResult);
 };
 const chrome={storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async()=>{}}},runtime:{onMessage:{addListener(){},lastError:null}},action:{setBadgeText(){},setBadgeBackgroundColor(){}},tabs:{onRemoved:{addListener(){}}},alarms:{create(){},onAlarm:{addListener(){}}}};
 const context=vm.createContext({chrome,fetch,crypto:globalThis.crypto,URL,URLSearchParams,Response,TextEncoder,AbortController,location:{hostname:'www.youtube.com',origin:'https://www.youtube.com'},document:{cookie:'SAPISID=browser-session-cookie',querySelector:()=>null},ytcfg:{data_:{INNERTUBE_API_KEY:'account-api-key',INNERTUBE_CLIENT_NAME:'WEB',INNERTUBE_CLIENT_VERSION:'1.20261001.00.00',LOGGED_IN:true,DATASYNC_ID:'V1-user-account||opaque-token'}},setInterval(){},setTimeout,clearTimeout});vm.runInContext(source,context);
 const own=JSON.parse(JSON.stringify(await context.runProviderPageOperation('youtube','listPlaylists',{},{})));
 assert.equal(calls[0].url,'/feed/playlists');assert.equal(calls[0].options.credentials,'include');assert.equal(calls[0].body,null);assert.equal(own.items[0].title,'User playlist');assert.equal(own.accountId,'V1-user-account');assert.equal(own.nextCursor,'next-own-library');
 calls.length=0;
 const publicList=JSON.parse(JSON.stringify(await context.runProviderPageOperation('youtube','listPlaylists',{ownerId:'https://www.youtube.com/@publichandle/playlists'},{})));
 assert.match(calls[0].url,/navigation\/resolve_url/);assert.equal(calls[1].body.browseId,'UCresolvedPublicChannel');
 assert.equal(publicList.items[0].ownerId,'https://www.youtube.com/@publichandle/playlists');assert.equal(publicList.items[0].title,'User playlist');
});

test('YouTube bootstrap resolves an omitted channel handle from the explicit active Music header',async()=>{
 const calls=[],channelId='UCaaaaaaaaaaaaaaaaaaaaaa';
 const selected={isSelected:true,accountName:{runs:[{text:'Fixture account'}]},serviceEndpoint:{selectActiveIdentityEndpoint:{supportedTokens:[{pageIdToken:{pageId:'123456789012345'}}]}}};
 const fetchImpl=async(url,options={})=>{
  const target=String(url); const body=options.body?JSON.parse(options.body):null; calls.push({target,body});
  if(options.method==='GET')return {ok:true,status:200,headers:new Headers(),text:async()=>`ytcfg.set({"INNERTUBE_CLIENT_NAME":"WEB_REMIX","INNERTUBE_CLIENT_VERSION":"1.20261001.00.00","INNERTUBE_API_KEY":"fixtureApiKey123456","VISITOR_DATA":"fixtureVisitor","SESSION_INDEX":0,"LOGGED_IN":true});`};
  const endpoint=target.split('/youtubei/v1/')[1].split('?')[0];
  if(endpoint==='account/accounts_list')return {ok:true,status:200,headers:new Headers(),text:async()=>JSON.stringify({contents:[{accountItemSectionRenderer:{contents:[{accountItem:selected}]}}]})};
  if(endpoint==='account/account_menu')return {ok:true,status:200,headers:new Headers(),text:async()=>JSON.stringify({actions:[{openPopupAction:{popup:{multiPageMenuRenderer:{header:{activeAccountHeaderRenderer:{accountName:{runs:[{text:'Fixture account'}]},channelHandle:{runs:[{text:'@fixture_handle'}]}}}}}}}]})};
  if(endpoint==='navigation/resolve_url'&&body.url.endsWith('/@fixture_handle'))return {ok:true,status:200,headers:new Headers(),text:async()=>JSON.stringify({endpoint:{urlEndpoint:{url:'https://www.youtube.com/channel/fixture_handle?feature=accounts'}}})};
  if(endpoint==='navigation/resolve_url'&&body.url==='https://www.youtube.com/channel/fixture_handle')return {ok:true,status:200,headers:new Headers(),text:async()=>JSON.stringify({endpoint:{browseEndpoint:{browseId:channelId}}})};
  throw new Error(`Unexpected fixture request: ${endpoint}`);
 };
 const context=vm.createContext({fetch,crypto:webcrypto,URL,URLSearchParams,TextEncoder,AbortController,Headers,setTimeout,clearTimeout});
 vm.runInContext(youtubeSessionSource,context);
 const result=JSON.parse(JSON.stringify(await context.BurrataYouTubeSessionProvider.run('bootstrap',{}, {origin:'https://music.youtube.com',cookieHeader:'SAPISID=fixture-session-cookie'}, {fetchImpl,now:()=>1_800_000_000_000})));
 assert.equal(result.accountId,channelId);
 assert.equal(result.displayName,'Fixture account');
 assert.deepEqual(calls.filter(call=>call.target.includes('navigation/resolve_url')).map(call=>call.body.url),['https://www.youtube.com/@fixture_handle','https://www.youtube.com/channel/fixture_handle']);
 assert.equal(calls.at(-1).body.context.user.onBehalfOfUser,undefined);
});

test('YouTube bootstrap rejects unresolved channel identity without leaking account values or accepting page IDs',async()=>{
 const accountName='Private fixture display name';
 const selected={isSelected:true,accountName:{runs:[{text:accountName}]},serviceEndpoint:{selectActiveIdentityEndpoint:{supportedTokens:[{pageIdToken:{pageId:'123456789012345'}}]}}};
 const fetchImpl=async(url,options={})=>{
  const target=String(url),body=options.body?JSON.parse(options.body):null;
  if(options.method==='GET')return {ok:true,status:200,headers:new Headers(),text:async()=>`ytcfg.set({"INNERTUBE_CLIENT_NAME":"WEB_REMIX","INNERTUBE_CLIENT_VERSION":"1.20261001.00.00","INNERTUBE_API_KEY":"fixtureApiKey123456","VISITOR_DATA":"fixtureVisitor","DATASYNC_ID":"fixtureDataSync","SESSION_INDEX":0,"LOGGED_IN":true});`};
  const endpoint=target.split('/youtubei/v1/')[1].split('?')[0];
  if(endpoint==='account/accounts_list')return {ok:true,status:200,headers:new Headers(),text:async()=>JSON.stringify({contents:[{accountItemSectionRenderer:{contents:[{accountItem:selected}]}}]})};
  if(endpoint==='account/account_menu')return {ok:true,status:200,headers:new Headers(),text:async()=>JSON.stringify({actions:[{openPopupAction:{popup:{multiPageMenuRenderer:{header:{activeAccountHeaderRenderer:{accountName:{runs:[{text:accountName}]}}}}}}}]})};
  throw new Error(`Unexpected fixture request: ${endpoint}`);
 };
 const context=vm.createContext({fetch,crypto:webcrypto,URL,URLSearchParams,TextEncoder,AbortController,Headers,setTimeout,clearTimeout});vm.runInContext(youtubeSessionSource,context);
 await assert.rejects(context.BurrataYouTubeSessionProvider.run('bootstrap',{}, {origin:'https://music.youtube.com',cookieHeader:'SAPISID=fixture-session-cookie'}, {fetchImpl,now:()=>1_800_000_000_000}),error=>{
  assert.equal(error.code,'PROVIDER_CHANGED');assert.equal(error.phase,'channel_identity');assert.equal(error.diagnostics.shape.activeHeader,true);assert.equal(error.diagnostics.shape.headerHandle,false);assert.equal(error.diagnostics.shape.pageIdToken,true);assert.equal(JSON.stringify(error.diagnostics).includes(accountName),false);return true;
 });
});

test('YouTube channel identity rejects an unsafe urlEndpoint redirect without following it',async()=>{
 const calls=[],selected={isSelected:true,channelHandle:{runs:[{text:'@fixture_handle'}]}};let redirectUrl='https://attacker.example/steal';
 const fetchImpl=async(url,options={})=>{
  const target=String(url),body=options.body?JSON.parse(options.body):null;calls.push(target);
  if(options.method==='GET')return {ok:true,status:200,headers:new Headers(),text:async()=>`ytcfg.set({"INNERTUBE_CLIENT_NAME":"WEB_REMIX","INNERTUBE_CLIENT_VERSION":"1.20261001.00.00","INNERTUBE_API_KEY":"fixtureApiKey123456","VISITOR_DATA":"fixtureVisitor","DATASYNC_ID":"fixtureDataSync","SESSION_INDEX":0,"LOGGED_IN":true});`};
  const endpoint=target.split('/youtubei/v1/')[1].split('?')[0];
  if(endpoint==='account/accounts_list')return {ok:true,status:200,headers:new Headers(),text:async()=>JSON.stringify({contents:[{accountItemSectionRenderer:{contents:[{accountItem:selected}]}}]})};
  if(endpoint==='navigation/resolve_url')return {ok:true,status:200,headers:new Headers(),text:async()=>JSON.stringify({endpoint:{urlEndpoint:{url:redirectUrl}}})};
  if(endpoint==='account/account_menu')return {ok:true,status:200,headers:new Headers(),text:async()=>JSON.stringify({actions:[]})};
  throw new Error(`Unexpected fixture request: ${endpoint}`);
 };
 const context=vm.createContext({fetch,crypto:webcrypto,URL,URLSearchParams,TextEncoder,AbortController,Headers,setTimeout,clearTimeout});vm.runInContext(youtubeSessionSource,context);
 for (redirectUrl of ['https://attacker.example/steal','https://www.youtube.com/@fixture%252F..','https://www.youtube.com/@fixture/../../other']) {
  calls.length=0;
  await assert.rejects(context.BurrataYouTubeSessionProvider.run('bootstrap',{}, {origin:'https://music.youtube.com',cookieHeader:'SAPISID=fixture-session-cookie'}, {fetchImpl,now:()=>1_800_000_000_000}),error=>{
   assert.equal(error.code,'PROVIDER_CHANGED');assert.equal(error.phase,'channel_identity');assert.equal(error.diagnostics.shape.urlEndpoint,true);assert.equal(error.diagnostics.shape.secondResolve,false);return true;
  });
  assert.equal(calls.filter(url=>url.includes('navigation/resolve_url')).length,1);assert.equal(calls.some(url=>url.startsWith('https://attacker.example/')),false);
 }
});

test('YouTube bootstrap does not borrow a different active-header handle',async()=>{
 const calls=[],selected={isSelected:true,channelHandle:{runs:[{text:'@selected_handle'}]}};
 const fetchImpl=async(url,options={})=>{
  const target=String(url),body=options.body?JSON.parse(options.body):null;calls.push({target,body});
  if(options.method==='GET')return {ok:true,status:200,headers:new Headers(),text:async()=>`ytcfg.set({"INNERTUBE_CLIENT_NAME":"WEB_REMIX","INNERTUBE_CLIENT_VERSION":"1.20261001.00.00","INNERTUBE_API_KEY":"fixtureApiKey123456","VISITOR_DATA":"fixtureVisitor","SESSION_INDEX":0,"LOGGED_IN":true});`};
  const endpoint=target.split('/youtubei/v1/')[1].split('?')[0];
  if(endpoint==='account/accounts_list')return {ok:true,status:200,headers:new Headers(),text:async()=>JSON.stringify({contents:[{accountItemSectionRenderer:{contents:[{accountItem:selected}]}}]})};
  if(endpoint==='navigation/resolve_url')return {ok:true,status:200,headers:new Headers(),text:async()=>JSON.stringify({endpoint:{browseEndpoint:{browseId:'not-a-channel'}}})};
  if(endpoint==='account/account_menu')return {ok:true,status:200,headers:new Headers(),text:async()=>JSON.stringify({actions:[{openPopupAction:{popup:{multiPageMenuRenderer:{header:{activeAccountHeaderRenderer:{channelHandle:{runs:[{text:'@different_handle'}]}}}}}}}]})};
  throw new Error(`Unexpected fixture request: ${endpoint}`);
 };
 const context=vm.createContext({fetch,crypto:webcrypto,URL,URLSearchParams,TextEncoder,AbortController,Headers,setTimeout,clearTimeout});vm.runInContext(youtubeSessionSource,context);
 await assert.rejects(context.BurrataYouTubeSessionProvider.run('bootstrap',{}, {origin:'https://music.youtube.com',cookieHeader:'SAPISID=fixture-session-cookie'}, {fetchImpl,now:()=>1_800_000_000_000}),error=>{
  assert.equal(error.code,'PROVIDER_CHANGED');assert.equal(error.phase,'channel_identity');assert.equal(error.diagnostics.shape.headerHandle,true);return true;
 });
 assert.equal(calls.filter(call=>call.target.includes('navigation/resolve_url')).length,1);
});
