import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import {JSDOM} from 'jsdom';
import * as parsers from '../parsers.js';

const source=readFileSync(new URL('../content.js',import.meta.url),'utf8').replace(
  /const \{extract,mergeSnapshots,isVerification,diagnosticSample\}=await import\([^;]+\);/,
  'const {extract,mergeSnapshots,isVerification,diagnosticSample}=globalThis.parserFunctions;',
);

function capturePage(){
  const dom=new JSDOM('<!doctype html><html><body><aside><div id="sidebar-scroll" data-overflow-y="auto"><div role="row" aria-rowindex="1"><a href="/track/SIDEBAR">Sidebar item</a></div></div></aside><main><h1>Large playlist</h1><p>96 songs</p><div data-overlayscrollbars-viewport data-overflow-y="hidden"><div id="rows"></div></div></main></body></html>',{url:'https://open.spotify.com/playlist/PL'});
  const {document}=dom.window, viewport=document.querySelector('[data-overlayscrollbars-viewport]'), sidebar=document.querySelector('#sidebar-scroll'), rows=document.querySelector('#rows');
  let top=400;const history=[];
  Object.defineProperties(viewport,{scrollHeight:{get:()=>5000},clientHeight:{get:()=>500},scrollTop:{get:()=>top,set:value=>{top=Math.max(0,Math.min(4500,Number(value)||0));history.push(top);render();}}});
  let sidebarTop=0;
  Object.defineProperties(sidebar,{scrollHeight:{get:()=>2000},clientHeight:{get:()=>300},scrollTop:{get:()=>sidebarTop,set:value=>{sidebarTop=Math.max(0,Math.min(1700,Number(value)||0));}}});
  function render(){
    rows.replaceChildren();
    const start=Math.min(Math.floor(top/50)+1,87);
    for(let position=start;position<Math.min(97,start+10);position++){
      const row=document.createElement('div');row.dataset.testid='tracklist-row';row.setAttribute('aria-rowindex',String(position));
      const cell=document.createElement('div');cell.setAttribute('role','gridcell');cell.textContent=String(position);row.append(cell);
      const track=document.createElement('a');track.href=`/track/ID${position}`;track.textContent=`Song ${position}`;row.append(track);
      const artist=document.createElement('a');artist.href='/artist/ARTIST';artist.textContent='Artist';row.append(artist);
      rows.append(row);
    }
  }
  render();
  let listener;
  const api={runtime:{getURL:()=>'',onMessage:{addListener:fn=>listener=fn,removeListener(){}}}};
  const context=vm.createContext({browser:api,document,location:dom.window.location,chrome:api,parserFunctions:parsers,
    getComputedStyle:node=>({overflowY:node.getAttribute?.('data-overflow-y')||'visible',overflow:node.getAttribute?.('data-overflow-y')||'visible'}),
    setTimeout:fn=>{setImmediate(fn);return 1;},clearTimeout(){}});
  vm.runInContext(source,context);
  return {document,dom,viewport,sidebar,history,get listener(){return listener;}};
}

test('Spotify capture finds hidden OverlayScrollbars viewport, captures virtualized rows, and restores scroll',async()=>{
  const page=capturePage();
  await new Promise(resolve=>setImmediate(resolve));
  const diagnostic=await new Promise(resolve=>page.listener({type:'diagnostic'},{},resolve));
  const overlay=diagnostic.diagnostic.scrollAncestors.find(node=>node.overlayViewport);
  assert.equal(overlay.overflowY,'hidden');assert.ok(overlay.scrollHeight>overlay.clientHeight);
  const result=await new Promise(resolve=>page.listener({type:'capture'},{},resolve));
  assert.equal(result.error,undefined);
  assert.equal(result.snapshot.expectedTotal,96);
  assert.equal(result.snapshot.entries.length,96);
  assert.equal(result.snapshot.complete,true);
  assert.equal(page.viewport.scrollTop,400);
  assert.equal(page.sidebar.scrollTop,0);
  page.dom.window.close();
});

test('YouTube public playlist capture scrolls bounded rendered rows, keeps duplicate positions, and restores scroll',async()=>{
  const dom=new JSDOM('<!doctype html><html><body><main><ytd-playlist-header-renderer><h1>Public playlist</h1><span>6 videos</span></ytd-playlist-header-renderer><div id="viewport" data-overlayscrollbars-viewport><ytd-playlist-video-list-renderer><div id="rows"></div></ytd-playlist-video-list-renderer></div></main></body></html>',{url:'https://www.youtube.com/playlist?list=PLabcdefghijk'});
  const {document}=dom.window,viewport=document.querySelector('#viewport'),rows=document.querySelector('#rows');
  let top=40;const history=[];
  Object.defineProperties(viewport,{scrollHeight:{get:()=>600},clientHeight:{get:()=>200},scrollTop:{get:()=>top,set:value=>{top=Math.max(0,Math.min(400,Number(value)||0));history.push(top);render();}}});
  function render(){
    rows.replaceChildren();
    const start=Math.floor(top/100)+1;
    for(let position=start;position<=Math.min(6,start+2);position++){
      const row=document.createElement('ytd-playlist-video-renderer');
      const index=document.createElement('span');index.id='index';index.textContent=String(position);
      const video=document.createElement('a');video.id='video-title';video.href=`/watch?v=${position===1||position===3?'abcdefghijk':`VIDEOID${String(position).padStart(4,'0')}`}`;video.textContent=`Song ${position}`;
      const byline=document.createElement('div');byline.id='byline-container';byline.textContent='Public Artist';
      row.append(index,video,byline);rows.append(row);
    }
  }
  render();let listener;
  const api={runtime:{getURL:()=>'',onMessage:{addListener:fn=>listener=fn,removeListener(){}}}};
  const context=vm.createContext({browser:api,document,location:dom.window.location,chrome:api,parserFunctions:parsers,
    getComputedStyle:node=>({overflowY:node.hasAttribute?.('data-overlayscrollbars-viewport')?'hidden':'visible',overflow:'visible'}),
    setTimeout:fn=>{setImmediate(fn);return 1;},clearTimeout(){}});
  vm.runInContext(source,context);await new Promise(resolve=>setImmediate(resolve));
  const result=await new Promise(resolve=>listener({type:'capture'},{},resolve));
  assert.equal(result.error,undefined);assert.equal(result.snapshot.provider,'youtube');
  assert.equal(result.snapshot.entries.length,6);assert.deepEqual(result.snapshot.entries.map(e=>e.position),[1,2,3,4,5,6]);
  assert.equal(result.snapshot.entries[0].providerId,result.snapshot.entries[2].providerId);
  assert.equal(result.snapshot.complete,true);assert.equal(top,40);assert.ok(history.length<80);
  dom.window.close();
});
