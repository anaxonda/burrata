import {tick} from 'svelte';
export type QueueUiAction={action:'select';index:number;modifier:'none'|'ctrl'|'shift'}|{action:'arrow';index:number;direction:'up'|'down'}|{action:'drag';from:number;to:number};
export type QueueUiError='invalid_request'|'unavailable'|'stale'|'busy'|'unsupported_drag'|'cancelled'|'timeout'|'move_failed';
export type QueueUiMove={from:number;to:number};
export type QueueUiResult={protocolVersion:1;action:'select'|'arrow'|'drag';dispatched:boolean;syntheticEvents:true;trustedGesture:false;movePending:boolean;selectedIndices:number[];focusedIndex:number|null;rowCount:number;errorCode:QueueUiError|null;acknowledgedMoves:QueueUiMove[];mutationState:'unchanged'|'completed'|'partial_or_unknown'};
export function validQueueUiAction(value:unknown):value is QueueUiAction{
 if(!value||typeof value!=='object'||Array.isArray(value))return false;const v=value as Record<string,unknown>;
 const index=(n:unknown)=>Number.isInteger(n)&&Number(n)>=0&&Number(n)<=199;
 if(v.action==='select')return Object.keys(v).sort().join(',')==='action,index,modifier'&&index(v.index)&&typeof v.modifier==='string'&&['none','ctrl','shift'].includes(v.modifier);
 if(v.action==='arrow')return Object.keys(v).sort().join(',')==='action,direction,index'&&index(v.index)&&typeof v.direction==='string'&&['up','down'].includes(v.direction);
 return v.action==='drag'&&Object.keys(v).sort().join(',')==='action,from,to'&&index(v.from)&&index(v.to);
}
export async function dispatchQueueUi(root:Document,request:QueueUiAction,signal:AbortSignal,current:()=>boolean):Promise<QueueUiResult>{
 const result:QueueUiResult={protocolVersion:1,action:['select','arrow','drag'].includes(request?.action)?request.action:'select',dispatched:false,syntheticEvents:true,trustedGesture:false,movePending:false,selectedIndices:[],focusedIndex:null,rowCount:0,errorCode:null,acknowledgedMoves:[],mutationState:'unchanged'};
 const panel=root.querySelector<HTMLElement>('.playback-queue-panel');
 const rows=()=>[...(panel?.querySelectorAll<HTMLElement>('.queue-list > article')??[])];
 const grips=()=>rows().map(row=>row.querySelector<HTMLButtonElement>(':scope > .queue-drag-select'));
 const finish=(errorCode:QueueUiError|null)=>{const controls=grips();result.errorCode=errorCode;result.rowCount=controls.length;result.movePending=panel?.getAttribute('aria-busy')==='true';result.selectedIndices=controls.flatMap((g,i)=>g?.getAttribute('aria-pressed')==='true'?[i]:[]);const focused=controls.indexOf(root.activeElement as HTMLButtonElement);result.focusedIndex=focused<0?null:focused;return result;};
 if(!validQueueUiAction(request))return finish('invalid_request');
 if(signal.aborted)return finish('cancelled');if(!current())return finish('stale');
 if(!panel?.isConnected||panel.getBoundingClientRect().width<=0)return finish('unavailable');
 if(panel.getAttribute('aria-busy')==='true')return finish('busy');
 if(rows().length<1||rows().length>200)return finish('unavailable');
 const from=request.action==='drag'?request.from:request.index,grip=grips()[from];
 if(!grip||request.action==='drag'&&!rows()[request.to])return finish('unavailable');
 const previousFailure=panel.dataset.queueMoveFailures;
 let endDrag:(()=>void)|null=null;
 try{
  if(request.action==='select')grip.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true,ctrlKey:request.modifier==='ctrl',shiftKey:request.modifier==='shift'}));
  else if(request.action==='arrow'){grip.focus({preventScroll:true});grip.dispatchEvent(new KeyboardEvent('keydown',{key:request.direction==='up'?'ArrowUp':'ArrowDown',bubbles:true,cancelable:true}));}
  else{
   if(typeof DragEvent!=='function'||typeof DataTransfer!=='function')return finish('unsupported_drag');
   const dataTransfer=new DataTransfer(),target=rows()[request.to];
   endDrag=()=>grip.dispatchEvent(new DragEvent('dragend',{bubbles:true,dataTransfer}));
   grip.dispatchEvent(new DragEvent('dragstart',{bubbles:true,cancelable:true,dataTransfer}));
   target.dispatchEvent(new DragEvent('dragover',{bubbles:true,cancelable:true,dataTransfer}));
   target.dispatchEvent(new DragEvent('drop',{bubbles:true,cancelable:true,dataTransfer}));
  }
  result.dispatched=true;await tick();const deadline=performance.now()+4000;
  while(panel.getAttribute('aria-busy')==='true'){
   if(signal.aborted)return finish('cancelled');if(!current())return finish('stale');if(performance.now()>=deadline)return finish('timeout');
   await new Promise(resolve=>setTimeout(resolve,10));await tick();
  }
  if(signal.aborted)return finish('cancelled');if(!current())return finish('stale');
  return finish(panel.dataset.queueMoveFailures!==previousFailure?'move_failed':null);
 }catch{return finish('move_failed');}finally{endDrag?.();}
}

/** Only command acknowledgements advance this request's revision; never adopt a live external revision. */
export function queueUiMoves(initialRevision:number,send:(from:number,to:number,revision:number)=>Promise<{queueRevision?:number}>,signal:AbortSignal){
 let revision=initialRevision;const acknowledgedMoves:QueueUiMove[]=[];let pending=false,uncertain=false;
 return { get revision(){return revision;},get maximumObservedRevision(){return revision+(pending?1:0);}, acknowledgedMoves,
 get mutationState():'unchanged'|'completed'|'partial_or_unknown'{return pending||uncertain||(signal.aborted&&acknowledgedMoves.length>0)?'partial_or_unknown':acknowledgedMoves.length?'completed':'unchanged';},
 async move(from:number,to:number){if(signal.aborted)throw new Error('cancelled');pending=true;
 try{const result=await send(from,to,revision);if(result.queueRevision!==revision+1){uncertain=true;throw new Error('stale');}revision=result.queueRevision;acknowledgedMoves.push({from,to});}
 catch(error){uncertain=true;throw error;}finally{pending=false;}
 if(signal.aborted)throw new Error('cancelled');}
 };
}
