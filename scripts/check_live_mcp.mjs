import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
const client = new Client({name:'burrata-live-acceptance',version:'1.0.0'});
const transport = new StdioClientTransport({command:process.execPath,args:[`${process.cwd()}/mcp/server.js`],cwd:process.cwd(),stderr:'pipe'});
const report = {checks:[]};
async function call(name,args={}) {
  const response=await client.callTool({name,arguments:args});
  assert(!response.isError, `${name}: ${JSON.stringify(response.structuredContent ?? response.content)}`);
  return response.structuredContent?.result;
}
try {
  await client.connect(transport);
  const registry=await client.listTools(); report.toolCount=registry.tools.length;
  assert(registry.tools.some(tool=>tool.name==='play_tracks'));
  const status=await call('get_app_status'); report.permissions=status.permissions; report.initialPlayback=status.playback;
  const listing=await call('list_playlists',{limit:100});
  report.playlists=listing.items.map(item=>({id:item.playlist.id,name:item.playlist.name,count:item.playlist.trackCount,provider:item.sourceProvider}));
  for (const [name,count] of [['favorites',3],['monochrome',37],['omara-style',14],['ytsurf-history',25]]) {
    const item=listing.items.find(item=>item.playlist.name.toLowerCase()===name);
    assert(item,`Missing discovered playlist ${name}`);
    const contents=await call('get_playlist',{playlistId:item.playlist.id,limit:100});
    assert.equal(contents.total,count);
    assert(contents.tracks.every(track=>track.provenance.audioCandidate?.provider==='youtube'),`Missing exact source candidate in ${name}`);
    report.checks.push(`${name}: ${count} exact YouTube entries`);
  }
  const chart=listing.items.find(item=>item.sourceProvider==='beatport');
  assert(chart,'No Beatport import');
  const top=await call('get_playlist',{playlistId:chart.playlist.id,limit:50});
  assert.equal(top.tracks.length,50); report.checks.push('Latest Beatport chart returns top 50');
  const known=[];
  for (const query of ['Ha Ya','Mi Gente','Let the Drums Talk','Pump up the Jam']) {
    const page=await call('search_library',{query,limit:100});
    const track=page.items.find(track=>track.localAvailability==='available');
    assert(track,`Missing downloaded recording ${query}`); known.push(track);
  }
  const created=await call('create_playlist',{name:'Local downloads',idempotencyKey:'burrata-live-local-downloads-v1'});
  const duplicate=await call('create_playlist',{name:'Local downloads',idempotencyKey:'burrata-live-local-downloads-v1'});
  assert.equal(created.playlistId,duplicate.playlistId);
  await call('add_tracks_to_playlist',{playlistId:created.playlistId,trackIds:known.map(track=>track.id),idempotencyKey:'burrata-live-add-downloads-v1'});
  const playlist=await call('get_playlist',{playlistId:created.playlistId,limit:100});
  assert.equal(playlist.total,4); report.createdPlaylistId=created.playlistId; report.checks.push('Idempotent playlist creation and batch membership');
  const denied=await client.callTool({name:'download_candidate',arguments:{trackId:known[0].id,candidate:{}}});
  assert.equal(denied.isError,true); assert.equal(denied.structuredContent.error.code,'PERMISSION_DENIED');
  report.checks.push('Downloads denied until Manage enabled');
  const originalVolume=Number(status.playback?.volume ?? 0.8);
  await call('set_volume',{volume:0.05});
  try {
    const started=await call('play_tracks',{trackIds:known.map(track=>track.id)}); assert.equal(started.trackId,known[0].id); assert.equal(started.previewKind,'local','Downloaded recording unexpectedly fell back to online audio');
    await new Promise(resolve=>setTimeout(resolve,1600));
    const playing=await call('playback_state'); assert.equal(playing.paused,false); assert(playing.position>0);
    const paused=await call('pause_playback'); assert.equal(paused.paused,true);
    await call('queue_tracks',{trackIds:[known[1].id],mode:'next'});
    const queued=await call('playback_state'); assert.equal(queued.queue[queued.currentIndex+1].id,known[1].id);
    report.checks.push('Direct local desktop audio advances, pauses, and queues next'); report.playback={trackId:playing.trackId,position:playing.position,duration:playing.duration};
  } finally { await call('pause_playback'); await call('set_volume',{volume:originalVolume}); }
  await writeFile('/tmp/burrata-mcp-live-report.json',JSON.stringify(report,null,2));
  console.log(JSON.stringify({toolCount:report.toolCount,checks:report.checks,createdPlaylistId:report.createdPlaylistId,playback:report.playback},null,2));
} finally { await client.close(); }
