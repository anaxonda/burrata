// @vitest-environment jsdom
// @ts-expect-error Browser runtime for component tests.
import { mount, tick, unmount } from '../node_modules/svelte/src/index-client.js';
import { afterEach, expect, it, vi } from 'vitest';
// @ts-expect-error Browser runtime for component tests.
vi.mock('svelte', async () => await import('../node_modules/svelte/src/index-client.js'));
import LocalAlbumMatch from './LocalAlbumMatch.svelte';
let component: ReturnType<typeof mount>;
afterEach(async () => { if(component)await unmount(component); document.body.innerHTML=''; vi.useRealTimers(); });
const album={id:'album:ref:spotify:edition',kind:'album',name:'Album',artist:'Artist',provider:'spotify',providerId:'edition',explicitlySaved:true,trackCount:2,albumCount:0};
const candidate={album,matchedTracks:2,localTracks:2,catalogTracks:2,completeMatch:true,canLink:true,reason:'Every track matches.',token:'proof'};
async function settle(){for(let i=0;i<12;i++)await Promise.resolve();await tick();}
async function render(state='auto'){
 vi.useFakeTimers();const target=document.createElement('div');document.body.append(target);
 const call=vi.fn(async(command:string)=>command==='list_local_album_matches'?{localAlbum:{...album,name:'Local Album'},localTracks:2,state,folders:['/Music/Artist/Album'],candidates:[candidate]}:album);
 const onChanged=vi.fn(),onClose=vi.fn();component=mount(LocalAlbumMatch,{target,props:{localAlbumIds:['album:ref:local:hash'],call:call as never,onChanged,onClose}});
 await settle();await vi.advanceTimersByTimeAsync(160);await settle();return{target,call,onChanged,onClose};
}
it('explains automatic linking and sends the current proof when choosing an edition',async()=>{
 const {target,call,onChanged,onClose}=await render();
 expect(target.textContent).toContain('unique complete tracklist match');expect(target.textContent).toContain('/Music/Artist/Album');
 [...target.querySelectorAll('button')].find(b=>b.textContent==='Use this album')!.click();await settle();
 expect(call).toHaveBeenCalledWith('set_local_album_match',{localAlbumId:'album:ref:local:hash',targetAlbumId:album.id,decision:'confirm',token:'proof'});
 expect(onChanged).toHaveBeenCalledWith(album);expect(onClose).toHaveBeenCalled();
});
it('offers a reversible unlink action for automatic matches',async()=>{
 const {target,call}=await render();[...target.querySelectorAll('button')].find(b=>b.textContent==='Unlink and keep separate')!.click();await settle();
 expect(call).toHaveBeenCalledWith('set_local_album_match',{localAlbumId:'album:ref:local:hash',targetAlbumId:'',decision:'reject',token:null});
});
it('keeps the dialog open when the files change before confirmation',async()=>{
 const {target,call,onClose}=await render('unlinked');call.mockImplementation(async(command:string)=>{if(command==='set_local_album_match')throw new Error('The files changed. Refresh matches.');return{} as never;});
 [...target.querySelectorAll('button')].find(b=>b.textContent==='Use this album')!.click();await settle();
 expect(target.querySelector('[role="alert"]')?.textContent).toContain('files changed');expect(onClose).not.toHaveBeenCalled();
});
