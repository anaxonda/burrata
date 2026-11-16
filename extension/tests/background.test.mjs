import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

const source=readFileSync(new URL('../background.js',import.meta.url),'utf8');
function worker(session={}, {enabled=false, verification=false, requests=[],pageUrl='https://www.beatport.com/genre/afro-house/89/top-100',albumCollection=false,handoffSession=null,youtubePage=null,accountIdentity=null,providerFailure=null,providerResponse={},targetCollectionId='',contextSelection=null,nativeReplies=[],deferDisconnect=false}={}) {
  let listener, removed;
  let menuClick;
  let connectionCount=0,responseIndex=0;
  const messages=[], created=[], updated=[], scripts=[],menus=[],local={enabled,targetCollectionId},ports=[];
  const host=new URL(pageUrl).hostname;
  const snapshot={provider:host.includes('shazam')?'shazam':host.includes('last.fm')?'lastfm':host.includes('bandcamp')?'bandcamp':host.includes('soundcloud')?'soundcloud':host.includes('youtube')?'youtube':'beatport',kind:host.includes('youtube')?'playlist':'chart',sourceUrl:pageUrl,entries:[]};
  const api={
    action:{setBadgeText(){},setBadgeBackgroundColor(){}},
    storage:{local:{get:async()=>local,set:async value=>Object.assign(local,value)},session:{get:async()=>session,set:async value=>Object.assign(session,value)}},
    runtime:{getURL:path=>`chrome-extension://test/${path}`,onInstalled:{addListener(){}},onMessage:{addListener(fn){listener=fn;}},connectNative(){connectionCount++;let disconnected,reply;const port={
      onMessage:{addListener(fn){reply=fn;}},onDisconnect:{addListener(fn){disconnected=fn;}},disconnect(){if(!deferDisconnect)disconnected?.();},
      postMessage(message){messages.push(message);const custom=nativeReplies[responseIndex++];queueMicrotask(()=>reply(custom||{ok:true,requests:message.type==='poll'?requests:[],collections:[{id:'playlist-1',name:'Fixture'}],result:{collectionId:'playlist-1',collectionName:'Fixture',artist:'Artist',trackTitle:'Song'}}));},
    };ports.push({fireDisconnect:()=>disconnected?.()});return port;}},
    scripting:{executeScript:async details=>{scripts.push(details);return details.world==='MAIN'?[{result:details.func?.name==='captureYouTubeAccountIdentity'?{account:accountIdentity||{accountId:handoffSession?.accountId||'UCaaaaaaaaaaaaaaaaaaaaaa',displayName:handoffSession?.displayName||'Fixture Channel'}}:details.func?.name==='captureYouTubeSessionHandoff'?(handoffSession?{session:handoffSession}:details.func()):providerFailure?{__burrataProviderError:providerFailure}:{accountId:'browser-account',displayName:'Test'}}]:[];}},
    tabs:{get:async id=>({id,status:'complete',url:pageUrl,windowId:1}),
      query:async()=>[{id:8,url:pageUrl}],create:async value=>{created.push(value);return {id:9};},
      update:async(id,value)=>{updated.push({id,...value});return {windowId:1};},
      sendMessage:async(_id,message)=>message?.type==='captureTrackForPlaylist'?(contextSelection||{error:'No track fixture'}):verification?{verification:true}:albumCollection?{albumCollection:{sourceUrl:'https://bandcamp.com/discover/hip-hop-rap',title:'Hip-hop',albumUrls:['https://artist.bandcamp.com/album/record']}}:{snapshot},onRemoved:{addListener(fn){removed=fn;}}},
    windows:{update:async()=>{}},permissions:{contains:async()=>true},
    contextMenus:{removeAll:async()=>{},create:menu=>menus.push(menu),onClicked:{addListener(fn){menuClick=fn;}}},
    alarms:{create(){},onAlarm:{addListener(){}}},
  };
  const context=vm.createContext({chrome:api,URL,TextEncoder,Map,Promise,AbortController,crypto:{randomUUID:()=> 'test-id'},BurrataSpotifyAuth:{loadRegistry:async()=>({secret:'fixture',version:'fixture'}),totp:async()=> '123456'},location:youtubePage?.location||{origin:new URL(pageUrl).origin,hostname:new URL(pageUrl).hostname,pathname:new URL(pageUrl).pathname},document:youtubePage?.document||{cookie:'',querySelector:()=>null},fetch:async()=>({ok:true,status:200,text:async()=>JSON.stringify(providerResponse)}),setInterval(){},setTimeout(fn,ms){if(ms<30000)queueMicrotask(fn);return 1;},clearTimeout(){}});
  vm.runInContext(source,context);
  return {messages,created,updated,scripts,menus,session,removed,local,get connectionCount(){return connectionCount;},clickMenu:(info,tab)=>menuClick(info,tab),fireDisconnect:index=>ports[index]?.fireDisconnect(),
    send:(message,sender={})=>new Promise(resolve=>listener(message,sender,resolve)),
    poll:()=>vm.runInContext('poll()',context),context,
  };
}
const youtubeArtistRun=(name)=>({text:name,navigationEndpoint:{browseEndpoint:{browseEndpointContextSupportedConfigs:{browseEndpointContextMusicConfig:{pageType:'MUSIC_PAGE_TYPE_ARTIST'}}}}});
const youtubeTrackColumn=(runs)=>({musicResponsiveListItemFlexColumnRenderer:{text:{runs}}});
test('YouTube video search keeps the channel byline out of artist metadata',async()=>{
 const response={contents:{videoRenderer:{videoId:'abcdefghijk',title:{simpleText:'Song'},ownerText:{runs:[{text:'Uploader Channel'}]},lengthText:{simpleText:'3:12'}}}};
 const w=worker({}, {pageUrl:'https://www.youtube.com/results?search_query=song',providerResponse:response});
 const result=await w.context.runProviderPageOperation('youtube','search',{query:'song',kind:'track'},{});
 const item=JSON.parse(JSON.stringify(result.items[0]));
 assert.equal(item.data.artist,'');assert.equal(item.data.channelName,'Uploader Channel');
});
test('YouTube Music search accepts typed artist runs and keeps untyped bylines separate',async()=>{
 const response={contents:{musicResponsiveListItemRenderer:{playlistItemData:{videoId:'abcdefghijk'},flexColumns:[youtubeTrackColumn([{text:'Song',navigationEndpoint:{watchEndpoint:{videoId:'abcdefghijk'}}}]),youtubeTrackColumn([{text:'Earth, Wind & Fire',navigationEndpoint:{browseEndpoint:{browseEndpointContextSupportedConfigs:{browseEndpointContextMusicConfig:{pageType:'MUSIC_PAGE_TYPE_ARTIST'}}}}},{text:'Uploader Channel'}])],fixedColumns:[{musicResponsiveListItemFixedColumnRenderer:{text:{simpleText:'3:12'}}}]}}};
 const w=worker({}, {pageUrl:'https://music.youtube.com/search?q=song',providerResponse:response});
 const result=await w.context.runProviderPageOperation('youtube','search',{query:'song',kind:'track'},{});
 const item=JSON.parse(JSON.stringify(result.items[0]));
 assert.equal(item.data.artist,'Earth, Wind & Fire');assert.equal(item.data.channelName,'Uploader Channel');
});
test('YouTube playlist rows do not promote uploaders or ambiguous music columns to artists',async()=>{
 const response={contents:{playlistVideoRenderer:{videoId:'abcdefghijk',title:{simpleText:'Video'},shortBylineText:{simpleText:'Uploader Channel'},lengthText:{simpleText:'3:12'}},musicResponsiveListItemRenderer:{playlistItemData:{videoId:'BCDEFGHIJKL'},flexColumns:[youtubeTrackColumn([{text:'Music Song',navigationEndpoint:{watchEndpoint:{videoId:'BCDEFGHIJKL'}}}]),youtubeTrackColumn([{text:'Ambiguous Channel and album'}])],fixedColumns:[{musicResponsiveListItemFixedColumnRenderer:{text:{simpleText:'4:05'}}}]}}};
 const w=worker({}, {pageUrl:'https://www.youtube.com/playlist?list=PLabcdefghijk',providerResponse:response});
 const result=await w.context.runProviderPageOperation('youtube','getPlaylist',{playlistId:'PLabcdefghijk'},{});
 const entries=JSON.parse(JSON.stringify(result.entries));
 assert.deepEqual(entries.map(entry=>[entry.artist,entry.channelName]),[['','Uploader Channel'],['','Ambiguous Channel and album']]);
});
test('Chromium worker restart retains request association for manual import',async()=>{
  const w=worker({requestTabs:[[8,'pending-1']]});
  const result=await w.send({type:'importCurrent'});
  assert.equal(result.ok,true);
  assert.equal(w.messages[0].requestId,'pending-1');
  assert.equal(w.messages[0].type,'import');
  assert.equal(w.session.requestTabs.length,0);
});
test('app-driven import opens a new tab and retains verification request',async()=>{
  const url='https://www.beatport.com/genre/afro-house/89/top-100';
  const w=worker({}, {enabled:true,verification:true,requests:[{id:'pending-2',url}]});
  await w.poll();
  assert.equal(w.created.length,1);
  assert.equal(w.created[0].url,url);
  assert.ok(w.updated.every(value=>!('url' in value)));
  assert.equal(w.session.requestTabs[0][1],'pending-2');
  assert.ok(w.messages.some(m=>m.type==='status'&&m.state==='verification'));
});
test('closing a pending import tab reports failure and clears association',async()=>{
  const w=worker({requestTabs:[[8,'pending-3']]});
  await w.send({type:'status'});
  w.removed(8);
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(w.session.requestTabs.length,0);
  assert.ok(w.messages.some(m=>m.requestId==='pending-3'&&m.state==='failed'));
});

