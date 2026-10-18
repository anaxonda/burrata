// @vitest-environment jsdom
// @ts-expect-error Exercise Svelte's browser runtime in jsdom.
import {mount,tick,unmount} from '../node_modules/svelte/src/index-client.js';
import {afterEach,describe,expect,it,vi} from 'vitest';
// @ts-expect-error Match Svelte browser runtime used by the component.
vi.mock('svelte',async()=>await import('../node_modules/svelte/src/index-client.js'));
import PlaylistWritebackMatch from './PlaylistWritebackMatch.svelte';
import {SOURCE_OPENER} from './lib/source-opening';

let dispose: (()=>void)|undefined;
afterEach(()=>{dispose?.();dispose=undefined;document.body.innerHTML='';});
async function settle(){for(let i=0;i<8;i++)await Promise.resolve();await tick();}
function render({provider='youtube',result,context=new Map(),disabled=false}:{provider?:string;result?:unknown;context?:Map<unknown,unknown>;disabled?:boolean}={}){
 const call=vi.fn(async(command:string)=>command==='search_remote_playlist_writeback_candidates'?result:null),onSelect=vi.fn();
 const target=document.createElement('div');document.body.append(target);
 const instance=mount(PlaylistWritebackMatch,{target,context,props:{bindingId:'binding-1',localTrackId:'local-1',provider,disabled,call,onSelect}});
 dispose=()=>unmount(instance);return {target,call,onSelect};
}
const youtubeCandidate={candidateToken:'token-1',title:'Magalenha Extended Mix',artist:'Simon Fava • Sergio Mendes',version:'Extended Mix',durationSeconds:268,url:'https://www.youtube.com/watch?v=abcdefghijk'};
const result=(candidate: Record<string, unknown>=youtubeCandidate)=>({bindingId:'binding-1',localTrackId:'local-1',provider:'youtube',candidates:[candidate]});

describe('PlaylistWritebackMatch candidate evidence',()=>{
 it('opens a Spotify recording without treating its artist as a YouTube byline',async()=>{
  const opener=vi.fn(async()=>{});
  const candidate={...youtubeCandidate,url:'https://open.spotify.com/track/AAAAAAAAAAAAAAAAAAAAAA',artist:'Recording Artist'};
  const {target,onSelect}=render({provider:'spotify',result:{...result(candidate),provider:'spotify'},context:new Map([[SOURCE_OPENER,opener]])});
  await settle();target.querySelector('button')!.click();await settle();
  expect(target.textContent).not.toContain('YouTube byline');
  Array.from(target.querySelectorAll('button')).find(button=>button.textContent?.includes('Open on Spotify'))!.click();await settle();
  expect(opener).toHaveBeenCalledWith(expect.objectContaining({provider:'spotify',providerId:'AAAAAAAAAAAAAAAAAAAAAA'}),'https://open.spotify.com/track/AAAAAAAAAAAAAAAAAAAAAA');
  expect(onSelect).not.toHaveBeenCalled();
 });
 it('labels YouTube byline uncertainty and never selects a search result automatically',async()=>{
  const {target,call,onSelect}=render({result:result()});
  await settle();target.querySelector('button')!.click();await settle();
  expect(call).toHaveBeenCalledTimes(1);expect(onSelect).not.toHaveBeenCalled();
  expect(target.textContent).toContain('not verified matches');
  expect(target.textContent).toContain('uploader or channel rather than the recording artist');
  expect(target.textContent).toContain('YouTube byline: Simon Fava • Sergio Mendes (artist not verified)');
  expect(target.textContent).toContain('4:28');
 });
 it('opens only a canonical source through the injected application source opener',async()=>{
  const opener=vi.fn(async()=>{});
  const {target,call,onSelect}=render({result:result(),context:new Map([[SOURCE_OPENER,opener]])});
  await settle();target.querySelector('button')!.click();await settle();
  const open=Array.from(target.querySelectorAll('button')).find(button=>button.textContent?.includes('Open on YouTube'))!;
  open.click();await settle();
  expect(opener).toHaveBeenCalledWith(expect.objectContaining({id:'candidate:youtube:abcdefghijk',provider:'youtube',providerId:'abcdefghijk'}),'https://www.youtube.com/watch?v=abcdefghijk');
  expect(onSelect).not.toHaveBeenCalled();
  expect(call).not.toHaveBeenCalledWith('open_external_url',expect.anything());
 });
 it('does not expose an external link for a mismatched or unsafe candidate URL',async()=>{
  const candidate={...youtubeCandidate,url:'https://evil.example/watch?v=abcdefghijk'};
  const {target,onSelect}=render({result:result(candidate)});await settle();target.querySelector('button')!.click();await settle();
  expect(target.textContent).toContain(candidate.title);
  expect(Array.from(target.querySelectorAll('button')).some(button=>button.textContent?.includes('Open on'))).toBe(false);
  expect(onSelect).not.toHaveBeenCalled();
 });
 it('labels missing artist and duration for review without dropping the candidate',async()=>{
  const candidate={...youtubeCandidate,artist:'',version:null,durationSeconds:null};
  const {target,onSelect}=render({result:result(candidate)});await settle();target.querySelector('button')!.click();await settle();
  expect(target.textContent).toContain('Artist credit missing or weak; verify manually.');
  expect(target.textContent).toContain('Duration is missing; verify manually.');
  expect(target.textContent).not.toContain('Version unknown');
  expect(onSelect).not.toHaveBeenCalled();
  target.querySelector<HTMLButtonElement>('button[aria-label^="Use "]')?.click();await settle();
  expect(onSelect).toHaveBeenCalledWith('local-1','token-1',expect.stringContaining('YouTube byline: unknown (artist not verified)'));
 });

 it('rejects noncanonical source query parameters and ports',async()=>{
  for(const url of ['https://www.youtube.com:444/watch?v=abcdefghijk','https://www.youtube.com/watch?v=abcdefghijk&feature=share']){
   const {target}=render({result:result({...youtubeCandidate,url}),context:new Map([[SOURCE_OPENER,vi.fn()]])});
   await settle();target.querySelector('button')!.click();await settle();
   expect(Array.from(target.querySelectorAll('button')).some(button=>button.textContent?.includes('Open on'))).toBe(false);
   dispose?.();dispose=undefined;document.body.innerHTML='';
  }
 });
 it('keeps current identity checks and rejects results for another binding',async()=>{
  const {target,call,onSelect}=render({result:{...result(),bindingId:'other-binding'}});await settle();target.querySelector('button')!.click();await settle();
  expect(target.textContent).toContain('returned matches belong to a different recording');
  expect(target.textContent).not.toContain(youtubeCandidate.title);
  expect(onSelect).not.toHaveBeenCalled();
 });
});
