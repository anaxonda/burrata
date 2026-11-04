// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { measurePlaybackLayout, measureTrackListCounts, type PlaybackLayoutRole } from "./playback-layout";

const boxes: Partial<Record<PlaybackLayoutRole, {x:number;y:number;width:number;height:number;clientWidth:number;scrollWidth:number;display?:string}>> = {
  footer: {x:0,y:150,width:320,height:110,clientWidth:320,scrollWidth:320},
  now_playing: {x:0,y:150,width:180,height:40,clientWidth:180,scrollWidth:205},
  transport: {x:164,y:151,width:140,height:35,clientWidth:140,scrollWidth:140},
  scrubber: {x:0,y:190,width:180,height:10,clientWidth:180,scrollWidth:180},
  right: {x:278,y:190,width:42,height:10,clientWidth:42,scrollWidth:42},
  quality: {x:38,y:184,width:150,height:12,clientWidth:150,scrollWidth:168},
  error: {x:0,y:202,width:320,height:50,clientWidth:320,scrollWidth:320,display:"none"},
};

function fixtureRoot() {
  const root=document.createElement("main");
  root.innerHTML=`<footer class="player">
    <button class="now-playing" data-source-url="https://private.example/track">PRIVATE TITLE · PRIVATE ARTIST<span class="playback-quality"></span></button>
    <div class="player-center"><div class="player-transport"></div><div class="player-scrubber"></div></div>
    <div class="player-right"></div>
    <div class="player-error" role="alert"></div>
  </footer>`;
  const roles: Record<string,PlaybackLayoutRole> = {
    "footer.player":"footer",
    "footer.player > .now-playing":"now_playing",
    "footer.player .player-transport":"transport",
    "footer.player .player-scrubber":"scrubber",
    "footer.player .player-right":"right",
    "footer.player .now-playing .playback-quality":"quality",
    "footer.player .player-error[role='alert']":"error",
  };
  for (const [selector,role] of Object.entries(roles)) {
    const element=root.querySelector(selector);
    if (!element) continue;
    const box=boxes[role]!;
    Object.defineProperties(element,{
      getBoundingClientRect:{value:()=>({left:box.x,top:box.y,width:box.width,height:box.height,right:box.x+box.width,bottom:box.y+box.height})},
      clientWidth:{value:box.clientWidth},
      scrollWidth:{value:box.scrollWidth},
      getClientRects:{value:()=>box.display==="none"?[]:[{}]},
    });
    Object.defineProperty(element,"__role",{value:role});
  }
  return root;
}