test('current-page capture accepts Shazam, Last.fm and Bandcamp provider hosts',async()=>{
  for(const pageUrl of ['https://www.shazam.com/charts/top-200/world','https://www.last.fm/charts','https://label.bandcamp.com/album/example','https://soundcloud.com/artist/public-track']){
    const w=worker({}, {pageUrl});
    const result=await w.send({type:'importCurrent'});
    assert.equal(result.ok,true,pageUrl);
    assert.equal(w.messages[0].snapshot.provider,new URL(pageUrl).hostname.includes('shazam')?'shazam':new URL(pageUrl).hostname.includes('last.fm')?'lastfm':new URL(pageUrl).hostname.includes('soundcloud')?'soundcloud':'bandcamp');
  }
});
test('track context menu persists a selected target and appends one parser-selected row',async()=>{
 const entry={providerId:'123',sourceUrl:'https://www.traxsource.com/track/123/hello',artist:'Artist',title:'Song',position:1};
 const selection={targetUrl:entry.sourceUrl,snapshot:{schemaVersion:1,provider:'traxsource',kind:'chart',sourceUrl:'https://www.traxsource.com/genre/13/deep-house/top',title:'Top',capturedAt:'2026-11-16T00:00:00Z',complete:false,expectedTotal:null,warnings:[],entries:[entry]}};
 const w=worker({}, {pageUrl:'https://www.traxsource.com/genre/13/deep-house/top',targetCollectionId:'playlist-1',contextSelection:selection});
 await new Promise(resolve=>setTimeout(resolve,0));
 assert.equal(w.menus[0].id,'burrata-add-track-to-playlist');assert.ok(w.menus[0].contexts.includes('link'));
 w.clickMenu({menuItemId:'burrata-add-track-to-playlist',pageUrl:'https://www.traxsource.com/genre/13/deep-house/top',linkUrl:entry.sourceUrl},{id:8,url:'https://www.traxsource.com/genre/13/deep-house/top'});
 await new Promise(resolve=>setTimeout(resolve,0));
 const append=w.messages.find(message=>message.type==='appendTrackToCollection');
 assert.equal(append.collectionId,'playlist-1');assert.equal(append.targetUrl,entry.sourceUrl);assert.equal(append.snapshot.entries.length,1);
 assert.equal(w.local.lastContextAction,'Added Artist — Song to Fixture');
 assert.equal(w.messages.some(message=>message.type==='import'),false);
});

