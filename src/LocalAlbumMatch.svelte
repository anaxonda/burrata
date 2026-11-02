<script lang="ts">
  import { untrack } from 'svelte';
  import type { LibraryEntity } from './lib/library';
  type Candidate={album:LibraryEntity;matchedTracks:number;localTracks:number;catalogTracks:number;completeMatch:boolean;canLink:boolean;reason:string;token:string};
  type Page={localAlbum:LibraryEntity;localTracks:number;state:string;folders?:string[];candidates:Candidate[]};
  let { localAlbumIds,call,onChanged,onClose }:{localAlbumIds:string[];call:<T=unknown>(command:string,args?:Record<string,unknown>)=>Promise<T>;onChanged:(entry:LibraryEntity)=>void;onClose:()=>void}=$props();
  let selected=$state(''),query=$state(''),page=$state<Page|null>(null),loading=$state(false),applying=$state(false),error=$state('');
  let generation=0;
  $effect(()=>{if(!localAlbumIds.includes(selected))selected=localAlbumIds[0]??'';});
  $effect(()=>{
    const local=selected,search=query,request=++generation;
    loading=Boolean(local);error='';
    const timer=setTimeout(()=>{if(local)void load(local,search,request);},150);
    return()=>{clearTimeout(timer);generation++;};
  });
  async function load(local:string,search:string,request:number){
    try{const result=await call<Page>('list_local_album_matches',{localAlbumId:local,query:search});if(request===generation)page=result;}
    catch(reason){if(request===generation)error=reason instanceof Error?reason.message:String(reason);}
    finally{if(request===generation)loading=false;}
  }
  async function choose(candidate:Candidate|null){
    const request=generation;applying=true;error='';
    try{
      const entry=await call<LibraryEntity|null>('set_local_album_match',{localAlbumId:selected,targetAlbumId:candidate?.album.id??'',decision:candidate?'confirm':'reject',token:candidate?.token??null});
      window.dispatchEvent(new Event('library-changed'));
      if(request===generation){if(entry)onChanged(entry);onClose();}
    }catch(reason){if(request===generation)error=reason instanceof Error?reason.message:String(reason);}
    finally{if(request===generation)applying=false;}
  }
  function activate(node:HTMLDialogElement){untrack(()=>{if(typeof node.showModal==='function')node.showModal();else node.setAttribute('open','');});return{destroy(){node.removeAttribute('open');}};}
</script>
<dialog use:activate class="local-album-match" aria-labelledby="local-album-match-title" oncancel={event=>{event.preventDefault();if(!applying)onClose();}}>
  <header><div><h2 id="local-album-match-title">Match local files to an album</h2><p>Links combine your Library views. Your files and tags stay in place.</p></div><button aria-label="Close local album matching" disabled={applying} onclick={onClose}>×</button></header>
  {#if localAlbumIds.length>1}<label>Local copy<select bind:value={selected} disabled={applying}>{#each localAlbumIds as id,index}<option value={id}>Local copy {index+1}</option>{/each}</select></label>{/if}
  {#if page}<h3>{page.localAlbum.name}</h3><p>{page.localAlbum.artist} · {page.localTracks} indexed tracks</p>{#each page.folders??[] as folder}<p class="folder">{folder}</p>{/each}
    {#if page.state==='auto'}<p class="linked" role="status">Linked automatically from a unique complete tracklist match.</p>{:else if page.state==='embedded'}<p class="linked" role="status">Linked using an embedded release identity.</p>{:else if page.state==='confirmed'}<p class="linked" role="status">You confirmed this album match.</p>{:else if page.state==='needs-review'}<p role="status">The files or available editions changed. Review the match.</p>{:else if page.state==='rejected'}<p role="status">These local files are kept separate until you confirm a match.</p>{/if}
  {/if}
  <label>Find a saved album<input type="search" bind:value={query} disabled={applying} placeholder="Album or artist" /></label>
  {#if error}<p role="alert">{error}</p>{/if}
  {#if loading}<p role="status">Checking saved albums…</p>{:else if page}
    <div class="candidates">{#each page.candidates as candidate(candidate.album.id)}<article><div><strong>{candidate.album.name}</strong><p>{candidate.album.artist} · {candidate.album.releaseDate?.slice(0,4)??'Year unknown'} · {candidate.album.provider??'Catalog'} · {candidate.catalogTracks} tracks</p><p>{candidate.matchedTracks} of {candidate.localTracks} local tracks matched. {candidate.reason}</p>{#if candidate.album.sourceUrl}<a href={candidate.album.sourceUrl} target="_blank" rel="noreferrer">Inspect source edition ↗</a>{/if}</div><button disabled={!candidate.canLink||applying} onclick={()=>void choose(candidate)}>{candidate.completeMatch?'Use this album':'Confirm album match'}</button></article>{:else}<p>No saved album matches these files yet. Save the correct catalog album, then check again.</p>{/each}</div>
    <footer><button disabled={applying} onclick={()=>void choose(null)}>{['auto','embedded','confirmed'].includes(page.state)?'Unlink and keep separate':'Keep separate'}</button><button disabled={applying} onclick={onClose}>Done</button></footer>
  {/if}
</dialog>
<style>
  .local-album-match{box-sizing:border-box;width:min(720px,calc(100vw - 32px));max-height:85vh;overflow:auto;border:1px solid var(--line);border-radius:12px;padding:20px;background:var(--surface,#202328);color:var(--text,#eee)}.local-album-match::backdrop{background:#0009}header,footer{display:flex;align-items:start;justify-content:space-between;gap:12px}header h2{margin:0}p{color:var(--muted);font-size:.9rem;line-height:1.4}label{display:grid;gap:6px;margin:12px 0}.folder{font-size:.8rem;overflow-wrap:anywhere}.candidates{display:grid;gap:10px}article{display:flex;align-items:start;justify-content:space-between;gap:12px;padding:12px;border:1px solid var(--line);border-radius:8px}article p{margin:5px 0}article button{flex:none}a{color:var(--accent)}.linked{color:var(--accent)}footer{margin-top:16px}button,input,select{font:inherit}button{cursor:pointer}@media(max-width:600px){article{flex-direction:column}}
</style>
