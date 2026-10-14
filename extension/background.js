const api=globalThis.browser||chrome;
const sites=['https://www.beatport.com/*','https://www.traxsource.com/*','https://open.spotify.com/*','https://www.shazam.com/*','https://shazam.com/*','https://www.last.fm/*','https://last.fm/*','https://bandcamp.com/*','https://*.bandcamp.com/*'];
let port=null,waiters=[],nativeBusy=Promise.resolve(),pollBusy=false;
let lastError='',requestTabs=new Map(),enabled=false;
// Chromium may suspend its MV3 worker while the user completes verification.
// Session storage survives worker restarts without retaining stale tab IDs after
// the browser itself restarts.
const ready=Promise.all([api.storage.local.get('enabled'),api.storage.session.get('requestTabs')]).then(([settings,session])=>{
  enabled=!!settings.enabled;
  requestTabs=new Map(session.requestTabs||[]);
});
async function rememberRequest(tabId,requestId){requestTabs.set(tabId,requestId);await api.storage.session.set({requestTabs:[...requestTabs]});}
async function forgetRequest(tabId){requestTabs.delete(tabId);await api.storage.session.set({requestTabs:[...requestTabs]});}
function badge(text){api.action.setBadgeText({text});api.action.setBadgeBackgroundColor({color:'#bd5734'});}
function disconnect(error){port=null;lastError=error;badge(error?'!':'');for(const w of waiters.splice(0)){clearTimeout(w.timeout);w.reject(new Error(error));}}
function connect(){
  if(port)return;
  port=api.runtime.connectNative('org.burrata.browser');
  port.onMessage.addListener(reply=>{const w=waiters.shift();if(w){clearTimeout(w.timeout);reply.ok?w.resolve(reply):w.reject(new Error(reply.error||'Native host failed'));}});
  port.onDisconnect.addListener(()=>disconnect(!enabled&&!waiters.length?'':api.runtime.lastError?.message||'Native host disconnected. Open Burrata and install its browser host.'));
}
function native(message){
  const run=()=>new Promise((resolve,reject)=>{
    try{if(new TextEncoder().encode(JSON.stringify(message)).length>1024*1024)throw new Error('Capture exceeds the 1 MiB bridge limit. Import a smaller chart, playlist, or album.');connect();const limit=message.type==='importAlbumCollection'?120000:30000;const w={resolve,reject,timeout:setTimeout(()=>{port?.disconnect();disconnect(`Burrata did not reply within ${Math.round(limit/1000)} seconds.`);},limit)};waiters.push(w);port.postMessage(message);}catch(e){reject(e);}
  });
  const result=nativeBusy.then(run,run);nativeBusy=result.catch(()=>{});return result;
}
async function verification(tabId,requestId){
  await api.tabs.update(tabId,{active:true});const tab=await api.tabs.get(tabId);await api.windows.update(tab.windowId,{focused:true});
  if(requestId)await native({type:'status',requestId,state:'verification',message:'Complete provider login or verification in the browser, then click Import current page.'});
  throw new Error('Complete login or verification in this tab, then click Import current page.');
}
async function capture(tabId, requestId){
  const current=await api.tabs.get(tabId);
  if(!supported(current.url)||/\/(?:login|signin|authorize)(?:\/|$)/i.test(new URL(current.url).pathname))return verification(tabId,requestId);
  await api.scripting.executeScript({target:{tabId},files:['content.js']});
  let result;
  for(let i=0;i<5;i++){
    try{result=await api.tabs.sendMessage(tabId,{type:'capture'});break;}catch(e){
      if(i===4)throw new Error('The extension could not connect to this page. Reload the extension and the provider tab, then retry.');
      // Retry listener startup only. Content capture already owns its bounded loading wait.
      await new Promise(r=>setTimeout(r,250));
    }
  }
  if(result?.verification){
    return verification(tabId,requestId);
  }
  if(result?.error)throw new Error(result.error);
  let response;
  if(result?.albumCollection){
    const collection=result.albumCollection;
    response=await native({type:'importAlbumCollection',...(requestId?{requestId}:{}),sourceUrl:collection.sourceUrl,title:collection.title,albumUrls:collection.albumUrls});
  }else if(result?.snapshot){
    response=await native({type:'import',...(requestId?{requestId}:{}),snapshot:result.snapshot});
  }else throw new Error('Page did not return a chart, playlist, album, or Bandcamp album collection.');
  await forgetRequest(tabId);badge('✓');lastError='';return response.result;
}
async function waitLoaded(tabId){
  for(let i=0;i<40;i++){const t=await api.tabs.get(tabId);if(t.status==='complete'){await new Promise(r=>setTimeout(r,1200));return;}await new Promise(r=>setTimeout(r,500));}
  throw new Error('Provider page took too long to load.');
}
function supported(url){try{const u=new URL(url),host=u.hostname.toLowerCase();return u.protocol==='https:'&&(['www.beatport.com','beatport.com','www.traxsource.com','traxsource.com','open.spotify.com','www.shazam.com','shazam.com','www.last.fm','last.fm','bandcamp.com'].includes(host)||host.endsWith('.bandcamp.com'));}catch{return false;}}
async function poll(){
  await ready;
  if(pollBusy)return;pollBusy=true;
  try{
    const reply=await native({type:enabled?'poll':'heartbeat'});
    if(!enabled)return;
    for(const request of reply.requests||[]){
      if(!supported(request.url)){await native({type:'status',requestId:request.id,state:'failed',message:'Unsupported provider URL.'});continue;}
      if(!await api.permissions.contains({origins:[new URL(request.url).origin+'/*']})){await native({type:'status',requestId:request.id,state:'failed',message:'Enable provider permissions in the Burrata extension.'});continue;}
      let tab;
      try{
        await native({type:'status',requestId:request.id,state:'loading',message:'Loading chart in the browser.'});
        tab=await api.tabs.create({url:request.url,active:false});await rememberRequest(tab.id,request.id);
        await waitLoaded(tab.id);await capture(tab.id,request.id);
      }catch(e){
        lastError=e.message;badge('!');
        // Verification remains pending for a user-initiated capture.
        if(!/verification/i.test(e.message)) {if(tab)await forgetRequest(tab.id);await native({type:'status',requestId:request.id,state:'failed',message:e.message});}
      }
    }
  }catch(e){lastError=e.message;badge('!');}finally{pollBusy=false;}
}
api.runtime.onMessage.addListener((message,_sender,respond)=>{
  const handle=async()=>{
    await ready;
    if(message.type==='status'){
      try{await native({type:'heartbeat'});lastError='';}catch(error){lastError=error.message;}
      return {enabled,error:lastError,connected:!!port};
    }
    if(message.type==='enable'){enabled=message.enabled;await api.storage.local.set({enabled});if(enabled)await poll();else port?.disconnect();return {enabled};}
    if(message.type==='diagnosticCurrent'){
      const [tab]=await api.tabs.query({active:true,currentWindow:true});
      if(!tab?.id||!supported(tab.url))throw new Error('Open a supported provider chart, playlist, or album page.');
      await api.scripting.executeScript({target:{tabId:tab.id},files:['content.js']});
      let result;
      for(let i=0;i<5;i++){
        try{result=await api.tabs.sendMessage(tab.id,{type:'diagnostic'});break;}catch(error){
          if(i===4)throw new Error('Could not read this page. Reload the extension and provider tab, then retry.');
          await new Promise(resolve=>setTimeout(resolve,250));
        }
      }
      if(result?.error)throw new Error(result.error);
      if(!result?.diagnostic)throw new Error('This page did not return a parser diagnostic.');
      // Diagnostic export is local to the popup. Never forward it to native messaging.
      return {diagnostic:result.diagnostic};
    }
    if(message.type==='importCurrent'){
      const [tab]=await api.tabs.query({active:true,currentWindow:true});if(!tab?.id||!supported(tab.url))throw new Error('Open a supported provider page.');
      return {result:await capture(tab.id,requestTabs.get(tab.id))};
    }
    throw new Error('Unknown action');
  };
  handle().then(r=>respond({ok:true,...r}),e=>{lastError=e.message;badge('!');respond({ok:false,error:e.message});});return true;
});
api.tabs.onRemoved.addListener(tabId=>{
  void ready.then(async()=>{
    const requestId=requestTabs.get(tabId);
    if(!requestId)return;
    await forgetRequest(tabId);
    await native({type:'status',requestId,state:'failed',message:'The import tab was closed. Submit the URL again to retry.'});
  }).catch(e=>{lastError=e.message;badge('!');});
});
// MV3 native connection keeps the service worker alive; timer never polls faster than 2 seconds.
setInterval(poll,2000);
api.alarms.create('burrata-wake',{periodInMinutes:1});
api.alarms.onAlarm.addListener(()=>{poll();});