test('track context menu asks for a playlist before parsing or sending data',async()=>{
 const w=worker({}, {pageUrl:'https://www.traxsource.com/genre/13/deep-house/top',targetCollectionId:''});
 await new Promise(resolve=>setTimeout(resolve,0));
 w.clickMenu({menuItemId:'burrata-add-track-to-playlist',pageUrl:'https://www.traxsource.com/genre/13/deep-house/top',linkUrl:'https://www.traxsource.com/track/123/hello'},{id:8,url:'https://www.traxsource.com/genre/13/deep-house/top'});
 await new Promise(resolve=>setTimeout(resolve,0));
 assert.equal(w.messages.some(message=>message.type==='appendTrackToCollection'),false);
 assert.match(w.local.lastContextAction,/Choose a Burrata playlist/);
});

test('popup collection selection is restricted to its own page and saved in extension storage',async()=>{
 const w=worker();
 const popupSender={url:'chrome-extension://test/popup.html'};
 const listed=await w.send({type:'listTargetCollections'},popupSender);
 assert.equal(listed.ok,true);assert.equal(listed.collections[0].id,'playlist-1');
 const selected=await w.send({type:'setTargetCollection',collectionId:'playlist-1'},popupSender);
 assert.equal(selected.ok,true);assert.equal(w.local.targetCollectionId,'playlist-1');
 const denied=await w.send({type:'setTargetCollection',collectionId:'other'},{url:'https://www.traxsource.com/'});
 assert.equal(denied.ok,false);
});

