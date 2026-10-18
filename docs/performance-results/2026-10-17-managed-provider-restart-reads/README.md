# Managed-session read checkpoint

The sole normal Linux app uses native `7a43ea0c8def` and App `da4309cd0191` for these reads, after the preceding verified restarts. Both provider-confirmed flags are true, revision1, credential storage is OS keyring, transport is `native-webview-http`, and companion connection-seen is false. This task opens no login or external-browser window and performs no provider writes/imports.

The known Spotify artist returns8/8 release rows with no cursor. Its response account ID matches the prior accepted artist read. The YouTube account returns62 unique playlist IDs with no next cursor and `truncated:false`; `complete` and total count are null. This demonstrates a larger usable listing than the earlier two special collections, not full account completeness. The implementation routes listing through general YouTube (`www.youtube.com` WEB), rather than restricting it to YouTube Music.

Confirmed flags and revisions remain unchanged. The redacted status exposes flags, not account IDs, so it cannot independently establish a before/after YouTube account-ID comparison. The current response ID is retained privately as a baseline for a subsequent restart check. Library counts and the complete playback snapshot compare exactly equal before and after these reads; no playback controls were used.

Private source responses and finite summary: `.cache/qualification-desktop-downloads-settings/`. Expiry/refresh causality, account switching, broad pagination completeness, interactive reauthentication and writeback remain unqualified. Spotify playlist work stays deferred. This is bounded read acceptance, not complete integration or release qualification.

## Subsequent current-app restart read

After the verified restart to App `33260ec39305` (same native `7a43ea0c8def`), a fresh managed YouTube listing returns the same response account ID and 62 unique playlist IDs. Confirmed flags and account revisions remain equal; transport is `native-webview-http`, with companion connection-seen false. No login window, import or remote write was invoked. `complete` and total remain null, so this still does not establish account completeness. Evidence: `.cache/qualification-desktop-downloads-settings/restart-read-summary.json` and private response snapshots. This closes one bounded YouTube restart read, not forced expiry, account switching or broad integration qualification.

A fresh read-only writeback candidate search for the unresolved 267-second Magalenha recording returns five Extended Mix uploads, 265–268 seconds. Their artist fields are uploader/channel names rather than independently verified recording credits. No candidate was selected and no edit was applied. The distinction needs explicit review before an exact write proposal; YouTube writeback remains off.

The subsequent Spotify artist read also returns8/8 release rows with no cursor on the same response account ID as the pre-restart read. Library statistics compare exactly equal to the pre-restart checkpoint. Evidence: `spotify-artist-restart-private.json` and `library-restart-read-private.json` in the same private evidence directory. This is an artist read, not Spotify playlist work.
