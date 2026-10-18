# Connections and artist discovery

Open **Settings → Sources & artist discovery**. MusicBrainz artist search,
recordings, releases and credits work without credentials. Select the intended
artist identity when names collide. Discogs results remain separate unless a
provider supplies an explicit cross-link.

## Soulseek through slskd

For this machine, use the [prepared Docker setup](../deploy/slskd/README.md).

Run [slskd](https://github.com/slskd/slskd#quick-start) separately on the same
computer and configure its Soulseek login, API access and completed-download
folder. Burrata does not install or manage the daemon.

In Burrata, enable Soulseek, enter the local address (normally
`http://localhost:5030`), its API key and the absolute path to the daemon's
completed-download folder. With Docker, use the host-side mounted path, not the
container path. Save connections, then test them. The API key needs search and
transfer access. Soulseek credentials remain in slskd.

Choose Soulseek in Search, or use any track's Download button. Review the peer,
filename, mix and available audio properties. Burrata queues and monitors the
transfer through slskd, then copies the completed file into its own acquisition
pipeline for validation, tags and library publication. The daemon's original is
retained. The folder must be readable by Burrata. A daemon connection check does
not prove that a remote peer is online or has available transfer slots.

## Spotify and YouTube playlist accounts

Connect an account in **Settings → Connections** and review the provider identity
before confirming it. Depending on platform support and the selected transport,
Burrata can make provider requests through its app-managed session or a
browser-based connection; the browser companion remains available for browser
integration and imports. The app-managed native WebView HTTP transport is
experimental and currently implemented for Linux and macOS builds. It can be opted
into in Connections and reuses the confirmed account for on-demand requests.
The native provider session requires confirmed credentials to be saved in the
operating system keyring; if secure storage is unavailable, account confirmation
fails. Burrata never asks MCP clients for streaming passwords.

A confirmed account grants provider-specific read access; confirming Spotify
does not confirm YouTube. This consent is separate from MCP tool scopes. Account
reads do not by themselves enable playlist writes. Writing changes requires a
separate opt-in for each provider and a reviewed, complete preview whose operations are supported by the playlist
capabilities before changes are sent. The current implementation and its
live qualification are different claims: Spotify app-managed reads are qualified
to one Linux account, and bounded YouTube verification covers one account and a
selected playlist only. This does not establish general account, platform,
completeness or write qualification. In MCP, remote import and refresh actions
require Control permission, previews require Read, and applying a preview
requires Manage permission.

## Optional artist providers

- **Discogs:** enable it and enter a personal token from your Discogs developer
  settings, or choose **Import token from OneTagger** to reuse its saved token
  without showing the secret. It supplies releases, editions, labels and credits.
- **Last.fm:** enable it and enter an API key for popularity-ranked artist tracks.
  Top-track results are discovery suggestions, not verified recording matches.

Keys are held in the operating system credential store, never the library SQLite
database. Blank input preserves a saved key; the removal checkbox clears it.
Session-only keeps new credentials in memory when the secure store is unavailable.
They must be entered again after restarting.

## Local analysis and tags

Library supports All / On this device / Not on this device filters. Missing BPM
and key are analyzed in the background for registered downloads. A completed job
can still have an unknown key when the audio has no confident estimate. Library's
analysis action retries missing values; available estimates carry an approximate
marker and analysis details. Catalog and manually accepted values take priority.

Metadata review applies only checked fields. Release dates use the explicitly
selected edition. Accepted corrections are saved immediately and file tags are
updated asynchronously. Retagging verifies the registered file checksum, preserves
decoded audio, and journals replacement so an interrupted update can recover at
startup. Externally modified files are refused rather than silently overwritten.
