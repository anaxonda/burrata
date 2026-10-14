// Fault-injection acceptance against the live desktop. Configure the requested mode first.
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { writeFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { homedir } from 'node:os';
import { join } from 'node:path';
const mode=process.argv[2];if(!['disabled','lossless','lossy'].includes(mode))throw Error('Choose disabled, lossless, or lossy');
const client=new Client({name:'burrata-download-fallback-acceptance',version:'1.0.0'});
const transport=new StdioClientTransport({command:process.execPath,args:[`${process.cwd()}/mcp/server.js`],cwd:process.cwd(),stderr:'pipe'});
const report={mode,startedAt:new Date().toISOString(),snapshots:[]};
async function call(name,args={}){const r=await client.callTool({name,arguments:args});if(r.isError)throw Error(JSON.stringify(r.structuredContent??r.content));return r.structuredContent.result;}
try{
 await client.connect(transport);
 const candidate={id:'monochrome:fallback-fault-injection',provider:'monochrome',url:'https://tracks.monochrome.st/track/999999999999999999',title:'Sui Generis - Quizás, Porque',uploader:'Sui Generis',durationSeconds:132.973,artworkUrl:null,confidence:'high',score:0.97,permission:'unknown',official:false,reasons:['Deliberately unavailable source for fallback acceptance testing'],formats:[{id:'flac',codec:'flac',extension:'flac',lossless:true,sampleRate:44100,bitrate:null}]};
 const started=await call('download_candidate',{trackId:'import:53d7198a1fefb838731241fdd2d9cf36813c819c7ddaadd7e5b61de53e235884',candidate,outputProfile:'source',idempotencyKey:`source-fallback-acceptance-${mode}-${Date.now()}`});
 report.jobId=started.job.id;
 console.log(JSON.stringify({mode,jobId:report.jobId}));
 const deadline=Date.now()+240000;
 for(;;){
  const page=await call('download_jobs',{limit:100});const job=page.items.find(j=>j.id===report.jobId);if(!job)throw Error('Test job missing');
  report.snapshots.push({at:new Date().toISOString(),state:job.state,provider:job.candidate.provider,progress:job.progress,error:job.error});
  console.log(JSON.stringify(report.snapshots.at(-1)));
  if(['completed','failed','cancelled'].includes(job.state)){report.finalJob=job;break;}
  if(Date.now()>deadline)throw Error('Fallback test timed out');await new Promise(r=>setTimeout(r,5000));
 }
 const job=report.finalJob;
 if(mode==='lossy'){
  if(job.state!=='completed'||job.candidate.provider!=='youtube')throw Error('Expected successful YouTube fallback');
  report.finalTrack=await call('get_track',{trackId:job.track.id});
  // MCP intentionally omits local paths. Read only this job's published path for ffprobe.
  const database=join(process.env.XDG_DATA_HOME||join(homedir(),'.local','share'),'org.burrata.discovery','burrata.sqlite');
  const path=execFileSync('python3',['-c',"import sqlite3,json,sys; c=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True); print(json.loads(c.execute('SELECT payload FROM download_jobs WHERE id=?',(sys.argv[2],)).fetchone()[0])['outputPath'])",database,job.id],{encoding:'utf8'}).trim();
  report.outputPath=path;
  report.ffprobe=JSON.parse(execFileSync('ffprobe',['-v','error','-show_entries','stream=codec_name,sample_rate:format=duration,size','-of','json',path],{encoding:'utf8'}));
 }else{
  if(job.state!=='failed'||job.candidate.provider!=='monochrome')throw Error('Unexpected fallback or download when disabled/quality protected');
  if(mode==='disabled'&&job.track.provenance.downloadFallbackAttempts)throw Error('Disabled fallback tried another provider');
  if(mode==='lossless'&&!job.track.provenance.downloadFallbackAttempts?.some(a=>a.provider==='youtube'&&a.error?.includes('quality')))throw Error('Lossless guard was not recorded');
 }
}catch(error){report.error=String(error);console.error(report.error);process.exitCode=1;}
finally{report.finishedAt=new Date().toISOString();await writeFile(`/tmp/burrata-download-fallback-${mode}.json`,JSON.stringify(report,null,2));await client.close();}
