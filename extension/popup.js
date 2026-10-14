const api=globalThis.browser||chrome,status=document.querySelector('#status'),enable=document.querySelector('#enable'),importButton=document.querySelector('#import');let enabled=false;
async function send(message){const r=await api.runtime.sendMessage(message);if(!r.ok)throw new Error(r.error);return r;}
function show(e){status.textContent=e.message||String(e);}
send({type:'status'}).then(r=>{enabled=r.enabled;enable.textContent=enabled?'Disable app-driven imports':'Enable app-driven imports';status.textContent=r.error||(r.connected?(enabled?'Companion connected; app-driven imports enabled.':'Companion connected; app-driven imports disabled.'):'Companion disconnected. Open Burrata and install the browser host.');}).catch(show);
importButton.onclick=async()=>{importButton.disabled=true;status.textContent='Collecting visible track metadata…';try{const r=await send({type:'importCurrent'});status.textContent=`Imported ${r.result.importedCount} tracks.\n${(r.result.warnings||[]).join('\n')}`;}catch(e){show(e);}finally{importButton.disabled=false;}};
enable.onclick=async()=>{
  try{
    const next=!enabled;
    if(next&&!await api.permissions.request({origins:['https://www.beatport.com/*','https://www.traxsource.com/*','https://open.spotify.com/*','https://www.shazam.com/*','https://shazam.com/*','https://www.last.fm/*','https://last.fm/*','https://bandcamp.com/*','https://*.bandcamp.com/*']}))throw new Error('Provider permission was declined. Current-page imports remain available.');
    await send({type:'enable',enabled:next});enabled=next;enable.textContent=enabled?'Disable app-driven imports':'Enable app-driven imports';status.textContent=enabled?'App-driven imports enabled.':'App-driven imports disabled.';
  }catch(e){show(e);}
};

const diagnosticButton=document.querySelector('#diagnostic');
diagnosticButton.onclick=async()=>{
  diagnosticButton.disabled=true;importButton.disabled=true;
  status.textContent='Preparing a sanitized public row diagnostic…';
  try{
    const {diagnostic}=await send({type:'diagnosticCurrent'});
    const provider=['beatport','traxsource','spotify','shazam','lastfm','bandcamp'].includes(diagnostic.provider)?diagnostic.provider:'provider';
    const filename=`burrata-${provider}-diagnostic.json`;
    const blob=new Blob([JSON.stringify(diagnostic,null,2)+'\n'],{type:'application/json'});
    const url=URL.createObjectURL(blob),anchor=document.createElement('a');
    anchor.href=url;anchor.download=filename;document.body.append(anchor);anchor.click();anchor.remove();
    setTimeout(()=>URL.revokeObjectURL(url),10000);
    status.textContent=`Saved ${filename}. Attach the locally saved file when reporting this parser failure. It contains sanitized public row data; cookies, JSON scripts and forms are excluded.`;
  }catch(error){show(error);}finally{diagnosticButton.disabled=false;importButton.disabled=false;}
};
