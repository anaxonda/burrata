import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

const source=readFileSync(new URL('../background.js',import.meta.url),'utf8');
function worker(session={}, {enabled=false, verification=false, requests=[],pageUrl='https://www.beatport.com/genre/afro-house/89/top-100',albumCollection=false}={}) {
  let listener, removed, reply;
  const messages=[], created=[], updated=[];
  const snapshot={provider:new URL(pageUrl).hostname.includes('shazam')?'shazam':new URL(pageUrl).hostname.includes('last.fm')?'lastfm':new URL(pageUrl).hostname.includes('bandcamp')?'bandcamp':'beatport',entries:[]};
  const api={
    action:{setBadgeText(){},setBadgeBackgroundColor(){}},
    storage:{local:{get:async()=>({enabled}),set:async()=>{}},session:{get:async()=>session,set:async value=>Object.assign(session,value)}},
    runtime:{onMessage:{addListener(fn){listener=fn;}},connectNative(){return {
      onMessage:{addListener(fn){reply=fn;}},onDisconnect:{addListener(){}},disconnect(){},
      postMessage(message){messages.push(message);queueMicrotask(()=>reply({ok:true,requests:message.type==='poll'?requests:[],result:{id:'saved'}}));},
    };}},
    scripting:{executeScript:async()=>{}},
    tabs:{get:async id=>({id,status:'complete',url:pageUrl,windowId:1}),
      query:async()=>[{id:8,url:pageUrl}],create:async value=>{created.push(value);return {id:9};},
      update:async(id,value)=>{updated.push({id,...value});return {windowId:1};},
      sendMessage:async()=>verification?{verification:true}:albumCollection?{albumCollection:{sourceUrl:'https://bandcamp.com/discover/hip-hop-rap',title:'Hip-hop',albumUrls:['https://artist.bandcamp.com/album/record']}}:{snapshot},onRemoved:{addListener(fn){removed=fn;}}},
    windows:{update:async()=>{}},permissions:{contains:async()=>true},
    alarms:{create(){},onAlarm:{addListener(){}}},
  };
  const context=vm.createContext({chrome:api,URL,TextEncoder,Map,Promise,setInterval(){},setTimeout(fn,ms){if(ms<30000)queueMicrotask(fn);return 1;},clearTimeout(){}});
  vm.runInContext(source,context);
  return {messages,created,updated,session,removed,
    send:message=>new Promise(resolve=>listener(message,{},resolve)),
    poll:()=>vm.runInContext('poll()',context),
  };
}
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
 assert.ok(w.messages.some(message=>message.type==='heartbeat'));
});
