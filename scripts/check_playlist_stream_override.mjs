// Live acceptance: run with the desktop's override option enabled, then disabled.
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { writeFile } from 'node:fs/promises';
const client = new Client({ name:'playlist-stream-override-test',version:'1.0.0' });
const transport = new StdioClientTransport({command:process.execPath,args:[`${process.cwd()}/mcp/server.js`],cwd:process.cwd(),stderr:'pipe'});
const report = { enabled:process.argv.includes('--enabled'), snapshots:[] };
async function call(name,args={}) { const r=await client.callTool({name,arguments:args}); if(r.isError)throw Error(JSON.stringify(r.structuredContent));return r.structuredContent.result; }
try {
 await client.connect(transport);
 const trackId='import:53d7198a1fefb838731241fdd2d9cf36813c819c7ddaadd7e5b61de53e235884';
 if(report.enabled) {
  const found=await call('search_audio_sources',{trackId,providers:['monochrome']});
  report.matches=found.items;
  if(!found.items.some(c=>c.confidence==='high'&&c.id==='monochrome:174208180224667648'))throw Error('Expected exact recording did not match');
 }
 await call('play_tracks',{trackIds:[trackId]});
 for(let i=0;i<4;i++) { await new Promise(r=>setTimeout(r,5000));const p=await call('playback_state');report.snapshots.push(p);console.log(JSON.stringify({enabled:report.enabled,kind:p.previewKind,status:p.status,position:p.position,message:p.playerMessage,error:p.mediaError})); }
 const first=report.snapshots[0],last=report.snapshots.at(-1);
 const expected=report.enabled?'monochrome':'youtube';
 if(last.status!=='playing'||last.previewKind!=='audio'||last.mediaError||last.position<first.position+8||!last.playerMessage?.toLowerCase().includes(expected))throw Error('Wrong source or playback did not advance');
 await call('pause_playback');
} catch(error) {report.error=String(error);console.error(report.error);process.exitCode=1;}
finally {await writeFile(`/tmp/burrata-playlist-override-${report.enabled?'on':'off'}.json`,JSON.stringify(report,null,2));await client.close();}