test('only an exact native schema error disconnects once and retries through a fresh host',async()=>{
 const sender={url:'chrome-extension://test/popup.html'};
 const recovered=worker({}, {nativeReplies:[{ok:false,error:'Invalid native request'},{ok:true,collections:[{id:'playlist-1',name:'Fixture'}]}]});
 const result=await recovered.send({type:'listTargetCollections'},sender);
 assert.equal(result.ok,true);assert.equal(recovered.connectionCount,2);
 assert.equal(recovered.messages.filter(message=>message.type==='listTargetCollections').length,2);

 const persistent=worker({}, {nativeReplies:[{ok:false,error:'Invalid native request'},{ok:false,error:'Invalid native request'}]});
 const failed=await persistent.send({type:'listTargetCollections'},sender);
 assert.equal(failed.ok,false);assert.match(failed.error,/browser host is outdated/);assert.equal(persistent.connectionCount,2);
});
test('a delayed stale-host disconnect cannot clear the replacement native port',async()=>{
 const w=worker({}, {deferDisconnect:true,nativeReplies:[{ok:false,error:'Invalid native request'},{ok:true,collections:[{id:'playlist-1',name:'Fixture'}]},{ok:true,collections:[{id:'playlist-1',name:'Fixture'}]}]});
 const sender={url:'chrome-extension://test/popup.html'};
 assert.equal((await w.send({type:'listTargetCollections'},sender)).ok,true);assert.equal(w.connectionCount,2);
 w.fireDisconnect(0);
 assert.equal((await w.send({type:'listTargetCollections'},sender)).ok,true);assert.equal(w.connectionCount,2);
});

test('generic bridge and mutation errors do not retry the native message',async()=>{
 const sender={url:'chrome-extension://test/popup.html'};
 for(const error of ['Burrata browser bridge unavailable','MUTATION_OUTCOME_UNKNOWN']){
  const w=worker({}, {nativeReplies:[{ok:false,error}]});
  const result=await w.send({type:'listTargetCollections'},sender);
  assert.equal(result.ok,false);assert.equal(w.connectionCount,1);assert.equal(w.messages.filter(message=>message.type==='listTargetCollections').length,1);
 }
});

