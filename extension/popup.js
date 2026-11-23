const api=globalThis.browser||chrome,status=document.querySelector('#status'),enable=document.querySelector('#enable'),importButton=document.querySelector('#import'),settingsButton=document.querySelector('#show-settings'),importView=document.querySelector('#import-view'),settingsView=document.querySelector('#settings-view'),showImportButton=document.querySelector('#show-import'),youtubePairingCode=document.querySelector('#youtube-pairing-code'),youtubeConsent=document.querySelector('#youtube-consent'),youtubeCheckAccount=document.querySelector('#youtube-check-account'),youtubeAccount=document.querySelector('#youtube-account'),youtubeAccountConsentLabel=document.querySelector('#youtube-account-consent-label'),youtubeAccountConsent=document.querySelector('#youtube-account-consent'),youtubeHandoff=document.querySelector('#youtube-handoff'),contextPlaylistSelect=document.querySelector('#context-playlist-select'),refreshTargetPlaylists=document.querySelector('#refresh-target-playlists'),contextPlaylistStatus=document.querySelector('#context-playlist-status');let enabled=false,confirmedYouTubeAccount=null,savedTargetCollectionId='';
async function send(message){const r=await api.runtime.sendMessage(message);if(!r.ok)throw new Error(r.error);return r;}
function show(e){status.textContent=e.message||String(e);}
function showView(view){const settings=view==='settings';settingsView.hidden=!settings;importView.hidden=settings;settings?document.querySelector('#settings-heading').focus():document.querySelector('#import-heading').focus();if(settings)void loadTargetPlaylists();}
settingsButton.onclick=()=>showView('settings');
showImportButton.onclick=()=>showView('import');
function addTargetOption(value,label){const option=document.createElement('option');option.value=value;option.textContent=label;contextPlaylistSelect.appendChild(option);}
async function loadTargetPlaylists(){
  refreshTargetPlaylists.disabled=true;contextPlaylistStatus.textContent='Loading saved Burrata playlists…';
  try{
    const result=await send({type:'listTargetCollections'}),collections=Array.isArray(result.collections)?result.collections:[];
    contextPlaylistSelect.replaceChildren();addTargetOption('','Choose a Burrata playlist…');
    for(const collection of collections){if(typeof collection?.id==='string'&&typeof collection?.name==='string')addTargetOption(collection.id,collection.name);}
    const target=collections.find(collection=>collection.id===savedTargetCollectionId);
    if(savedTargetCollectionId&&!target)addTargetOption(savedTargetCollectionId,'Saved playlist unavailable — choose another');
    contextPlaylistSelect.value=savedTargetCollectionId;
    contextPlaylistStatus.textContent=savedTargetCollectionId?(target?`Context menu adds tracks to ${target.name}.`:'The saved playlist is no longer available. Choose another.'):'Choose a playlist; the context menu will not create one automatically.';
  }catch(error){contextPlaylistStatus.textContent=error.message||String(error);}
  finally{refreshTargetPlaylists.disabled=false;}
}
contextPlaylistSelect.onchange=async()=>{
  try{const result=await send({type:'setTargetCollection',collectionId:contextPlaylistSelect.value});savedTargetCollectionId=result.targetCollectionId;contextPlaylistStatus.textContent=savedTargetCollectionId?'Target playlist saved.':'No target playlist selected.';}
  catch(error){show(error);}
};
refreshTargetPlaylists.onclick=()=>void loadTargetPlaylists();
send({type:'status'}).then(r=>{
  enabled=r.enabled;
  savedTargetCollectionId=r.targetCollectionId||'';
  enable.textContent=enabled?'Disable browser-assisted access':'Enable browser-assisted access';
  const missingHost=/^specified native messaging host not found\.?$/i.test(r.error?.trim()||'');
  const message=r.error&&!missingHost?r.error:r.connected
    ? `Companion ready · Browser-assisted access ${enabled?'on':'off'}.`
    : 'Companion unavailable. Open Burrata and install the browser host.';
  status.textContent=[message,r.lastContextAction].filter(Boolean).join('\n');
}).catch(show);
importButton.onclick=async()=>{importButton.disabled=true;status.textContent='Collecting visible track metadata…';try{const r=await send({type:'importCurrent'});status.textContent=`Imported ${r.result.capturedCount} entries (${r.result.importedCount} unique recordings)${r.result.expectedTotal!=null?` of ${r.result.expectedTotal} expected`:""} · ${r.result.complete?"complete":"partial"}.\n${r.result.changes?`${r.result.changes.added} added · ${r.result.changes.removed} removed · ${r.result.changes.moved} moved\n`:""}${r.result.failedSources?.length?`${r.result.failedSources.length} album pages failed; retry from Burrata.\n`:""}${(r.result.warnings||[]).join('\n')}`;}catch(e){show(e);}finally{importButton.disabled=false;}};
enable.onclick=async()=>{
  try{
    const next=!enabled;
    if(next&&!await api.permissions.request({origins:['https://www.beatport.com/*','https://www.traxsource.com/*','https://open.spotify.com/*','https://www.youtube.com/*','https://music.youtube.com/*','https://francescograzioso.github.io/Meld/*','https://api.github.com/gists/*','https://www.shazam.com/*','https://shazam.com/*','https://www.last.fm/*','https://last.fm/*','https://bandcamp.com/*','https://*.bandcamp.com/*','https://soundcloud.com/*','https://www.soundcloud.com/*']}))throw new Error('Site access was declined. Importing the active page remains available.');
    await send({type:'enable',enabled:next});enabled=next;
    enable.textContent=enabled?'Disable browser-assisted access':'Enable browser-assisted access';status.textContent=enabled?'Browser-assisted access enabled.':'Browser-assisted access disabled.';
  }catch(e){show(e);}
};

