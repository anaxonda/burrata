// Read-only live acceptance monitoring. Acquisition stays through Burrata MCP.
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { writeFile } from 'node:fs/promises';

const durationIndex = process.argv.indexOf('--seconds');
const seconds = durationIndex < 0 ? 0 : Number(process.argv[durationIndex + 1]);
if (!Number.isFinite(seconds) || seconds < 0 || seconds > 3600) throw new Error('--seconds must be between 0 and 3600');
const reportPath = process.env.BURRATA_SOULSEEK_REPORT || '/tmp/burrata-soulseek-live-report.json';
const client = new Client({ name: 'burrata-soulseek-acceptance', version: '1.0.0' });
const transport = new StdioClientTransport({ command: process.execPath, args: [`${process.cwd()}/mcp/server.js`], cwd: process.cwd(), stderr: 'pipe' });
const report = { startedAt: new Date().toISOString(), snapshots: [] };
async function call(name, args = {}) {
  const result = await client.callTool({ name, arguments: args });
  if (result.isError) throw new Error(`${name}: ${JSON.stringify(result.structuredContent ?? result.content)}`);
  return result.structuredContent.result;
}
try {
  await client.connect(transport);
  const deadline = Date.now() + seconds * 1000;
  do {
    const all = [];
    let offset = 0;
    for (;;) {
      const page = await call('download_jobs', { limit: 100, offset });
      all.push(...page.items);
      if (!page.hasMore) break;
      offset += page.items.length;
      if (!page.items.length) throw new Error('Download pagination made no progress');
    }
    const jobs = all.filter(job => job.candidate.provider === 'slskd').map(job => ({
      id: job.id, title: job.track.title, peer: job.candidate.source?.username,
      state: job.state, progress: job.progress, error: job.error,
      batchId: job.candidate.source?.batch_id, transferId: job.candidate.source?.transfer_id,
      details: job.soulseek,
    }));
    report.snapshots.push({ at: new Date().toISOString(), jobs });
    console.log(JSON.stringify({ at: report.snapshots.at(-1).at, transferring: jobs.filter(job => job.state === 'downloading').map(job => ({ title: job.title, bytes: job.details?.bytesReceived, speed: job.details?.speedBytesPerSecond })), waiting: jobs.filter(job => job.state === 'waiting_for_peer').length, completed: jobs.filter(job => job.state === 'completed').length }));
    if (Date.now() >= deadline) break;
    await new Promise(resolve => setTimeout(resolve, Math.min(10000, deadline - Date.now())));
  } while (true);
  const playlist = await call('get_playlist', { playlistId: '81bc23f1-29af-44e6-ab20-f087a3f5788d', limit: 100 });
  report.playlist = { total: playlist.total, local: playlist.tracks.filter(track => track.localAvailability === 'available').map(track => ({ id: track.id, title: track.title, bpm: track.bpm, key: track.key })) };
  report.finishedAt = new Date().toISOString();
  await writeFile(reportPath, JSON.stringify(report, null, 2));
  console.log(JSON.stringify({ report: reportPath, playlist: report.playlist }));
} finally {
  await client.close();
}
