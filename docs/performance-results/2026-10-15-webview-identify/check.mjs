import {Client} from '@modelcontextprotocol/client';
import {StdioClientTransport} from '@modelcontextprotocol/client/stdio';
import {writeFile} from 'node:fs/promises';
const [automationFile,outputFile]=process.argv.slice(2);
if(!automationFile||!outputFile)process.exit(2);
const c=new Client({name:'burrata-webview-identification-check',version:'1'});
const t=new StdioClientTransport({command:'bash',args:['scripts/dev/mcp-stdio.sh'],cwd:process.cwd(),env:{...process.env,BURRATA_AUTOMATION_FILE:automationFile},stderr:'pipe'});
t.stderr?.on('data',()=>{});
const trackId='spotify:0Kvqc2zK0UKPTK6pAgyv1v';
const out={binarySha256:'ced6cc73fd2c5b7205ba6650e10edab1d95f0113487da7b73dd2e450b5be6394',engine:'webview',muted:true,physicalAudibilityMeasured:false,playbackVerified:false,identificationAttempted:false};
async function call(name,args={},timeout=25000){const r=await c.callTool({name,arguments:args},{timeout});if(r.isError){const e=new Error('tool_error');const message=r.structuredContent?.error?.message??r.content?.[0]?.text??'';e.category=/SongRec|songrec/.test(message)?'recognition_helper_unavailable':/extract an audio excerpt/.test(message)?'excerpt_extraction_failed':/cancel|changed/i.test(message)?'playback_changed':/source is unavailable/i.test(message)?'source_unavailable':'tool_error';throw e;}return r.structuredContent?.result??r.structuredContent;}
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
let before;
try{
 await c.connect(t);before=await call('playback_state',{limit:1});
 await call('set_volume',{volume:0});await call('play_tracks',{trackIds:[trackId]});await call('seek_playback',{position:30});await call('resume_playback');
 let baseline=null;const end=Date.now()+20000;
 while(Date.now()<end){const s=await call('playback_state',{limit:1},5000);const id=s.trackId??s.track?.id;if(id===trackId&&s.status==='playing'&&Math.abs(s.duration-250.078912)<1){if(baseline!==null&&s.position>baseline+0.3){out.playbackVerified=true;out.durationSeconds=s.duration;out.sourceGenerationPresent=Number.isSafeInteger(s.sourceGeneration);break;}baseline??=s.position;}await sleep(300);}
 if(!out.playbackVerified)throw Object.assign(new Error('playback_not_verified'),{category:'playback_not_verified'});
 out.identificationAttempted=true;const started=Date.now();const result=await call('identify_playing_track',{},65000);out.identificationMs=Date.now()-started;out.identificationReturned=true;out.resultPresent=typeof result?.title==='string'&&result.title.length>0;out.artistPresent=typeof result?.artist==='string'&&result.artist.length>0;out.resultTitleMatchesExpected=out.resultPresent&&/poso|πόσο/i.test(result.title);out.tagsAutomaticallyEdited=false;
}catch(e){out.failureCategory=e.category??'mcp_error';}
finally{await call('pause_playback',{},5000).catch(()=>{});if(typeof before?.volume==='number')await call('set_volume',{volume:before.volume},5000).catch(()=>{});await c.close().catch(()=>{});await writeFile(outputFile,JSON.stringify(out,null,2)+'\n');console.log(JSON.stringify(out));}