function clearYouTubeAccountConfirmation(){confirmedYouTubeAccount=null;youtubeAccount.hidden=true;youtubeAccount.textContent='';youtubeAccountConsentLabel.hidden=true;youtubeAccountConsent.checked=false;youtubeHandoff.disabled=true;}
function validPairingCode(value){const match=/^([1-9]\d{0,4})-([a-f0-9]{32})$/.exec(value.trim());return !!match&&Number(match[1])<=65535;}
function updateYouTubeButtons(){youtubeCheckAccount.disabled=!youtubeConsent.checked||!validPairingCode(youtubePairingCode.value);youtubeHandoff.disabled=!confirmedYouTubeAccount||!youtubeAccountConsent.checked;}
updateYouTubeButtons();
youtubePairingCode.oninput=()=>{clearYouTubeAccountConfirmation();updateYouTubeButtons();};
youtubeConsent.onchange=()=>{clearYouTubeAccountConfirmation();updateYouTubeButtons();};
youtubeAccountConsent.onchange=updateYouTubeButtons;
youtubeCheckAccount.onclick=async()=>{
  clearYouTubeAccountConfirmation();updateYouTubeButtons();youtubeCheckAccount.disabled=true;
  status.textContent='Reading only the account name shown in the active YouTube tab…';
  try{
    const [tab]=await api.tabs.query({active:true,currentWindow:true});
    const origin=tab?.url?new URL(tab.url).origin:'';
    if(!['https://www.youtube.com','https://music.youtube.com'].includes(origin))throw new Error('Open a signed-in YouTube or YouTube Music tab, then retry.');
    if(!await api.permissions.contains({origins:[`${origin}/*`]})&&!await api.permissions.request({origins:[`${origin}/*`]}))throw new Error('YouTube site permission was declined. No session data was shared.');
    const r=await send({type:'youtubeCheckHandoffAccount',pairingCode:youtubePairingCode.value.trim()});
    if(!r.account?.accountId||!r.account?.displayName)throw new Error('YouTube did not show a verifiable selected channel account.');
    confirmedYouTubeAccount=r.account;youtubeAccount.textContent=`Selected YouTube account: ${r.account.displayName}`;youtubeAccount.hidden=false;youtubeAccountConsentLabel.hidden=false;
    status.textContent='Review the account name, then confirm it below before sending any session data.';
  }catch(error){show(error);}
  finally{updateYouTubeButtons();}
};
youtubeHandoff.onclick=async()=>{
  if(!confirmedYouTubeAccount||!youtubeConsent.checked||!youtubeAccountConsent.checked)return;
  youtubeHandoff.disabled=true;youtubeCheckAccount.disabled=true;
  status.textContent='Rechecking the confirmed account, then sending the session to the paired Burrata instance…';
  try{
    const r=await send({type:'youtubeSessionHandoff',pairingCode:youtubePairingCode.value.trim(),confirmedAccountId:confirmedYouTubeAccount.accountId});
    status.textContent=r.account?.displayName?`YouTube session connected for ${r.account.displayName}.`:'YouTube session connected.';
    clearYouTubeAccountConfirmation();youtubePairingCode.value='';youtubeConsent.checked=false;
  }catch(error){show(error);}
  finally{updateYouTubeButtons();}
};

const diagnosticButton=document.querySelector('#diagnostic');
diagnosticButton.onclick=async()=>{
  diagnosticButton.disabled=true;importButton.disabled=true;
  status.textContent='Preparing a sanitized public row diagnostic…';
  try{
    const {diagnostic}=await send({type:'diagnosticCurrent'});
    const provider=['beatport','traxsource','spotify','shazam','lastfm','bandcamp','soundcloud'].includes(diagnostic.provider)?diagnostic.provider:'provider';
    const filename=`burrata-${provider}-diagnostic.json`;
    const blob=new Blob([JSON.stringify(diagnostic,null,2)+'\n'],{type:'application/json'});
    const url=URL.createObjectURL(blob),anchor=document.createElement('a');
    anchor.href=url;anchor.download=filename;document.body.append(anchor);anchor.click();anchor.remove();
    setTimeout(()=>URL.revokeObjectURL(url),10000);
    status.textContent=`Saved ${filename}. Attach the locally saved file when reporting this parser failure. It contains sanitized public row data; cookies, JSON scripts and forms are excluded.`;
  }catch(error){show(error);}finally{diagnosticButton.disabled=false;importButton.disabled=false;}
};