test('Spotify track-credit read is routed to Spotify only and rejects unsupported operations',async()=>{
 const args={trackId:'2u9k9X8d1uD4qDLjMhVxU0',trackTitle:'Halka Halka (From "Fanney Khan")',mainArtistId:'3eDT9fwXKuHWFvgZaaYC5v',durationMilliseconds:247381,artistIds:['3eDT9fwXKuHWFvgZaaYC5v','0NErdIJtuKBjtxKmlqaWdj']};
 const request={id:'credit-read-1',kind:'providerOperation',operationId:'credit-op-1',provider:'spotify',operation:'getTrackCredits',args};
 const spotify=worker({}, {enabled:true,requests:[request],pageUrl:'https://open.spotify.com/'});
 await spotify.poll();
 const invoke=spotify.scripts.find(call=>typeof call.func==='function'&&Array.isArray(call.args));
 assert.ok(invoke,'the supported exact read reaches the injected Spotify provider adapter');
 assert.equal(invoke.args[0],'spotify');assert.equal(invoke.args[1],'getTrackCredits');assert.deepEqual(invoke.args[2],args);
 assert.ok(spotify.messages.some(message=>message.type==='poll'&&message.providerCapabilities.includes('spotifyTrackCreditsV1')),'the companion advertises support before receiving the read');
 assert.ok(spotify.messages.some(message=>message.type==='providerResult'&&message.operationId==='credit-op-1'&&message.ok));

 const youtube=worker({}, {enabled:true,requests:[{...request,id:'credit-read-2',operationId:'credit-op-2',provider:'youtube'}],pageUrl:'https://www.youtube.com/'});
 await youtube.poll();
 assert.equal(youtube.scripts.length,0,'a Spotify catalog operation is refused before provider-page execution on YouTube');
 assert.ok(youtube.messages.some(message=>message.type==='providerResult'&&message.operationId==='credit-op-2'&&!message.ok));

 const unknown=worker({}, {enabled:true,requests:[{...request,id:'credit-read-3',operationId:'credit-op-3',operation:'getTrackCreditsExtended'}],pageUrl:'https://open.spotify.com/'});
 await unknown.poll();
 assert.equal(unknown.scripts.length,0,'unrecognized provider operations never reach page execution');
 assert.ok(unknown.messages.some(message=>message.type==='providerResult'&&message.operationId==='credit-op-3'&&!message.ok));
});

test('current-page YouTube video import uses the rendered public snapshot without account status',async()=>{
 const pageUrl='https://www.youtube.com/watch?v=abcdefghijk';
 const w=worker({}, {pageUrl});
 const result=await w.send({type:'importCurrent'});
 assert.equal(result.ok,true);assert.equal(w.messages.length,1);
 assert.equal(w.messages[0].type,'import');assert.equal(w.messages[0].snapshot.provider,'youtube');
 assert.equal(w.messages[0].snapshot.sourceUrl,pageUrl);
 assert.equal(w.messages.some(message=>message.type==='status'&&message.state==='verification'),false);
});

test('Bandcamp Discover forwards album URLs as a native collection import',async()=>{
 const w=worker({}, {pageUrl:'https://bandcamp.com/discover/hip-hop-rap',albumCollection:true});
 const result=await w.send({type:'importCurrent'});
 assert.equal(result.ok,true);
 const request=w.messages.find(message=>message.type==='importAlbumCollection');
 assert.deepEqual(request.albumUrls,['https://artist.bandcamp.com/album/record']);
 assert.equal(request.sourceUrl,'https://bandcamp.com/discover/hip-hop-rap');
});

test('disabled app-driven mode sends companion heartbeat',async()=>{
 const w=worker({}, {enabled:false});await w.poll();
 const heartbeat=w.messages.find(message=>message.type==='heartbeat');assert.equal(heartbeat?.providerProtocolVersion,0);
});

test('enabled companion heartbeat advertises provider protocol support',async()=>{
 const w=worker({}, {enabled:true});await w.send({type:'status'});
 const heartbeat=w.messages.find(message=>message.type==='heartbeat');assert.equal(heartbeat?.providerProtocolVersion,2);
});

