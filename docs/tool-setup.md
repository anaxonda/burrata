# Tool setup

Burrata's release packaging stages its command-line tools alongside the app. The
packaged set includes yt-dlp, Node.js, FFmpeg/ffprobe, the native recognition
helper, and the browser/MCP companion tools. Node.js is shared by yt-dlp's
JavaScript challenge support and the local MCP services. The portable recognition
helper and official Chromaprint `fpcalc` binaries are included in all three release
targets. `slskd` is an optional, separately installed service used only for Soulseek.

The release resource layout and versions are pinned during packaging. Native jobs
install-smoke the DMG, NSIS installer, and Linux DEB; fresh-machine product behavior
and macOS/Windows security prompts still need manual qualification.
If a packaged tool is missing or cannot start, open **Settings → Tools Burrata
uses** and select **Check tools**. Keep the default executable names to use the
packaged tools. The collapsed **Advanced: use custom executable paths** section
accepts a full path for a system-installed override.

## YouTube support and updates

The bundled yt-dlp is the tested baseline. Burrata can stage a managed version in
app data and switch to it through an atomic version pointer, leaving the bundled
copy intact. In YouTube support settings, **Recommended** checks the configured
Burrata release manifest when one has been published. Until that feed is
available, the check reports that status rather than claiming an update is
current. **Official nightly** checks the upstream yt-dlp nightly release and
verifies its SHA-256 and SHA-512 manifests before staging. Burrata runs a real
YouTube compatibility probe before activating an update. If an update fails or
causes trouble, use **Rollback** to return to the prior managed version or the
bundled baseline.

The bundled Node runtime and yt-dlp's packaged JavaScript components support
YouTube challenge solving. Burrata also starts the anonymous BgUtils 2.0.2 local
provider on demand when available; it is bound to loopback and uses the trusted,
packaged plugin directory. It stops when idle and on app shutdown. No YouTube
account, browser cookies, or sign-in is used. A version check alone does not
prove that a particular video can be resolved; use the available YouTube support
diagnostic when search or media inspection fails.

Preview resolves a temporary audio stream for playback. It does not download or
retain the source audio; acquisition happens only after you explicitly start it.

## What each tool does

- **yt-dlp** searches YouTube and SoundCloud, reads source metadata and preview
  streams, and retrieves a source after you explicitly start an acquisition.
- **Node.js** runs yt-dlp's packaged JavaScript challenge support and the bundled
  local MCP/browser helper services. Deno is not required by the packaged setup.
- **FFmpeg and ffprobe** handle audio conversion, MP3 compatibility exports,
  metadata checks, and BPM estimation when trusted catalog metadata has no BPM.
  BPM estimates can be imperfect. Decoded audio also feeds Burrata's bundled key
  detector; uncertain or non-tonal audio can retain an unknown key.
- **Recognition helper** supports automatic matching of newly downloaded tracks.
  Chromaprint `fpcalc` is included on all release targets; AcoustID lookup also
  needs a saved application key. See
  [discovery, chapters, and autotagging](discovery-chapters-autotag.md) for the
  review and metadata boundaries.
- **slskd** is an optional local Soulseek service. Burrata does not bundle or
  install it; configure it only if you use Soulseek.

## If a check fails

Select **Check tools** in Settings and use the full diagnostic message to identify
the missing resource or configured path. If you use a custom executable path,
expand the advanced section, correct the path, and save the settings. On Windows,
use an executable path ending in `.exe` where applicable. Restart Burrata after
changing system `PATH` so the app receives the updated environment.

If yt-dlp starts but YouTube search or inspection fails, run the YouTube support
diagnostic on a public video URL and review its result. Errors can depend on
yt-dlp compatibility, network access, or YouTube's current response. A successful
version check does not establish live provider availability. If you installed a
custom yt-dlp, the [yt-dlp EJS setup guide](https://github.com/yt-dlp/yt-dlp/wiki/EJS)
describes its JavaScript challenge requirements.