describe("measurePlaybackLayout",()=>{
  it("captures only fixed player roles, computed visibility, and real geometry",()=>{
    const report=measurePlaybackLayout(fixtureRoot() as never,{width:320,height:200},element=>({display:boxes[(element as unknown as {__role:PlaybackLayoutRole}).__role]?.display??"block",visibility:"visible",opacity:"1"}));
    expect(report.viewport).toEqual({width:320,height:200});
    expect(report.footerHeight).toBe(110);
    const nowPlaying=report.elements.find(item=>item.role==="now_playing")!;
    const transport=report.elements.find(item=>item.role==="transport")!;
    expect(nowPlaying.x+nowPlaying.width).toBeGreaterThan(transport.x); // deliberate 16px UI overlap fixture
    const right=report.elements.find(item=>item.role==="right")!;
    const footer=report.elements.find(item=>item.role==="footer")!;
    expect(right.x).toBeGreaterThanOrEqual(footer.x);
    expect(right.x+right.width).toBeLessThanOrEqual(footer.x+footer.width); // right-side controls remain inside the player bounds
    expect(report.elements.map(item=>item.role)).toEqual(["footer","now_playing","transport","scrubber","right","quality","error","queue_panel","queue_header","queue_title","identify","queue_close","panel_close","expanded_player","expanded_artwork","expanded_close","expanded_cards"]);
    expect(report.elements.find(item=>item.role==="now_playing")).toMatchObject({visible:true,x:0,width:180,clientWidth:180,scrollWidth:205});
    expect(report.elements.find(item=>item.role==="error")).toMatchObject({visible:false});
    expect(JSON.stringify(report)).not.toMatch(/"(?:title|artist|sourceUrl)"\s*:|PRIVATE TITLE|PRIVATE ARTIST|https?:/i);
  });

  it("measures the actionable failure row inside the footer rather than a global banner",()=>{
    boxes.error!.display="block";
    const root=fixtureRoot();
    const report=measurePlaybackLayout(root as never,{width:320,height:260},element=>({display:element.getAttribute("role")==="alert"?"block":"block",visibility:"visible",opacity:"1"}));
    const error=report.elements.find(item=>item.role==="error")!;
    expect(error).toMatchObject({present:true,visible:true,x:0,y:202,width:320,height:50});
    expect(root.querySelector(".banner.error[role='alert']")).toBeNull();
    boxes.error!.display="none";
  });

  it("measures the queue header, title, Identify, and close controls at the 280px minimum",()=>{
    const root=document.createElement("main");
    root.innerHTML=`<aside class="playback-queue-panel"><header class="queue-header"><div><p class="eyebrow">PLAYER</p><h2>Queue</h2></div><button class="identify">Identify audio</button><button class="close" aria-label="Close playback queue">×</button></header></aside><nav class="player-panel-tabs"><button aria-label="Close player panel">×</button></nav>`;
    const geometry: Record<string,{x:number;y:number;width:number;height:number;client:number;scroll:number}>={
      "aside.playback-queue-panel":{x:40,y:10,width:280,height:500,client:280,scroll:280},
      "aside.playback-queue-panel > header.queue-header":{x:41,y:11,width:278,height:64,client:278,scroll:278},
      "aside.playback-queue-panel > header.queue-header h2":{x:58,y:25,width:64,height:22,client:64,scroll:64},
      "aside.playback-queue-panel > header.queue-header button.identify":{x:136,y:24,width:82,height:34,client:82,scroll:82},
      "aside.playback-queue-panel > header.queue-header button.close":{x:232,y:24,width:24,height:34,client:24,scroll:24},
      ".player-panel-tabs button[aria-label='Close player panel']":{x:270,y:82,width:28,height:34,client:28,scroll:28},
    };
    for(const [selector,box] of Object.entries(geometry)){
      const element=root.querySelector(selector)!;
      Object.defineProperties(element,{
        getBoundingClientRect:{value:()=>({left:box.x,top:box.y,width:box.width,height:box.height,right:box.x+box.width,bottom:box.y+box.height})},
        clientWidth:{value:box.client},scrollWidth:{value:box.scroll},getClientRects:{value:()=>[{}]},
      });
    }
    const report=measurePlaybackLayout(root as never,{width:320,height:600},()=>({display:"block",visibility:"visible",opacity:"1"}));
    expect(report.elements.find(item=>item.role==="queue_panel")).toMatchObject({visible:true,x:40,width:280,clientWidth:280,scrollWidth:280});
    expect(report.elements.find(item=>item.role==="queue_title")).toMatchObject({visible:true,x:58,width:64});
    expect(report.elements.find(item=>item.role==="identify")).toMatchObject({visible:true,x:136,width:82});
    expect(report.elements.find(item=>item.role==="panel_close")).toMatchObject({visible:true,width:28});
    expect(report.elements.find(item=>item.role==="queue_close")).toMatchObject({visible:true,width:24});
  });

  it("marks absent optional quality/error rows without inventing a rectangle",()=>{
    const empty={querySelector:(selector:string)=>selector==="footer.player"?fixtureRoot().querySelector(selector):null,querySelectorAll:()=>[]};
    const report=measurePlaybackLayout(empty as never,{width:320,height:200},()=>({display:"none",visibility:"hidden",opacity:"0"}));
    expect(report.elements.find(item=>item.role==="quality")).toMatchObject({present:false,visible:false,width:0,scrollWidth:0});
  });
});


