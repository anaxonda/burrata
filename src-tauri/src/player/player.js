(() => {
  const token = new URL(location.href).searchParams.get('token');
  const title = document.getElementById('title'), message = document.getElementById('message');
  let session = null, player = null, timer = null, startupTimer = null, autoplayTimer = null, generation = 0, ready = false;
  const loaded = new Map();
  function script(url) {
    if (!loaded.has(url)) loaded.set(url, new Promise((resolve, reject) => {
      const el = document.createElement('script'); el.src = url; el.onload = resolve;
      el.onerror = () => reject(new Error('Player API unavailable. Check your network and retry.'));
      document.head.appendChild(el);
    }));
    return loaded.get(url);
  }
  function status(data = {}, g = generation) {
    if (!session || g !== generation) return;
    const body = {sessionId: session.sessionId, paused:true, position:0, duration:0, canSeek:ready, canVolume:ready, ...data};
    fetch('/player/status', {method:'POST', headers:{'Authorization':`Bearer ${token}`, 'Content-Type':'application/json'}, body:JSON.stringify(body)}).catch(() => {});
  }
  function dispose() {
    clearInterval(timer); clearTimeout(startupTimer); clearTimeout(autoplayTimer);
    timer = null; startupTimer = null; autoplayTimer = null; ready = false;
    if (player) {try {player.destroy ? player.destroy() : player.pause();} catch (_) {} }
    player = null; document.getElementById('player').replaceChildren();
  }
  function playbackPending(g) {
    clearTimeout(autoplayTimer);
    message.textContent = 'Starting candidate preview…';
    autoplayTimer = setTimeout(() => {
      if (g === generation && ready) {
        message.textContent = 'Playback has not started automatically. Press Play below to try again.';
        status({error:message.textContent},g);
      }
    }, 5000);
  }
  function playbackStarted(g) {
    if (g !== generation) return;
    clearTimeout(autoplayTimer); autoplayTimer = null;
    message.textContent = 'Playing candidate preview.';
  }
  function autoplayBlocked(g) {
    if (g !== generation) return;
    clearTimeout(autoplayTimer); autoplayTimer = null;
    message.textContent = 'Autoplay was blocked. Press Play below to start the embedded player.';
    status({error:message.textContent},g);
  }
  async function start(data) {
    generation++; const g = generation; dispose(); session = data;
    startupTimer = setTimeout(() => { if (g === generation && !ready) {message.textContent='Player did not become ready. Check connectivity or open the source in your browser.';status({error:message.textContent},g);} }, 15000);
    title.textContent = data.title; message.textContent = 'Loading player…';
    try {
      if (data.provider === 'youtube') {
        if (!window.YT?.Player) {
          const apiReady = new Promise(resolve => {window.onYouTubeIframeAPIReady = resolve;});
          await script('https://www.youtube.com/iframe_api');
          if (!window.YT?.Player) await apiReady;
        }
        if (g !== generation) return;
        const mount = document.createElement('div'); mount.id = `video-${g}`; document.getElementById('player').appendChild(mount);
        player = new YT.Player(mount.id, {width:'100%',height:270,videoId:data.mediaId,playerVars:{origin:location.origin,playsinline:1,controls:1,autoplay:0},events:{
          onReady: e => {
            if (g !== generation) {e.target.destroy();return;}
            clearTimeout(startupTimer); startupTimer=null; ready=true; playbackPending(g); status({},g);
            try {e.target.playVideo();} catch (_) {autoplayBlocked(g);}
          },
          onStateChange: e => {if (g === generation) {if (e.data === 1) playbackStarted(g); reportYouTube(g);}},
          onAutoplayBlocked: () => autoplayBlocked(g),
          onError: e => {if (g === generation) {clearTimeout(startupTimer);clearTimeout(autoplayTimer);autoplayTimer=null;const errors={2:'Invalid video.',5:'This video cannot play in this browser.',100:'Video unavailable or private.',101:'The uploader disabled embedding.',150:'The uploader disabled embedding.',153:'YouTube could not verify this player. Open the source in your browser.'};message.textContent=errors[e.data]||'YouTube playback failed.';status({error:message.textContent},g);}}
        }});
        timer = setInterval(() => reportYouTube(g), 500);
      } else {
        await script('https://w.soundcloud.com/player/api.js'); if (g !== generation) return;
        const frame=document.createElement('iframe'); frame.allow='autoplay';frame.title='SoundCloud candidate recording';
        frame.src=`https://w.soundcloud.com/player/?url=${encodeURIComponent(data.url)}&auto_play=false&show_artwork=true`;
        document.getElementById('player').appendChild(frame); player=SC.Widget(frame);
        let duration=0, position=0, paused=true, lastProgressReport=0;
        const report=()=>status({duration,position,paused},g);
        player.bind(SC.Widget.Events.READY,()=>{
          if(g!==generation)return;
          clearTimeout(startupTimer); startupTimer=null; ready=true; playbackPending(g);
          player.getDuration(ms=>{duration=ms/1000;report();});report();
          try {player.play();} catch (_) {autoplayBlocked(g);}
        });
        player.bind(SC.Widget.Events.PLAY,()=>{paused=false;playbackStarted(g);report();});player.bind(SC.Widget.Events.PAUSE,()=>{paused=true;report();});
        player.bind(SC.Widget.Events.FINISH,()=>{paused=true;report();});
        player.bind(SC.Widget.Events.PLAY_PROGRESS,e=>{
          position=e.currentPosition/1000;
          const now=Date.now();
          if(now-lastProgressReport>=500){lastProgressReport=now;report();}
        });
        player.bind(SC.Widget.Events.ERROR,()=>{if(g!==generation)return;clearTimeout(startupTimer);clearTimeout(autoplayTimer);startupTimer=null;autoplayTimer=null;message.textContent='SoundCloud recording unavailable or embedding is disabled.';status({error:message.textContent},g);});
      }
    } catch(error) {if(g===generation){clearTimeout(startupTimer);clearTimeout(autoplayTimer);autoplayTimer=null;message.textContent=error.message;status({error:error.message},g);}}
  }
  function reportYouTube(g) {
    if(g!==generation||!ready||!player?.getPlayerState)return;
    status({paused:player.getPlayerState()!==1,position:player.getCurrentTime()||0,duration:player.getDuration()||0},g);
  }
  function control(data) {
    if(data.action==='stop'){generation++;dispose();session=null;message.textContent='Playback stopped.';return;}
    if(!session || data.sessionId!==session.sessionId || !ready)return;
    const yt=session.provider==='youtube';
    if(data.action==='play') {playbackPending(generation);try {yt?player.playVideo():player.play();} catch (_) {autoplayBlocked(generation);}}
    if(data.action==='pause') yt?player.pauseVideo():player.pause();
    if(data.action==='seek') yt?player.seekTo(data.value,true):player.seekTo(data.value*1000);
    if(data.action==='volume') player.setVolume(data.value*100);
  }
  window.burrataCandidateCommand=data=>data.action==='start'?start(data):control(data);
  document.getElementById('play').onclick=()=>control({action:'play',sessionId:session?.sessionId});
  document.getElementById('pause').onclick=()=>control({action:'pause',sessionId:session?.sessionId});
  fetch('/player/session',{headers:{Authorization:`Bearer ${token}`}}).then(r=>r.ok?r.json():Promise.reject()).then(data=>{if(generation===0&&!session)start(data);}).catch(()=>{if(generation===0)message.textContent='No active candidate. Choose Preview in Burrata.';});
})();
