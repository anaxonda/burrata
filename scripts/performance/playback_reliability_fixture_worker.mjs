// Local bytes only. A warm helper still launches a fresh FFmpeg decoder.
import { createInterface } from 'node:readline';
import { decodePcm } from './innertube/core.mjs';
import { commandBudget } from './innertube/budget.mjs';
for await (const line of createInterface({ input: process.stdin })) {
  let budget;
  try {
    const request = JSON.parse(line);
    if (request.fixture !== process.env.BURRATA_RELIABILITY_FIXTURE) throw new Error();
    budget = commandBudget(request.budgetMs);
    const before = process.cpuUsage();
    const result = await budget.run(() => decodePcm(request.fixture, { position: request.position ?? 0, seconds: 5, signal: budget.signal }));
    const cpu = process.cpuUsage(before);
    const helperCpuMs = (cpu.user + cpu.system) / 1000;
    process.stdout.write(JSON.stringify({ ok: true, ...result, helperCpuMs,
      helperRssBytes: process.memoryUsage().rss, helperMaxRssBytes: process.resourceUsage().maxRSS * 1024,
      totalCpuMs: helperCpuMs + result.decoderCpuMs + result.accountingWrapperCpuMs,
      stages: { decode: result.decodeWallMs }, decoderReused: false, accountingComplete: true }) + '\n');
  } catch (error) {
    const allowed = ['TIMEOUT', 'CANCELLED', 'DECODE_FAILED', 'NO_PCM', 'SHORT_AUDIO', 'DECODER_UNAVAILABLE'];
    process.stdout.write(JSON.stringify({ ok: false, failure: allowed.includes(error.code) ? error.code : 'INVALID_REQUEST', stage: 'decode', accountingComplete: false }) + '\n');
  } finally { budget?.close(); }
}
