// @vitest-environment jsdom
// @ts-expect-error Browser Svelte runtime.
import {mount,tick,unmount} from '../../node_modules/svelte/src/index-client.js';
import {afterEach,expect,it,vi} from 'vitest';
// @ts-expect-error Browser Svelte runtime.
vi.mock('svelte',async()=>await import('../../node_modules/svelte/src/index-client.js'));
import Panel from '../PlaybackQueuePanel.svelte';import {dispatchQueueUi,validQueueUiAction,queueUiMoves} from './queue-ui-automation';
const track={id:'same',provider:'library',providerId:'same',sourceUrl:'',title:'Private fixture title',artist:'Private artist',favorite:false,provenance:{}};
const originalShowModal=Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype,'showModal');
let cleanup=()=>{};afterEach(()=>{cleanup();document.body.innerHTML='';vi.unstubAllGlobals();if(originalShowModal)Object.defineProperty(HTMLDialogElement.prototype,'showModal',originalShowModal);else Reflect.deleteProperty(HTMLDialogElement.prototype,'showModal');});
async function render(onMove=vi.fn(async()=>{}),signal?:AbortSignal,onInteractionMove?: (from:number,to:number)=>Promise<void>){const target=document.createElement('div');document.body.append(target);const onPlay=vi.fn();const component=mount(Panel,{target,props:{tracks:[track,track,{...track,id:'third'},{...track,id:'fourth'}],currentIndex:1,paused:true,position:10,repeatMode:'off',shuffleEnabled:false,onClose:vi.fn(),onPlay,onMove,onRemove:vi.fn(),onSeek:vi.fn(),onToggleShuffle:vi.fn(),onCycleRepeat:vi.fn(),interactionSignal:signal,onInteractionMove} as never});cleanup=()=>{void unmount(component);};await tick();target.querySelector<HTMLElement>('.playback-queue-panel')!.getBoundingClientRect=()=>({width:340,height:400,left:0,top:0,right:340,bottom:400,x:0,y:0,toJSON(){return {};}});return {onPlay,onMove};}
const settle=async()=>{for(let i=0;i<12;i++)await Promise.resolve();await tick();};
it('strict finite input and actual modifier-selection handlers without playback',async()=>{
 for(const x of [{action:'select',index:-1,modifier:'ctrl'},{action:'select',index:200,modifier:'ctrl'},{action:'arrow',index:0,direction:'Enter'},{action:'drag',from:0,to:2,selector:'body'}])expect(validQueueUiAction(x)).toBe(false);
 const {onPlay}=await render(),signal=new AbortController().signal;
 expect((await dispatchQueueUi(document,{action:'select',index:1,modifier:'none'},signal,()=>true)).selectedIndices).toEqual([1]);
 expect((await dispatchQueueUi(document,{action:'select',index:3,modifier:'ctrl'},signal,()=>true)).selectedIndices).toEqual([1,3]);
 const result=await dispatchQueueUi(document,{action:'select',index:2,modifier:'shift'},signal,()=>true);expect(result.selectedIndices).toEqual([2,3]);expect(onPlay).not.toHaveBeenCalled();expect(JSON.stringify(result)).not.toContain(track.title);
});
it('waits for async movement, rejects busy dispatch and follows moved focus',async()=>{
 let finish!:()=>void;const onMove=vi.fn().mockImplementationOnce(()=>new Promise<void>(resolve=>finish=resolve)).mockResolvedValue(undefined);await render(onMove);const signal=new AbortController().signal;
 await dispatchQueueUi(document,{action:'select',index:1,modifier:'none'},signal,()=>true);
 let completed=false;const pending=dispatchQueueUi(document,{action:'arrow',index:1,direction:'up'},signal,()=>true).then(x=>{completed=true;return x;});await settle();expect(completed).toBe(false);
 expect((await dispatchQueueUi(document,{action:'select',index:2,modifier:'ctrl'},signal,()=>true)).errorCode).toBe('busy');finish();const result=await pending;expect(result.focusedIndex).toBe(0);expect(result.movePending).toBe(false);
 expect((await dispatchQueueUi(document,{action:'arrow',index:0,direction:'down'},signal,()=>true)).focusedIndex).toBe(1);expect(onMove.mock.calls).toEqual([[1,0],[1,0]]);
});
it('actual drag/drop handler moves only the duplicate occurrence',async()=>{
 class Transfer{effectAllowed='';setData(){}}class Drag extends Event{dataTransfer:unknown;constructor(type:string,init:EventInit&{dataTransfer?:unknown}){super(type,init);this.dataTransfer=init.dataTransfer;}}
 vi.stubGlobal('DataTransfer',Transfer);vi.stubGlobal('DragEvent',Drag);const {onMove,onPlay}=await render();const result=await dispatchQueueUi(document,{action:'drag',from:1,to:0},new AbortController().signal,()=>true);expect(result.errorCode).toBeNull();expect(result.syntheticEvents).toBe(true);expect(result.trustedGesture).toBe(false);expect(onMove).toHaveBeenCalledExactlyOnceWith(1,0);expect(onPlay).not.toHaveBeenCalled();
});
it('cancel stops a selected block after its in-flight acknowledgement',async()=>{
 const c=new AbortController();let finish!:()=>void;const onMove=vi.fn().mockImplementationOnce(()=>new Promise<void>(r=>finish=r)).mockResolvedValue(undefined);await render(onMove,c.signal);
 await dispatchQueueUi(document,{action:'select',index:1,modifier:'none'},c.signal,()=>true);await dispatchQueueUi(document,{action:'select',index:2,modifier:'shift'},c.signal,()=>true);
 const pending=dispatchQueueUi(document,{action:'arrow',index:1,direction:'up'},c.signal,()=>true);await settle();c.abort();finish();expect((await pending).errorCode).toBe('cancelled');await settle();expect(onMove).toHaveBeenCalledTimes(1);
});
it('stale or cancelled requests invoke no movement or playback',async()=>{const {onMove,onPlay}=await render();const c=new AbortController();expect((await dispatchQueueUi(document,{action:'arrow',index:1,direction:'up'},c.signal,()=>false)).errorCode).toBe('stale');c.abort();expect((await dispatchQueueUi(document,{action:'arrow',index:1,direction:'up'},c.signal,()=>true)).errorCode).toBe('cancelled');expect(onMove).not.toHaveBeenCalled();expect(onPlay).not.toHaveBeenCalled();});

