(async()=>{
  const api=globalThis.browser||chrome;
  const revision='2026-10-12-diagnostic-v4';
  const previous=globalThis.burrataCaptureState;
  if(previous){
    previous.cancelled=true;
    if(previous.listener){try{api.runtime.onMessage.removeListener(previous.listener);}catch{/* previous extension context may have been unloaded */}}
  }
  // The legacy boolean must not prevent a fresh extension from installing its listener.
  const state={revision,cancelled:false,listener:null,busy:false};
  globalThis.burrataCaptureState=state;
  const {extract,mergeSnapshots,isVerification,diagnosticSample}=await import(`${api.runtime.getURL('parsers.js')}?capture=${revision}-${Date.now()}`);
  if(state.cancelled)return;
  const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
  const checkCurrent=()=>{if(state.cancelled)throw new Error('The extension was reloaded during capture. Click Import current page again.');};
  const gated=()=>isVerification(document)||!!document.querySelector('input[type="password"],form[data-testid="login-form"]');
  function findSpotifyTrackRow(){
    const roots=[document.querySelector('main'),document.body].filter((node,index,nodes)=>node&&nodes.indexOf(node)===index);
    for(const root of roots){
      const find=selector=>[...(root.matches?.(selector)?[root]:[]),...root.querySelectorAll(selector)].find(row=>row.querySelector('a[href*="/track/"]'));
      const trackRow=find('[data-testid="tracklist-row"]')||find('[role="row"][aria-rowindex]');
      if(trackRow)return trackRow;
    }
    return null;
  }
  function findYouTubePlaylistRow(){
    return document.querySelector('ytd-playlist-video-renderer, ytmusic-responsive-list-item-renderer, yt-lockup-view-model');
  }
  const scrollAncestors=row=>{
    const nodes=[];
    for(let node=row;node && node!==document.documentElement;node=node.parentElement)nodes.push(node);
    if(document.scrollingElement && !nodes.includes(document.scrollingElement))nodes.push(document.scrollingElement);
    return nodes;
  };
  const scrollInfo=node=>{
    const style=getComputedStyle(node), overflow=style.overflowY||style.overflow||'';
    return {node,overflow,scrollHeight:node.scrollHeight||0,clientHeight:node.clientHeight||0,scrollTop:node.scrollTop||0,
      overlay:node.hasAttribute?.('data-overlayscrollbars-viewport')||node.classList?.contains('os-viewport')||false};
  };
  function findSpotifyScroller(row){
    const candidates=scrollAncestors(row).map(scrollInfo);
    for(const candidate of candidates){
      const {node,overflow,scrollHeight,clientHeight,overlay}=candidate;
      if(scrollHeight<=clientHeight+20)continue;
      if(!overlay && !/^(auto|scroll|overlay|hidden)$/.test(overflow.trim()))continue;
      // Probe programmatic scrolling. This includes OverlayScrollbars viewports
      // that report overflow:hidden, while rejecting tall, non-scrollable wrappers.
      const before=node.scrollTop||0, max=Math.max(0,scrollHeight-clientHeight);
      const probe=before<max?Math.min(max,before+24):Math.max(0,before-24);
      node.scrollTop=probe;
      const after=node.scrollTop||0;
      node.scrollTop=before;
      if(after===before)continue;
      return node;
    }
    return document.scrollingElement||document.documentElement;
  }
  function spotifyScrollDiagnostics(row){
    return scrollAncestors(row).slice(0,12).map(node=>{
      const {overflow,scrollHeight,clientHeight,scrollTop,overlay}=scrollInfo(node);
      return {tag:node.tagName?.toLowerCase()||null,role:node.getAttribute?.('role')||null,
        testId:(node.getAttribute?.('data-testid')||'').slice(0,100),overflowY:String(overflow).slice(0,30),
        scrollHeight,clientHeight,scrollTop,overlayViewport:overlay};
    });
  }
  function noRowsError(provider){
    // Diagnostic counts identify parser/loading failures without exposing page content.
    const trackLinks=document.querySelectorAll('a[href*="/track/"]').length;
    const selectors={beatport:'[data-testid*="track"],[data-track-id],tr,[role="row"]',traxsource:'.trk-row',spotify:'[data-testid="tracklist-row"],[role="row"][aria-rowindex]',youtube:'ytd-playlist-video-renderer,ytmusic-responsive-list-item-renderer,yt-lockup-view-model',shazam:'a[href*="/song/"]',lastfm:'.chartlist-row,[data-testid="chartlist-row"],tr,[role="row"]',bandcamp:'.track_row_view,[data-testid="track-row"]'};
    const rows=document.querySelectorAll(selectors[provider]||'a[href]').length;
    const name={beatport:'Beatport',traxsource:'Traxsource',spotify:'Spotify',youtube:'YouTube',shazam:'Shazam',lastfm:'Last.fm',bandcamp:'Bandcamp'}[provider]||'Provider';
    const jsonScripts=provider==='beatport'?document.querySelectorAll('script#__NEXT_DATA__,script[type="application/json"]').length:null;
    const counts=`${trackLinks} track links, ${rows} row candidates${jsonScripts===null?'':`, ${jsonScripts} metadata scripts`}`;
    return new Error(`${name}: no supported tracks after waiting 8 seconds (${counts}; capture ${revision}). Open the actual ${provider==='spotify'?'playlist or album':'chart'} page and let tracks load. If tracks are visible, reload the extension and provider tab, then retry; the provider page structure may have changed.`);
  }
  state.listener=(message,_sender,respond)=>{
    if(message.type==='diagnostic'){
      try{
        checkCurrent();const diagnostic=diagnosticSample(document,location.href);
        if(diagnostic.provider==='spotify'){
          const row=findSpotifyTrackRow();
          if(row)diagnostic.scrollAncestors=spotifyScrollDiagnostics(row);
        }
        respond({diagnostic});
      }catch(error){respond({error:error.message});}
      return;
    }
    if(message.type!=='capture')return;
    (async()=>{
      if(state.busy)throw new Error('A capture is already running in this tab.');state.busy=true;
      const saved=[];let scroller=null,original=0;
      try{
        const loadingDeadline=Date.now()+8000;
        let first,lastParseError=null;
        while(true){
          checkCurrent();if(gated())return {verification:true};
          try{first=extract(document,location.href);lastParseError=null;}catch(error){
            const retryable=/Shazam chart rows were not available|Last\.fm track rows were not available|Bandcamp did not expose an explicit album tracklist|Bandcamp album metadata did not contain importable songs|Bandcamp Discover did not show rendered album cards|YouTube video title or channel credit is not rendered|YouTube playlist videos were not available|Wait for the public playlist rows to load/i.test(error.message);
            if(!retryable||Date.now()>=loadingDeadline)throw error;
            lastParseError=error;await delay(250);continue;
          }
          if(first.albumCollection)return {albumCollection:first.albumCollection};
          if(first.entries.length||(first.provider==='youtube'&&first.kind==='playlist'&&first.expectedTotal===0&&first.complete))break;
          if(Date.now()>=loadingDeadline)throw lastParseError||noRowsError(first.provider);
          await delay(250);
        }
        saved.push(first);
        if(first.provider==='spotify'||(first.provider==='youtube'&&first.kind==='playlist'&&first.expectedTotal!==0)) {
          const row=first.provider==='spotify'?findSpotifyTrackRow():findYouTubePlaylistRow();
          scroller=row?findSpotifyScroller(row):(document.scrollingElement||document.documentElement);
          original=scroller.scrollTop||0;scroller.scrollTop=0;
          const deadline=Date.now()+25000,seen=new Set();let bottomQuiet=0;
          const remember=snapshot=>snapshot.entries.forEach(e=>seen.add(`${e.position}:${e.providerId}`));
          remember(first);
          while(Date.now()<deadline){
            await delay(400);checkCurrent();if(gated())return {verification:true};
            const snapshot=extract(document,location.href);saved.push(snapshot);
            const before=seen.size;remember(snapshot);const gained=seen.size>before;
            const max=Math.max(0,(scroller.scrollHeight||0)-(scroller.clientHeight||0));
            const atBottom=max===0 || (scroller.scrollTop||0)>=(max-8);
            bottomQuiet=atBottom&&!gained?bottomQuiet+1:0;
            if(bottomQuiet>=3)break;
            const step=Math.max(200,(scroller.clientHeight||300)*.7);
            const current=scroller.scrollTop||0;
            scroller.scrollTop=Math.min(max,current+step);
          }
        }
        const snapshot=mergeSnapshots(saved);
        return {snapshot};
      }finally{if(scroller)scroller.scrollTop=original;state.busy=false;}
    })().then(respond,error=>respond({error:error.message}));
    return true;
  };
  api.runtime.onMessage.addListener(state.listener);
})().catch(error=>{
  // Registration failures are surfaced as a safe runtime error by the background.
  globalThis.burrataCaptureLoadError=error.message;
});
