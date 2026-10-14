import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { writeFile } from 'node:fs/promises';
const client = new Client({ name: 'burrata-monochrome-acceptance', version: '1.0.0' });
const transport = new StdioClientTransport({ command: process.execPath, args: [`${process.cwd()}/mcp/server.js`], cwd: process.cwd(), stderr: 'pipe' });
const getLucky = process.argv.includes('--get-lucky');
const recordingId = getLucky ? '155142501534011392' : '155102871061270528';
const report = { startedAt: new Date().toISOString(), playback: [] };
async function call(name, args = {}) {
 const result = await client.callTool({ name, arguments: args });
 if (result.isError) throw new Error(`${name}: ${JSON.stringify(result.structuredContent ?? result.content)}`);
 return result.structuredContent.result;
}
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
try {
 await client.connect(transport);
 const found = await call('search_tracks', { query: getLucky ? 'Daft Punk Get Lucky' : 'Daft Punk One More Time', providers: ['monochrome'] });
 const track = found.items.find(t => t.id === `monochrome:${recordingId}`);
 if (!track) throw new Error('Exact test recording missing');
 report.track = track;
 if (track.assetId || track.localAvailability === 'available' || track.localPath) throw new Error('Streaming test requires a track without a local asset');
 const before = await call('playback_state');
 report.before = before;
 const candidates = await call('search_audio_sources', { trackId: track.id, providers: ['monochrome'] });
 const selected = candidates.items.find(c => c.id === track.id);
 if (!selected) throw new Error('Linked candidate missing');
 report.inspected = await call('inspect_audio_source', { trackId: track.id, candidate: selected });
 await call('play_tracks', { trackIds: [track.id] });
 for (let i = 0; i < 6; i++) {
  await sleep(5000);
  const state = await call('playback_state');
  report.playback.push(state);
  console.log(JSON.stringify({ phase: 'stream', status: state.status, kind: state.previewKind, position: state.position, error: state.mediaError, message: state.playerMessage }));
 }
 const first = report.playback.find(p => p.status === 'playing' && p.previewKind === 'audio');
 const last = report.playback.at(-1);
 if (!first || last.position < first.position + 10 || last.mediaError) throw new Error('Remote playback did not advance');
 await call('seek_playback', { position: 120 });
 await sleep(5000);
 report.afterSeek = await call('playback_state');
 console.log(JSON.stringify({ phase: 'seek', position: report.afterSeek.position, status: report.afterSeek.status }));
 if (report.afterSeek.position < 120 || report.afterSeek.position > 135) throw new Error('Seek failed');
 await call('pause_playback');
 if (process.argv.includes('--download')) {
  report.download = await call('download_candidate', { trackId: track.id, candidate: selected, outputProfile: 'source', idempotencyKey: 'monochrome-one-more-time-flac-acceptance-v1' });
  console.log(JSON.stringify({ phase: 'download-start', jobId: report.download.job?.id }));
  const deadline = Date.now() + 180000;
  for (;;) {
   const page = await call('download_jobs', { limit: 100 });
   const job = page.items.find(j => j.track.id === track.id && j.candidate.provider === 'monochrome');
   report.finalJob = job;
   console.log(JSON.stringify({ phase: 'download', state: job?.state, progress: job?.progress, error: job?.error, assetId: job?.localAssetId }));
   if (job?.state === 'completed') break;
   if (['failed','cancelled'].includes(job?.state) || Date.now() > deadline) throw new Error('Download did not complete');
   await sleep(5000);
  }
  report.finalTrack = await call('get_track', { trackId: track.id });
 }
} catch (error) { report.error = String(error); console.error(report.error); process.exitCode = 1; }
finally {
 report.finishedAt = new Date().toISOString();
 await writeFile(getLucky ? '/tmp/burrata-monochrome-get-lucky-stream-report.json' : '/tmp/burrata-monochrome-live-report.json', JSON.stringify(report, null, 2));
 await client.close();
}
