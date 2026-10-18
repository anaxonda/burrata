# Burrata

Burrata is a local-first desktop music discovery and library app for DJs. Search
music catalogs and audio sources, preview tracks, organize playlists and local
files, and review metadata, artwork, BPM and key. Your library and downloaded
files stay on your computer; online searches and imports use third-party services.

## Get Burrata

The [GitHub Releases page](https://github.com/anaxonda/burrata/releases) is the
download location. Burrata is currently distributed as a prerelease.
Main releases contain installers for Apple silicon macOS, Windows x64, and Linux
x86_64 (DEB, RPM, and AppImage), browser companions and checksums.
Required runtime helpers are bundled. Soulseek uses an optional, separately installed
[slskd service](deploy/slskd/README.md).

Start with the [installation guide](docs/install.md): it covers each operating
system, first-launch prompts, browser extensions and common connection problems.
The optional [browser companion](extension/README.md#install) is supplied as
`firefox-extension-<version>.xpi` (Mozilla-signed) and
`chrome-extension-<version>.zip` (Chrome/Chromium/Edge, loaded manually).

Optional helper updates, matching dependency sources, licenses, and qualification
records are separate supporting downloads. They are not needed for normal app
installation. See [third-party notices](scripts/release/THIRD_PARTY_NOTICES.md)
and the [vendored source inventory](src-tauri/vendor/SOURCES.md).

## What it does

- Build a local library from audio files, imports, charts, and playlists.
- Search music catalogs and online audio sources, then compare candidates and
  preview available audio.
- Save playlists and collections, manage local files, and review metadata,
  artwork, BPM, and key information.
- Acquire a source only after you choose to start a download; validate and tag
  the saved file before adding it to your library.

Availability varies by provider, account and region. Some imports need an
interactive sign-in. Recording-identity verification does not guarantee a
playable source. Playlist writeback is experimental, off by default and requires
a reviewed preview; Spotify playlist creation and YouTube writeback remain
unqualified for this release.

For practical setup, see [tool setup](docs/tool-setup.md),
[connections](docs/connections.md), [chart and playlist imports](docs/chart-playlist-imports.md),
and [streaming playlists and local files](docs/streaming-playlists.md). The local
automation bridge is documented in [MCP setup](docs/mcp.md).

## Run from source

Install Node.js 22.12 or newer, Rust, CMake, and the
[Tauri prerequisites for your platform](https://v2.tauri.app/start/prerequisites/).
Then run:

```sh
git clone https://github.com/anaxonda/burrata.git
cd burrata
npm ci
npm run tauri -- dev
```

The development command starts Vite and the desktop app together. Development
checkouts may need runtime tools configured through [tool setup](docs/tool-setup.md).
Test and packaging commands are in the [release workflow](.github/workflows/release.yml).

## License

Burrata is licensed under [GPL-3.0-or-later](LICENSE). Bundled tools and other
third-party components retain their own licenses; see the notices and source
inventory linked above and in each published release.