it('reports a failed move without poisoning later selection or successful retry',async()=>{const move=vi.fn().mockRejectedValueOnce(new Error('fixture')).mockResolvedValue(undefined);await render(move);const signal=new AbortController().signal;expect((await dispatchQueueUi(document,{action:'arrow',index:1,direction:'up'},signal,()=>true)).errorCode).toBe('move_failed');expect((await dispatchQueueUi(document,{action:'select',index:1,modifier:'none'},signal,()=>true)).errorCode).toBeNull();expect((await dispatchQueueUi(document,{action:'arrow',index:1,direction:'up'},signal,()=>true)).errorCode).toBeNull();});

it('advances only own acknowledged revisions and records cancellation partial state',async()=>{
 const controller=new AbortController();let finish!:(value:{queueRevision:number})=>void;
 const send=vi.fn().mockResolvedValueOnce({queueRevision:11}).mockImplementationOnce(()=>new Promise(r=>finish=r));
 const moves=queueUiMoves(10,send,controller.signal);await moves.move(2,0);expect(moves.revision).toBe(11);
 const pending=moves.move(3,1);controller.abort();finish({queueRevision:12});await expect(pending).rejects.toThrow('cancelled');
 expect(send.mock.calls).toEqual([[2,0,10],[3,1,11]]);expect(moves.acknowledgedMoves).toEqual([{from:2,to:0},{from:3,to:1}]);
 await expect(moves.move(0,1)).rejects.toThrow('cancelled');expect(send).toHaveBeenCalledTimes(2);expect(moves.mutationState).toBe('partial_or_unknown');
 const foreign=queueUiMoves(10,async()=>({queueRevision:12}),new AbortController().signal);await expect(foreign.move(1,0)).rejects.toThrow('stale');expect(foreign.mutationState).toBe('partial_or_unknown');expect(foreign.revision).toBe(10);
});

it('attributes synthetic movement separately from ordinary overflow movement',async()=>{
 vi.stubGlobal('ResizeObserver',class {observe(){}unobserve(){}disconnect(){}});
 Object.defineProperty(HTMLDialogElement.prototype,'showModal',{configurable:true,value:function(this:HTMLDialogElement){this.setAttribute('open','');}});
 const ordinary=vi.fn(async()=>{}),owned=vi.fn(async()=>{});await render(ordinary,new AbortController().signal,owned);
 await dispatchQueueUi(document,{action:'arrow',index:1,direction:'up'},new AbortController().signal,()=>true);
 expect(owned).toHaveBeenCalledExactlyOnceWith(1,0);expect(ordinary).not.toHaveBeenCalled();
 document.querySelector<HTMLElement>('.queue-list > article:nth-child(2) summary')!.click();await settle();
 const button=[...document.querySelectorAll<HTMLButtonElement>('.queue-list > article:nth-child(2) button')].find(b=>b.textContent?.trim()==='Move up');
 expect(button).toBeTruthy();button!.click();await settle();expect(ordinary).toHaveBeenCalledExactlyOnceWith(1,0);expect(owned).toHaveBeenCalledTimes(1);
});
