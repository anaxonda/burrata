import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const source = path.join(root, 'extension');
const target = path.join(root, 'src-tauri', 'resources', 'provider-extension');
await mkdir(target, { recursive: true });
const manifest = JSON.parse(await readFile(path.join(source, 'manifest.chromium.json'), 'utf8'));
if (!manifest.key || manifest.manifest_version !== 3) {
  throw new Error('Managed provider extension requires its stable MV3 Chromium manifest');
}
const managedSites = ['https://open.spotify.com/*', 'https://www.youtube.com/*', 'https://music.youtube.com/*'];
manifest.host_permissions = [...new Set([...(manifest.host_permissions || []), ...managedSites])];
manifest.optional_host_permissions = (manifest.optional_host_permissions || []).filter((host) => !managedSites.includes(host));
await copyFile(path.join(source, 'manifest.chromium.json'), path.join(target, 'manifest.chromium.json'));
await writeFile(path.join(target, 'manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
for (const name of ['background.js', 'content.js', 'parsers.js', 'popup.html', 'popup.js', 'spotify-auth.js', 'provider-operations.js']) {
  await copyFile(path.join(source, name), path.join(target, name));
}
const nativeTarget = path.join(root, 'src-tauri', 'resources', 'native-provider-session');
await mkdir(path.join(nativeTarget, 'scripts'), { recursive: true });
await mkdir(path.join(nativeTarget, 'extension'), { recursive: true });
await copyFile(path.join(root, 'scripts', 'provider-session-worker.mjs'), path.join(nativeTarget, 'scripts', 'provider-session-worker.mjs'));
await copyFile(path.join(source, 'spotify-auth.js'), path.join(nativeTarget, 'extension', 'spotify-auth.js'));
await copyFile(path.join(source, 'provider-operations.js'), path.join(nativeTarget, 'extension', 'provider-operations.js'));
process.stdout.write(`Staged managed provider extension at ${target}\n`);
