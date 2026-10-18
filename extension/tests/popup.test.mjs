import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

const source=readFileSync(new URL('../popup.js',import.meta.url),'utf8');

function popup(){
  const ids=['status','enable','import','youtube-pairing-code','youtube-consent','youtube-check-account','youtube-account','youtube-account-consent-label','youtube-account-consent','youtube-handoff','diagnostic'];
  const elements=Object.fromEntries(ids.map(id=>[id,{id,disabled:false,checked:false,hidden:false,value:'',textContent:'',onclick:null,oninput:null,onchange:null}]));
  const messages=[];
  const api={runtime:{sendMessage:async message=>{messages.push(message);if(message.type==='youtubeCheckHandoffAccount')return {ok:true,account:{accountId:'UCaaaaaaaaaaaaaaaaaaaaaa',displayName:'Fixture Channel'}};if(message.type==='youtubeSessionHandoff')return {ok:true,account:{displayName:'Fixture Channel'}};return {ok:true,enabled:false,connected:true};}},tabs:{query:async()=>[{id:1,url:'https://www.youtube.com/feed/playlists'}]},permissions:{contains:async()=>true,request:async()=>true}};
  const context=vm.createContext({browser:api,URL,console,Blob,setTimeout,document:{querySelector:selector=>elements[selector.slice(1)]}});
  vm.runInContext(source,context);
  return {elements,messages};
}

test('YouTube popup requires code, consent, account review, and a second explicit confirmation',async()=>{
  const {elements:e,messages}=popup();
  const code='43123-0123456789abcdef0123456789abcdef';
  e['youtube-pairing-code'].value=code;e['youtube-pairing-code'].oninput();
  assert.equal(e['youtube-check-account'].disabled,true);
  e['youtube-consent'].checked=true;e['youtube-consent'].onchange();
  assert.equal(e['youtube-check-account'].disabled,false);
  await e['youtube-check-account'].onclick();
  assert.deepEqual(messages.map(message=>message.type),['status','youtubeCheckHandoffAccount']);
  assert.equal(e['youtube-account'].textContent,'Selected YouTube account: Fixture Channel');
  assert.equal(e['youtube-handoff'].disabled,true);
  e['youtube-account-consent'].checked=true;e['youtube-account-consent'].onchange();
  assert.equal(e['youtube-handoff'].disabled,false);
  await e['youtube-handoff'].onclick();
  const handoff=messages.find(message=>message.type==='youtubeSessionHandoff');
  assert.equal(handoff.pairingCode,code);
  assert.equal(handoff.confirmedAccountId,'UCaaaaaaaaaaaaaaaaaaaaaa');
  assert.equal(e['youtube-pairing-code'].value,'');
});

test('YouTube popup rejects malformed pairing codes before account lookup',async()=>{
  const {elements:e,messages}=popup();
  e['youtube-pairing-code'].value='43123-0123456789abcdef0123456789abcg';e['youtube-pairing-code'].oninput();
  e['youtube-consent'].checked=true;e['youtube-consent'].onchange();
  assert.equal(e['youtube-check-account'].disabled,true);
  assert.equal(messages.filter(message=>message.type==='youtubeCheckHandoffAccount').length,0);
});
