// Capture only the selected Burrata instance's main WebView; never the desktop.
import { Client } from '@modelcontextprotocol/client';
import { StdioClientTransport } from '@modelcontextprotocol/client/stdio';
import { writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
const [automationFile, outputFile, ...extra] = process.argv.slice(2);
if (!automationFile || !outputFile || extra.length) {
  console.error('usage: capture-main-ui.mjs AUTOMATION_FILE OUTPUT_PNG');
  process.exit(2);
}
const root = fileURLToPath(new URL('../../', import.meta.url));
const client = new Client({ name: 'burrata-main-ui-capture', version: '1.0.0' });
const transport = new StdioClientTransport({ command: 'bash', args: [`${root}scripts/dev/mcp-stdio.sh`], cwd: root,
  env: { ...process.env, BURRATA_AUTOMATION_FILE: resolve(automationFile) }, stderr: 'pipe' });
transport.stderr?.on('data', () => {});
try {
  await client.connect(transport);
  const response = await client.callTool({ name: 'capture_main_ui', arguments: {} }, { timeout: 10_000 });
  const image = response.structuredContent?.result ?? response.structuredContent;
  const data = response.content?.find(block => block.type === 'image' && block.mimeType === 'image/png')?.data;
  if (response.isError || image?.mimeType !== 'image/png' ||
      !Number.isInteger(image.width) || image.width < 1 || image.width > 3840 ||
      !Number.isInteger(image.height) || image.height < 1 || image.height > 2160 ||
      typeof data !== 'string' || data.length > 11_184_812 ||
      data.length % 4 !== 0 || !/^[A-Za-z0-9+/]*={0,2}$/.test(data)) {
    throw new Error('capture_unavailable');
  }
  const png = Buffer.from(data, 'base64');
  if (png.length < 33 || png.length > 8 * 1024 * 1024 ||
      !png.subarray(0, 8).equals(Buffer.from([137,80,78,71,13,10,26,10])) ||
      png.readUInt32BE(8) !== 13 || png.toString('ascii', 12, 16) !== 'IHDR' ||
      png.readUInt32BE(16) !== image.width || png.readUInt32BE(20) !== image.height) {
    throw new Error('invalid_capture');
  }
  await writeFile(outputFile, png, { flag: 'wx', mode: 0o600 });
  console.log(JSON.stringify({ width: image.width, height: image.height, bytes: png.length }));
} catch {
  console.error('Main UI capture failed or output already exists. No desktop fallback was used.');
  process.exitCode = 1;
} finally {
  await client.close().catch(() => {});
}