test('provider poll operation returns normalized browser-owned result with correlation IDs',async()=>{
 const request={id:'native-4',kind:'providerOperation',operationId:'op-4',provider:'spotify',operation:'status',args:{}};
 const w=worker({}, {enabled:true,pageUrl:'https://open.spotify.com/',requests:[request]});
 await w.poll();
 const poll=w.messages.find(message=>message.type==='poll');assert.equal(poll?.providerProtocolVersion,2);
 const result=w.messages.find(message=>message.type==='providerResult');
 assert.equal(result.requestId,'native-4');assert.equal(result.operationId,'op-4');
 assert.equal(result.ok,true);assert.deepEqual(result.result,{accountId:'browser-account',displayName:'Test'});
});

test('YouTube session is handed off only from the explicit extension popup action',async()=>{
 const session={origin:'https://www.youtube.com',cookieHeader:'SAPISID=one-session-cookie',accountId:'UCaaaaaaaaaaaaaaaaaaaaaa',displayName:'Fixture Channel',clientName:'WEB',clientVersion:'1.20261001.00.00',apiKey:'TEST-FIXTURE-CREDENTIAL-B194A13007-public-api-key-fixture',visitorData:'visitor-fixture',authUser:'0',hl:'en',gl:'US'};
 const pairingCode='43123-0123456789abcdef0123456789abcdef';
 const w=worker({}, {pageUrl:'https://www.youtube.com/feed/playlists',handoffSession:session});
 const refused=await w.send({type:'youtubeCheckHandoffAccount',pairingCode},{tab:{id:4},url:'https://www.youtube.com/'});
 assert.equal(refused.ok,false);
 assert.equal(w.messages.length,0);
 const noConfirmation=await w.send({type:'youtubeSessionHandoff',pairingCode,confirmedAccountId:session.accountId},{url:'chrome-extension://test/popup.html'});
 assert.equal(noConfirmation.ok,false);
 assert.equal(w.messages.length,0);
 const checked=await w.send({type:'youtubeCheckHandoffAccount',pairingCode},{url:'chrome-extension://test/popup.html'});
 assert.equal(checked.ok,true);
 assert.deepEqual(checked.account,{accountId:session.accountId,displayName:session.displayName});
 assert.equal(w.messages.length,0,'account-only confirmation must not hand off session data');
 const accepted=await w.send({type:'youtubeSessionHandoff',pairingCode,confirmedAccountId:session.accountId},{url:'chrome-extension://test/popup.html'});
 assert.equal(accepted.ok,true);
 const request=w.messages.find(message=>message.type==='youtubeSessionHandoff');
 assert.equal(request.requestId,'youtube-session-test-id');
 assert.equal(request.pairingCode,pairingCode);
 assert.deepEqual(request.session,session);
 assert.equal(JSON.stringify(w.session).includes('one-session-cookie'),false);
});

test('YouTube pairing rejects malformed, out-of-range and unconfirmed handoff codes before tab capture',async()=>{
 for(const pairingCode of ['43123', '65536-0123456789abcdef0123456789abcdef', '43123-0123456789abcdef0123456789abcg']){
  const w=worker({}, {pageUrl:'https://www.youtube.com/feed/playlists'});
  const result=await w.send({type:'youtubeCheckHandoffAccount',pairingCode},{url:'chrome-extension://test/popup.html'});
  assert.equal(result.ok,false,pairingCode);
  assert.equal(w.messages.length,0);
 }
});

