# Existing YouTube session probe

The running main development app (`8e2bdfa4…`) reports no confirmed native Spotify or YouTube account, stopped mode and credential storage not-confirmed. A read-only `list_remote_playlists` request for YouTube failed with `OPERATION_FAILED`; the redacted error category is `COMPANION_UNAVAILABLE`. No playlist names, account identifiers, credentials or raw provider error were recorded. No remote mutation was attempted.

This does not establish whether the user's ordinary browser is signed into YouTube. It establishes that the current app cannot use that login through its available transport yet. Reusing it requires the explicit one-time companion pairing flow, or interactive account confirmation in the app-owned isolated login window. No browser cookie database was read or migrated.

The native-session settings now explain that confirmed account jobs start workers on demand. A separate bug hid Resume provider access for a stopped YouTube-only account because it required a Spotify account; the condition now accepts either provider. The component test verifies YouTube-only resume, and the focused suite passes eight tests. Actual authenticated YouTube reads and writeback remain unqualified.
