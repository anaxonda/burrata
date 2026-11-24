export type SettingsCategory = "general" | "playback" | "library" | "downloads" | "connections" | "integrations" | "advanced";

export const settingsCategories = [
  { id: "general", label: "General" },
  { id: "playback", label: "Playback" },
  { id: "library", label: "Library" },
  { id: "downloads", label: "Downloads" },
  { id: "connections", label: "Connections" },
  { id: "advanced", label: "Troubleshooting" },
] as const;

type SearchCategory = typeof settingsCategories[number]["id"];

type Setting = {
  category: SearchCategory;
  label: string;
  target: string;
  keywords?: string;
  desktopOnly?: boolean;
};

const settings: Setting[] = [
  { category: "general", label: "Theme", target: "appearance-theme", keywords: "appearance dark light mode colors" },
  { category: "general", label: "Show Spotify plays and YouTube views", target: "provider-counts-setting", keywords: "appearance provider counts popularity" },
  { category: "general", label: "Compact playback bar", target: "compact-playback-setting", keywords: "appearance mini player layout size" },
  { category: "general", label: "Search suggestions", target: "search-suggestions-setting", keywords: "artist autocomplete typeahead completion" },
  { category: "playback", label: "Even out volume", target: "loudness-title", keywords: "loudness normalization normalize gain volume level ReplayGain analysis" },
  { category: "playback", label: "Crossfade songs", target: "crossfade-settings", keywords: "fade duration blend transition gapless" },
  { category: "playback", label: "Soft Next", target: "soft-next-setting", keywords: "fade skip next track" },
  { category: "playback", label: "Fade when selecting a track", target: "soft-track-selection-setting", keywords: "soft track selection crossfade" },
  { category: "playback", label: "Play songs selected with arrow keys", target: "auto-preview-setting", keywords: "auto preview keyboard selection" },
  { category: "playback", label: "Play radio stations on click", target: "radio-on-click-setting", keywords: "autoplay" },
  { category: "playback", label: "Opening a song’s source", target: "open-song-source-settings", keywords: "pause source current position timestamp youtube soundcloud" },
  { category: "playback", label: "Recording matching", target: "recording-matching", keywords: "match identity exact balanced flexible remix live cover version" },
  { category: "playback", label: "Primary streaming source", target: "preferred-streaming-source", keywords: "audio provider playback youtube soundcloud monochrome" },
  { category: "playback", label: "Streaming fallback", target: "streaming-fallback-settings", keywords: "audio source priority order alternatives bandcamp youtube soundcloud monochrome" },
  { category: "playback", label: "Prepare saved radio songs", target: "radio-source-preparation-setting", keywords: "cache background playback" },
  { category: "playback", label: "Search for matches before trying playlist links", target: "playlist-source-preference-setting", keywords: "override playlist track urls audio source" },
  { category: "playback", label: "Include radio songs in playback history", target: "radio-history-setting", keywords: "listening history save" },
  { category: "playback", label: "Audio analysis", target: "audio-analysis-settings", keywords: "live bpm tempo musical key waveform spectrum" },
  { category: "playback", label: "Playback startup timeout", target: "playback-startup-timeout", keywords: "advanced playback startup time delay seconds slow start" },
  { category: "playback", label: "Preview delay", target: "preview-delay-setting", keywords: "auto preview ms delay arrow keys" },
  { category: "playback", label: "Wait for preferred source", target: "streaming-fallback-delay", keywords: "streaming fallback delay advanced playback latency source priority milliseconds" },
  { category: "playback", label: "Radio recognition", target: "radio-setup-settings", keywords: "radio identify track song shazam" },
  { category: "library", label: "Playlists in the sidebar", target: "account-playlist-settings-title", keywords: "account spotify youtube visibility show hide" },
  { category: "library", label: "Show catalog provider filter", target: "library-catalog-filter-setting", keywords: "artists albums catalog source spotify musicbrainz discogs" },
  { category: "library", label: "Favorites playlist", target: "playlist-settings-title", keywords: "liked songs playlist default shortcuts exports" },
  { category: "library", label: "Keep local M3U playlist copies", target: "playlist-mirroring-setting", keywords: "m3u8 mirror sync local files playlist downloads export" },
  { category: "library", label: "DJ XML options", target: "dj-xml-settings", keywords: "itunes rekordbox music folder exports" },
  { category: "library", label: "Listening history mirror", target: "history-mirror-settings-title", keywords: "listening history mirror playlist export copies", desktopOnly: true },
  { category: "library", label: "Music folder", target: "music-directory-setting", keywords: "download destination path location directory" },
  { category: "library", label: "Playlist folders", target: "playlist-folders-title", keywords: "local files m3u path directory" },
  { category: "library", label: "Backup & restore", target: "backup-settings", keywords: "automatic schedule archive import export reset factory settings recovery", desktopOnly: true },
  { category: "downloads", label: "Embed lyrics in downloads", target: "embed-lyrics-setting", keywords: "offline files tags" },
  { category: "downloads", label: "Download folder layout", target: "download-layout-setting", keywords: "flat artist album playlist organize files" },
  { category: "downloads", label: "Primary download source", target: "preferred-download-source", keywords: "audio provider youtube soundcloud soulseek monochrome" },
  { category: "downloads", label: "Download fallback", target: "download-fallback-settings", keywords: "preferred source order alternatives" },
  { category: "downloads", label: "Allow a lossy fallback", target: "allow-lossy-setting", keywords: "lossless downloads format quality" },
  { category: "downloads", label: "Soulseek download policy", target: "soulseek-policy-settings", keywords: "wait unavailable upgrade lossless" },
  { category: "downloads", label: "Automatic tagging", target: "autotag-title", keywords: "metadata fingerprint acoustid shazam" },
  { category: "connections", label: "Streaming playlist accounts", target: "remote-account-title", keywords: "spotify youtube sign in login playlist sync write access" },
  { category: "connections", label: "AI summaries & translation", target: "ai-connection", keywords: "api key endpoint model openai openrouter" },
  { category: "connections", label: "Lyrics translation", target: "lyrics-translation-connection", keywords: "language translate google ai" },
  { category: "connections", label: "Soulseek / slskd", target: "soulseek-connection", keywords: "server api key shared folder slots downloads" },
  { category: "connections", label: "Discogs", target: "discogs-connection", keywords: "personal token api key catalog onetagger metadata" },
  { category: "connections", label: "Last.fm", target: "lastfm-connection", keywords: "api key artist top tracks popularity" },
  { category: "connections", label: "AcoustID", target: "acoustid-connection", keywords: "api application key fingerprint recognition" },
  { category: "connections", label: "Subsonic", target: "subsonic-settings", keywords: "navidrome strawberry server music library sign in login refresh fallback" },
  { category: "connections", label: "Jellyfin", target: "jellyfin-settings", keywords: "server music library sign in login show visibility refresh catalog fallback" },
  { category: "connections", label: "Browser companion & assistants", target: "connection-tools-settings", keywords: "extension firefox chrome chromium mcp assistant permissions automation" },
  { category: "advanced", label: "YouTube support", target: "release-setup-title", keywords: "unavailable denied audio playback diagnostics yt-dlp 403" },
  { category: "advanced", label: "Playback reports", target: "playback-diagnostics-settings", keywords: "diagnostics debug export log errors trace" },
  { category: "advanced", label: "Included tools", target: "tool-setup-title", keywords: "ffmpeg ffprobe yt-dlp ytdlp javascript runtime deno node dependency custom path" },
];

function words(value: string): string[] {
  return value.toLowerCase().match(/[\p{L}\p{N}]+/gu) ?? [];
}

export function findSettings(query: string, desktop: boolean): Setting[] {
  const terms = words(query);
  if (!terms.length) return [];

  const matches = [];
  for (const setting of settings) {
    if (setting.desktopOnly && !desktop) continue;
    const label = words(setting.label);
    const searchable = [...label, ...words(setting.keywords ?? "")];
    const matchesTerms = (tokens: string[]) => terms.every(term => tokens.some(token => token.startsWith(term)));
    if (matchesTerms(searchable)) {
      const relevance = label.join(" ") === terms.join(" ") ? 2 : Number(matchesTerms(label));
      matches.push({ setting, relevance });
    }
  }
  return matches.sort((a, b) => b.relevance - a.relevance).map(({ setting }) => setting);
}
