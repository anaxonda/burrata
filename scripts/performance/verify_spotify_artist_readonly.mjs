// Read-only live qualification using an explicitly retained isolated session.
import {Client} from '@modelcontextprotocol/client';
import {StdioClientTransport} from '@modelcontextprotocol/client/stdio';
import {mkdir,writeFile} from 'node:fs/promises';
const [automationFile,outputDirectory]=process.argv.slice(2);
if(!automationFile?.startsWith('/tmp/burrata-launch.2bdf29d9ddb3/')||!outputDirectory)throw Error('Retained isolated profile and report directory required');
const client=new Client({name:'spotify-artist-live-read',version:'1.0.0'});
const transport=new StdioClientTransport({command:'bash',args:['scripts/dev/mcp-stdio.sh'],cwd:process.cwd(),env:{...process.env,BURRATA_AUTOMATION_FILE:automationFile},stderr:'pipe'});
transport.stderr?.on('data',()=>{});
const report={scope:'Read-only public artist/release/album pages using the retained isolated confirmed session; no confirmation, import, credentials or remote writes.',checks:[]};
async function read(name,args,label){
 const start=performance.now();const response=await client.callTool({name,arguments:args},{timeout:65000});
 const data=response.structuredContent?.result??response.structuredContent;
 if(response.isError){const raw=JSON.stringify(response);report.checks.push({label,ok:false,elapsedMs:Math.round(performance.now()-start),apiDenied:/API_DENIED|does not permit|denied|catalog API/i.test(raw),authenticationUnavailable:/SIGN_IN_REQUIRED|authentication is unavailable/i.test(raw),rateLimited:/RATE_LIMITED|rate limit/i.test(raw),requestTimedOut:/REQUEST_TIMEOUT/i.test(raw),phase:/phase=([a-z_]{1,40})/.exec(raw)?.[1]??null,httpStatus:Number(/HTTP (\d{3})/.exec(raw)?.[1])||null,retryAfterSeconds:Number(/retry-after=(\d{1,4})s/.exec(raw)?.[1]??/Try again in (\d{1,4}) seconds/.exec(raw)?.[1])||null});return null;}
 const rows=data?.albums??data?.tracks??[];
 report.checks.push({label,ok:true,elapsedMs:Math.round(performance.now()-start),rowCount:rows.length,distinctRows:new Set(rows.map(row=>row.id)).size,hasNextCursor:!!data?.nextCursor,total:data?.totalAlbums??data?.total??null,hasArtwork:!!(data?.artist?.artworkUrl??data?.album?.artworkUrl)});
 return data;
}
try{
 await client.connect(transport);
 const first=await read('get_remote_artist',{artistId:'4tZwfgrHOc3mvqYlEYSvVi'},'artist_first_page');
 if(first?.nextCursor){const second=await read('get_remote_artist',{artistId:'4tZwfgrHOc3mvqYlEYSvVi',cursor:first.nextCursor},'artist_second_page');if(second){const known=new Set(first.albums.map(row=>row.id));report.checks.at(-1).overlapWithFirstPage=second.albums.filter(row=>known.has(row.id)).length;report.checks.at(-1).repeatedCursor=!!second.nextCursor&&second.nextCursor===first.nextCursor;}}
 const albumId=first?.albums?.[0]?.id;
 if(albumId){const album=await read('get_remote_album_tracks',{albumId},'album_first_page');if(album?.nextCursor)await read('get_remote_album_tracks',{albumId,cursor:album.nextCursor},'album_second_page');}
 if(!report.checks.length||report.checks.some(check=>!check.ok))process.exitCode=1;
}catch{report.checks.push({label:'harness',ok:false,reason:'App-specific read verification did not finish; raw output excluded.'});process.exitCode=1;}
finally{await client.close().catch(()=>{});await mkdir(outputDirectory,{recursive:true});await writeFile(`${outputDirectory}/report.json`,JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));}
