import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

const source=readFileSync(new URL('../background.js',import.meta.url),'utf8');
function worker(session={}, {enabled=false, verification=false, requests=[],pageUrl='https://www.beatport.com/genre/afro-house/89/top-100',albumCollection=false,handoffSession=null,youtubePage=null,accountIdentity=null,providerFailure=null,providerResponse={}}={}) {
  let listener, removed, reply;
  const messages=[], created=[], updated=[], scripts=[];
  const host=new URL(pageUrl).hostname;
  const snapshot={provider:host.includes('shazam')?'shazam':host.includes('last.fm')?'lastfm':host.includes('bandcamp')?'bandcamp':host.includes('youtube')?'youtube':'beatport',kind:host.includes('youtube')?'playlist':'chart',sourceUrl:pageUrl,entries:[]};
  const api={
    action:{setBadgeText(){},setBadgeBackgroundColor(){}},
    storage:{local:{get:async()=>({enabled}),set:async()=>{}},session:{get:async()=>session,set:async value=>Object.assign(session,value)}},
    runtime:{getURL:path=>`chrome-extension://test/${path}`,onMessage:{addListener(fn){listener=fn;}},connectNative(){return {
      onMessage:{addListener(fn){reply=fn;}},onDisconnect:{addListener(){}},disconnect(){},
      postMessage(message){messages.push(message);queueMicrotask(()=>reply({ok:true,requests:message.type==='poll'?requests:[],result:{id:'saved'}}));},
    };}},
    scripting:{executeScript:async details=>{scripts.push(details);return details.world==='MAIN'?[{result:details.func?.name==='captureYouTubeAccountIdentity'?{account:accountIdentity||{accountId:handoffSession?.accountId||'UCaaaaaaaaaaaaaaaaaaaaaa',displayName:handoffSession?.displayName||'Fixture Channel'}}:details.func?.name==='captureYouTubeSessionHandoff'?(handoffSession?{session:handoffSession}:details.func()):providerFailure?{__burrataProviderError:providerFailure}:{accountId:'browser-account',displayName:'Test'}}]:[];}},
    tabs:{get:async id=>({id,status:'complete',url:pageUrl,windowId:1}),
      query:async()=>[{id:8,url:pageUrl}],create:async value=>{created.push(value);return {id:9};},
      update:async(id,value)=>{updated.push({id,...value});return {windowId:1};},
      sendMessage:async()=>verification?{verification:true}:albumCollection?{albumCollection:{sourceUrl:'https://bandcamp.com/discover/hip-hop-rap',title:'Hip-hop',albumUrls:['https://artist.bandcamp.com/album/record']}}:{snapshot},onRemoved:{addListener(fn){removed=fn;}}},
    windows:{update:async()=>{}},permissions:{contains:async()=>true},
    alarms:{create(){},onAlarm:{addListener(){}}},
  };
  const context=vm.createContext({chrome:api,URL,TextEncoder,Map,Promise,AbortController,crypto:{randomUUID:()=> 'test-id'},BurrataSpotifyAuth:{loadRegistry:async()=>({secret:'fixture',version:'fixture'}),totp:async()=> '123456'},location:youtubePage?.location||{origin:new URL(pageUrl).origin,hostname:new URL(pageUrl).hostname,pathname:new URL(pageUrl).pathname},document:youtubePage?.document||{cookie:'',querySelector:()=>null},fetch:async()=>({ok:true,status:200,text:async()=>JSON.stringify(providerResponse)}),setInterval(){},setTimeout(fn,ms){if(ms<30000)queueMicrotask(fn);return 1;},clearTimeout(){}});
  vm.runInContext(source,context);
  return {messages,created,updated,scripts,session,removed,
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
  for(const pageUrl of ['https://www.shazam.com/charts/top-200/world','https://www.last.fm/charts','https://label.bandcamp.com/album/example']){
    const w=worker({}, {pageUrl});
    const result=await w.send({type:'importCurrent'});
    assert.equal(result.ok,true,pageUrl);
    assert.equal(w.messages[0].snapshot.provider,new URL(pageUrl).hostname.includes('shazam')?'shazam':new URL(pageUrl).hostname.includes('last.fm')?'lastfm':'bandcamp');
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
