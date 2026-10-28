// Finite runtime checks against the disposable profile built by
// src-tauri/examples/build_qualification_profile.rs. Never target a user profile.
import { readFile, realpath, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { readDiscovery, apiRequest } from '../mcp/server.js';

const [profileArgument, name, argumentJson = '{}', output] = process.argv.slice(2);
if (!profileArgument || !name) throw new Error('Usage: first_four_runtime.mjs PROFILE TOOL JSON [PNG_OUTPUT]');
const profile = await realpath(profileArgument);
if (!profile.startsWith('/tmp/burrata-first-four-profile-')) throw new Error('Only disposable first-four profiles under /tmp are permitted');
if ((await readFile(join(profile, '.burrata-qualification-profile'), 'utf8')).trim() !== 'synthetic fixture only') throw new Error('Missing synthetic fixture marker');
const inventory = JSON.parse(await readFile(join(profile, 'qualification-inventory.json'), 'utf8'));
if (inventory.syntheticOnly !== true || inventory.externalAudioUsed !== false || resolve(inventory.appDataDirectory) !== join(profile, 'data')) throw new Error('Invalid synthetic profile inventory');
const allowed = new Set(['get_app_status', 'library_stats', 'list_playlists', 'get_playlist', 'playback_state', 'navigate_ui', 'capture_main_ui', 'resize_app_window', 'inspect_playback_layout', 'inspect_webview_media', 'set_volume', 'play_tracks', 'pause_playback', 'resume_playback', 'seek_playback', 'set_player_panel', 'get_live_bpm']);
if (!allowed.has(name)) throw new Error('Tool is outside the finite runtime qualification allowlist');
const args = JSON.parse(argumentJson);
if (name === 'set_volume' && args.volume !== 0) throw new Error('Synthetic playback must remain silent');
if (name === 'navigate_ui' && !['library', 'favorites', 'history', 'downloads', 'settings'].includes(args.view) && args.collectionId !== inventory.playlist.id) throw new Error('Navigation is limited to local qualification views');
if (name === 'resize_app_window' && (!Number.isFinite(args.width) || !Number.isFinite(args.height) || args.width < 360 || args.width > 1920 || args.height < 240 || args.height > 1080)) throw new Error('Invalid test viewport');
if (name === 'play_tracks' && (!Array.isArray(args.trackIds) || args.trackIds.some(id => !inventory.tracks.some(track => track.id === id)))) throw new Error('Playback is limited to generated synthetic tracks');
const discovery = await readDiscovery(join(profile, 'data', 'automation.json'));
const { response, payload } = await apiRequest(discovery, '/automation/call', { method: 'POST', body: JSON.stringify({ name, arguments: args }) }, { toolName: name });
if (!response.ok || payload?.isError) throw new Error(`Qualification tool failed: ${name} (${response.status})`);
const result = payload?.structuredContent?.result ?? payload?.structuredContent;
if (name === 'capture_main_ui') {
  const png = payload?.content?.find(block => block.type === 'image' && block.mimeType === 'image/png');
  if (!output || result?.mimeType !== 'image/png' || typeof png?.data !== 'string') throw new Error('Expected bounded app-only PNG');
  const path = resolve(output);
  if (!path.startsWith(profile + '/') && !path.startsWith(resolve('.cache') + '/')) throw new Error('Screenshot output must stay in the disposable profile or repository cache');
  await writeFile(path, Buffer.from(png.data, 'base64'));
  console.log(JSON.stringify({ path, width: result.width, height: result.height }));
} else {
  console.log(JSON.stringify(result ?? payload));
}
