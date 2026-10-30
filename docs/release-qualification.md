# Release download qualification

Use this checklist to qualify the actual public release downloads on clean
supported machines. GitHub-hosted CI runners are useful for repeatable build and
installer smoke checks, but they are not clean customer VMs. The installed smoke
checks exercise the packaged tool paths and generated media fixtures; OS security
prompts and full product behavior still need manual review.

## Before testing

- Download the installer for the machine's supported OS and verify its SHA-256
  against the release inventory.
- Use a fresh VM or user profile with no Burrata data and no separately installed
  Node.js, FFmpeg, ffprobe, yt-dlp, or fpcalc. Record the OS version and installer
  filename with the result.
- Use only synthetic or authorized test media. Do not include credentials or
  private library files in screenshots, logs, or issue reports.

## Install and media workflow

1. Install and launch Burrata. Record any Gatekeeper, SmartScreen, or other OS
   security prompts and whether the app starts after the user's decision.
2. Open **Settings → Troubleshooting → Included tools** and run **Check tools**. Confirm packaged
   tools are available without system media tools on `PATH`.
3. Import a short synthetic or authorized audio file. Play it, inspect its
   metadata, and run BPM/key analysis where the recording is suitable for those
   measurements.
4. Convert the file to MP3 and Opus. Add or preserve title, artist, and PNG cover
   art, then reopen the output in Burrata and confirm the metadata and artwork.
5. Restart Burrata and confirm the library entry and outputs remain available.
   If testing an upgrade, install the new release over the previous version and
   verify existing library data remains intact.

## App and integration workflow

- Test browser companion capture from a supported chart or playlist while the
  companion and Burrata are running.
- Test MCP `get_app_status` and the native messaging host heartbeat using the
  installed app's documented setup.
- Optionally test a public YouTube or SoundCloud URL. Provider availability can
  change and this live-network check is separate from required installer media
  tests.
- On Linux, test the DEB, RPM, and AppImage separately. Automated installed smoke
  currently installs the DEB; building the other formats does not qualify their
  installation or runtime behavior.
- Uninstall Burrata and verify the app is removed. Confirm user data follows the
  documented retention/removal behavior for that platform.

Record each platform and installer result separately, including failures and
manual OS security prompts. Do not describe hosted-runner smoke as fresh-machine
qualification.

## Artist biography check (2026-10-30)

A private Linux headless instance of candidate SHA-256 `e8cf78f96f6653cecc3c667407694ff1f0d1ee45d1f294e3bc9cb88bc693d434` returned a nonempty Wikipedia biography and the expected article link for MusicBrainz artist `0c4d8428-05fc-4386-a428-f12615479651` (Ghazi Abdel Baki). The linked Wikidata entity was `Q16148373`, and the Wikipedia summary matched that entity. The in-memory automation result was corroborated against the private profile's retained provider cache; the original response could not be saved because the temporary filesystem quota was exhausted. A bounded evidence record is retained at `.cache/playback-audit-evidence/20261030/ghazi-wikipedia-profile.json` in the local workspace.

This qualifies the direct MusicBrainz profile path only. It does not establish that the original player card resolved that identity, that a Spotify crosswalk was present, or that other platforms were exercised. The private instance used no audio server.
