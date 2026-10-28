import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

const source=readFileSync(new URL('../popup.js',import.meta.url),'utf8');
const markup=readFileSync(new URL('../popup.html',import.meta.url),'utf8');

function popup(){
  const ids=['preview-import','apply-import','status','enable','import','show-settings','show-import','import-view','settings-view','import-heading','settings-heading','youtube-pairing-code','youtube-consent','youtube-check-account','youtube-account','youtube-account-consent-label','youtube-account-consent','youtube-handoff','diagnostic'];
  const elements=Object.fromEntries(ids.map(id=>[id,{id,disabled:false,checked:false,hidden:false,value:'',textContent:'',onclick:null,oninput:null,onchange:null,focus(){this.focused=true;}}]));
  elements['settings-view'].hidden=true;
  const messages=[],permissionRequests=[];
  const api={runtime:{sendMessage:async message=>{messages.push(message);if(message.type==='youtubeCheckHandoffAccount')return {ok:true,account:{accountId:'UCaaaaaaaaaaaaaaaaaaaaaa',displayName:'Fixture Channel'}};if(message.type==='youtubeSessionHandoff')return {ok:true,account:{displayName:'Fixture Channel'}};return {ok:true,enabled:message.type==='enable'?message.enabled:false,connected:true};}},tabs:{query:async()=>[{id:1,url:'https://www.youtube.com/feed/playlists'}]},permissions:{contains:async()=>true,request:async request=>{permissionRequests.push(request);return true;}}};
  const context=vm.createContext({browser:api,URL,console,Blob,setTimeout,document:{querySelector:selector=>elements[selector.slice(1)]}});
  vm.runInContext(source,context);
  return {elements,messages,permissionRequests};
}

test('popup opens on page import and keeps connection controls in a separate settings view',()=>{
  const {elements:e}=popup();
  assert.equal(e['import-view'].hidden,false);
  assert.equal(e['settings-view'].hidden,true);
  const detailsStart=markup.indexOf('<summary>More options and supported pages</summary>');
  const detailsEnd=markup.indexOf('</details>',detailsStart);
  const diagnostic=markup.indexOf('id="diagnostic"');
  assert.ok(diagnostic>detailsStart&&diagnostic<detailsEnd,'diagnostic stays under the collapsed more-options disclosure');
  assert.ok(markup.indexOf('id="enable"')>markup.indexOf('id="settings-view"'),'browser-assisted controls stay out of the import view');
  e['show-settings'].onclick();
  assert.equal(e['import-view'].hidden,true);
  assert.equal(e['settings-view'].hidden,false);
  assert.equal(e['settings-heading'].focused,true);
  e['show-import'].onclick();
  assert.equal(e['settings-view'].hidden,true);
  assert.equal(e['import-view'].hidden,false);
  assert.equal(e['import-heading'].focused,true);
});

test('browser-assisted access asks for provider permissions only when enabled',async()=>{
  const {elements:e,messages,permissionRequests}=popup();
  e['show-settings'].onclick();
  await new Promise(resolve=>setTimeout(resolve,0));
  assert.equal(e.enable.textContent,'Enable browser-assisted access');
  await e.enable.onclick();
  assert.equal(e.enable.textContent,'Disable browser-assisted access');
  assert.match(e.status.textContent,/Browser-assisted access enabled/);
  assert.equal(messages.at(-1).type,'enable');
  assert.equal(messages.at(-1).enabled,true);
  assert.ok(permissionRequests[0].origins.includes('https://open.spotify.com/*'));
  assert.ok(permissionRequests[0].origins.includes('https://www.youtube.com/*'));
});

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