test('YouTube handoff stops if the captured session account differs from the explicitly confirmed tab account',async()=>{
 const session={origin:'https://www.youtube.com',cookieHeader:'SAPISID=wrong-session-cookie',accountId:'UCbbbbbbbbbbbbbbbbbbbbbb',displayName:'Different Channel',clientName:'WEB',clientVersion:'1.20261001.00.00',apiKey:'TEST-FIXTURE-CREDENTIAL-B194A13007-public-api-key-fixture',visitorData:'visitor-fixture',authUser:'0',hl:'en',gl:'US'};
 const pairingCode='43123-0123456789abcdef0123456789abcdef';
 const w=worker({}, {pageUrl:'https://www.youtube.com/feed/playlists',handoffSession:session,accountIdentity:{accountId:'UCaaaaaaaaaaaaaaaaaaaaaa',displayName:'Confirmed Channel'}});
 const checked=await w.send({type:'youtubeCheckHandoffAccount',pairingCode},{url:'chrome-extension://test/popup.html'});
 assert.equal(checked.ok,true);
 const result=await w.send({type:'youtubeSessionHandoff',pairingCode,confirmedAccountId:checked.account.accountId},{url:'chrome-extension://test/popup.html'});
 assert.equal(result.ok,false);
 assert.equal(w.messages.length,0,'session data must not be forwarded when account confirmation changed');
});

test('duplicate completed mutation operation replays receipt without re-executing page code',async()=>{
 const response={type:'providerResult',requestId:'native-5',operationId:'op-5',ok:true,result:{receipt:{provider:'spotify',operation:'addTracks',playlistId:'playlist123456',submittedAt:'2026-10-14T12:00:00.000Z'}}};
 const session={providerOperations:[['op-5',{state:'done',mutation:true,response}]]};
 const request={id:'native-5',kind:'providerOperation',operationId:'op-5',provider:'spotify',operation:'addTracks',args:{}};
 const w=worker(session,{enabled:true,pageUrl:'https://open.spotify.com/',requests:[request]});await w.poll();
 const results=w.messages.filter(message=>message.type==='providerResult');assert.equal(results.length,1);assert.deepEqual(results[0],response);
});

test('interrupted provider reads are rerun after a worker restart',async()=>{
 const session={providerOperations:[['op-read',{state:'running',mutation:false}]]};
 const request={id:'native-read',kind:'providerOperation',operationId:'op-read',provider:'spotify',operation:'status',args:{}};
 const w=worker(session,{enabled:true,pageUrl:'https://open.spotify.com/',requests:[request]});await w.poll();
 const result=w.messages.find(message=>message.type==='providerResult');
 assert.equal(result?.ok,true);assert.equal(result?.result?.accountId,'browser-account');
});


test('provider request timeouts survive companion error normalization separately from rate limits',async()=>{
 for(const code of ['REQUEST_TIMEOUT','RATE_LIMITED']){
  const request={id:'native-read',kind:'providerOperation',operationId:'op-read',provider:'spotify',operation:'status',args:{}};
  const w=worker({}, {enabled:true,pageUrl:'https://open.spotify.com/',requests:[request],providerFailure:{code,message:code==='REQUEST_TIMEOUT'?'Provider request timed out.':'Provider rate limit.'}});
  await w.poll();const result=w.messages.find(message=>message.type==='providerResult');
  assert.equal(result?.ok,false);assert.equal(result?.error?.code,code);
 }
});

test('preview captures metadata without committing and apply forwards only its preview ID',async()=>{
 const w=worker({requestTabs:[[8,'pending-review']]});
 const result=await w.send({type:'previewCurrent'});
 assert.equal(result.ok,true);
 const request=w.messages.find(message=>message.type==='import');
 assert.equal(request.preview,true);
 assert.equal(request.requestId,undefined);
 assert.equal(w.session.requestTabs[0][1],'pending-review');
 await w.send({type:'applyImportPreview',previewId:'12345678-1234-4234-8234-123456789012'});
 const applied=w.messages.find(message=>message.type==='applyImportPreview');
 assert.equal(applied.previewId,'12345678-1234-4234-8234-123456789012');
 assert.equal(applied.snapshot,undefined);
});

test('Discover preview retains its collection request and explicit album URLs',async()=>{
 const w=worker({}, {pageUrl:'https://bandcamp.com/discover/alternative',albumCollection:true});
 const result=await w.send({type:'previewCurrent'});
 assert.equal(result.ok,true);
 const request=w.messages.find(message=>message.type==='importAlbumCollection');
 assert.equal(request.preview,true);
 assert.deepEqual(request.albumUrls,['https://artist.bandcamp.com/album/record']);
});