describe("measureTrackListCounts",()=>{
  it("counts shared Library rows while excluding virtual spacers and clipped rows",()=>{
    const root=document.createElement("main");
    root.innerHTML=`<section class="track-area"><div class="track-scroll desktop-track-list"><div class="track-list-viewport"><ol class="shared-track-list">
      <li class="current paused" data-track-id="PRIVATE-ID" title="PRIVATE TITLE"></li>
      <li class="current"><button aria-busy="true"></button></li>
      <li class="virtual-spacer"></li><li class="hidden current buffering"></li>
      <li class="clipped current buffering"></li>
    </ol></div></div></section>`;
    for(const element of root.querySelectorAll("*")){
      const clipped=element.classList.contains("clipped");
      Object.defineProperty(element,"getBoundingClientRect",{value:()=>({left:0,top:clipped?150:0,right:300,bottom:clipped?170:100,width:300,height:clipped?20:100})});
      Object.defineProperty(element,"getClientRects",{value:()=>[{}]});
    }
    const counts=measureTrackListCounts(root,{width:300,height:300},element=>({
      display:element.classList.contains("hidden")?"none":"block",visibility:"visible",opacity:"1",
      overflowY:element.matches(".track-scroll")?"auto":"visible",
    }));
    expect(counts.find(item=>item.role==="library")).toMatchObject({role:"library",visibleRows:2,playing:2,paused:1,buffering:1,visibleSelectedRows:0,visibleCheckboxes:0,visibleCheckedCheckboxes:0,visibleReorderHandles:0});
    expect(JSON.stringify(counts)).not.toMatch(/PRIVATE|trackId|title|url/i);
  });
  it("excludes rows clipped by an outer scrolling pane",()=>{
    const root=document.createElement("main");
    root.innerHTML='<section class="search-panel"><div class="source-results"><ol class="shared-track-list"><li class="current"></li></ol></div></section>';
    for(const element of root.querySelectorAll("*")) {
      const isPane=element.matches(".search-panel");
      const isRow=element.matches("li");
      Object.defineProperty(element,"getBoundingClientRect",{value:()=>({left:0,top:isRow?50:0,right:300,bottom:isPane?30:100,width:300,height:isPane?30:50})});
      Object.defineProperty(element,"getClientRects",{value:()=>[{}]});
    }
    const counts=measureTrackListCounts(root,{width:300,height:300},element=>({display:"block",visibility:"visible",opacity:"1",overflowY:element.matches(".search-panel")?"auto":"visible"}));
    expect(counts[0].visibleRows).toBe(0);
  });
  it("counts only visible row states in fixed regions and never exposes track payload",()=>{
    const root=document.createElement("main");
    root.innerHTML=`
      <section class="search-panel"><div class="source-results"><ol class="shared-track-list"><li class="current buffering" data-track-id="SECRET-ID" title="SECRET TITLE"></li><li></li><li class="hidden paused"></li></ol></div></section>
      <section class="track-area"><div class="track-scroll desktop-track-list"><table><tbody><tr class="current paused"><td></td></tr><tr class="current buffering"><td></td></tr><tr class="spacer"></tr></tbody></table></div></section>
      <section class="history-page"><ol class="shared-track-list"><li class="current paused"></li></ol></section>
      <section class="artist-page-host"><ol class="shared-track-list"><li class="current"></li><li class="buffering"></li></ol></section>
      <section class="remote-playlist-detail"><ol class="shared-track-list"><li class="current buffering"></li></ol></section>`;
    for(const container of root.querySelectorAll(".track-list-viewport,.track-scroll.desktop-track-list,ol.shared-track-list")){
      Object.defineProperty(container,"getBoundingClientRect",{value:()=>({left:0,top:0,right:300,bottom:300,width:300,height:300})});
    }
    for(const row of root.querySelectorAll("li,tr")){
      Object.defineProperty(row,"getBoundingClientRect",{value:()=>({left:0,top:10,right:200,bottom:34,width:200,height:24})});
      Object.defineProperty(row,"getClientRects",{value:()=>row.classList.contains("hidden")?[]:[{}]});
    }
    const counts=measureTrackListCounts(root,{width:300,height:300},element=>({display:element.classList?.contains("hidden")?"none":"block",visibility:"visible",opacity:"1"}));
    expect(counts).toMatchObject([
      {role:"search",visibleRows:2,playing:1,paused:0,buffering:1,visibleSelectedRows:0,visibleCheckboxes:0,visibleCheckedCheckboxes:0,visibleReorderHandles:0},
      {role:"library",visibleRows:2,playing:2,paused:1,buffering:1,visibleSelectedRows:0,visibleCheckboxes:0,visibleCheckedCheckboxes:0,visibleReorderHandles:0},
      {role:"history",visibleRows:1,playing:1,paused:1,buffering:0,visibleSelectedRows:0,visibleCheckboxes:0,visibleCheckedCheckboxes:0,visibleReorderHandles:0},
      {role:"artist",visibleRows:2,playing:1,paused:0,buffering:1,visibleSelectedRows:0,visibleCheckboxes:0,visibleCheckedCheckboxes:0,visibleReorderHandles:0},
      {role:"remote",visibleRows:1,playing:1,paused:0,buffering:1,visibleSelectedRows:0,visibleCheckboxes:0,visibleCheckedCheckboxes:0,visibleReorderHandles:0},
    ]);
    const serialized=JSON.stringify(counts);
    expect(serialized).not.toMatch(/SECRET|PRIVATE|trackId|title|url|https?:/i);
  });

  it("returns zero counts for absent containers and excludes rows clipped outside the viewport",()=>{
    const root=document.createElement("main");
    root.innerHTML=`<section class="search-panel"><div class="source-results"><ol class="shared-track-list"><li></li></ol></div></section>`;
    const clip=root.querySelector("ol")!;
    Object.defineProperty(clip,"getBoundingClientRect",{value:()=>({left:0,top:0,right:300,bottom:30,width:300,height:30})});
    const row=clip.querySelector("li")!;
    Object.defineProperty(row,"getBoundingClientRect",{value:()=>({left:0,top:40,right:200,bottom:60,width:200,height:20})});
    Object.defineProperty(row,"getClientRects",{value:()=>[{}]});
    const counts=measureTrackListCounts(root,{width:300,height:300},()=>({display:"block",visibility:"visible",opacity:"1"}));
    expect(counts[0]).toMatchObject({role:"search",visibleRows:0,playing:0,paused:0,buffering:0});
    expect(counts.slice(1).every(item=>item.visibleRows===0)).toBe(true);
  });

  it("reads only bounded loaded rows and visible occurrence controls from a virtualized list",()=>{
    const root=document.createElement("main");
    root.innerHTML=`<section class="track-area"><div class="track-scroll desktop-track-list"><div class="track-list-viewport"><ol class="shared-track-list" data-loaded-logical-rows="10000">
      <li class="selected"><input class="select-track" type="checkbox" checked><button class="playlist-reorder-handle"></button></li>
      <li><input class="select-track" type="checkbox"><button class="playlist-reorder-handle"></button></li>
      <li><input class="select-track" type="checkbox"><button class="playlist-reorder-handle"></button></li>
      <li class="virtual-spacer" aria-hidden="true"></li>
    </ol></div></div></section>`;
    const viewport=root.querySelector(".track-list-viewport")!;
    const desktopList=root.querySelector(".track-scroll.desktop-track-list")!;
    for(const element of root.querySelectorAll("*")){
      const row=element.closest("li:not(.virtual-spacer)");
      const rowIndex=row?[...root.querySelectorAll("li:not(.virtual-spacer)")].indexOf(row):-1;
      const top=rowIndex<0?0:10+rowIndex*30;
      const isControl=element.matches("input,button");
      const y=isControl?top+5:top;
      const isClip=element===viewport||element===desktopList;
      const height=isClip?65:(isControl?18:24);
      Object.defineProperties(element,{
        getBoundingClientRect:{value:()=>({left:0,top:isClip?0:y,right:300,bottom:(isClip?0:y)+height,width:300,height})},
        getClientRects:{value:()=>[{}]},
      });
    }
    const counts=measureTrackListCounts(root,{width:300,height:200},element=>({
      display:"block",visibility:"visible",opacity:"1",overflowY:element===viewport?"auto":"visible",
    }));
    expect(counts.find(item=>item.role==="library")).toMatchObject({
      loadedLogicalRows:10000,visibleRows:2,visibleSelectedRows:1,
      visibleCheckboxes:2,visibleCheckedCheckboxes:1,visibleReorderHandles:2,
    });
    expect(JSON.stringify(counts)).not.toMatch(/PRIVATE|trackId|title|url|https?:/i);
  });
});
