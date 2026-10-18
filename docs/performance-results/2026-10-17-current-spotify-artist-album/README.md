# Current Spotify artist and album read acceptance

The current normal Linux debug app (`7a43ea0c8def`, frontend `3943b23cfb37`) successfully reads the reported Bulgarian State Television Female Choir artist and its Vol. 2 album. The artist response has eight release rows, reported total eight and no next cursor. The selected album has 16 unique tracks, reported total16 and no next cursor; all16 include finite positive durations and artwork. The earlier generic artist-release GraphQL error does not reproduce in these exact reads. This does not prove completeness for other artists/accounts or every release.

The current full album response supplies 131.706 seconds for Kaval Sviri. An initial summary assertion incorrectly expected the older integer fixture value131; the assertion failed without modifying application data. The corrected check retains the fractional duration rather than rounding it to match old evidence.

Library counts before/after the album read are identical (4,964 tracks,42 playlists,15 assets). The complete normal playback queue/occurrence IDs/order/index, paused position3.287602s, volume0.8, repeatoff and shuffleoff match the restored snapshot. No playlist writes, imports, source substitutions or playback controls were performed.

App status reports managed transport `native-webview-http`, OS-keyring credential storage and no companion connection seen. This supports this read path using the managed session; it does not qualify all restart/expiry/account-switch cases or writeback. The reads neither require a newly opened external browser nor prove absence of unrelated browser processes.

A bounded artist-album shape diagnostic was developed privately and reviewed, but not integrated because no current failure was reproduced. Its staged tests pass; it is a future troubleshooting aid, not a shipped fix or live provider qualification.

Private source responses, counts, playback snapshot and finite summary are under `.cache/qualification-current-spotify-artist/`. Spotify playlist work remains deferred. No release approval follows from this bounded acceptance.

A bounded read after the subsequent app restart again returns8/8 releases and no cursor on the same account, without requesting an interactive login. This checks one saved-session read across restart; expiry, account switching and writeback are still unqualified. The finite restart summary is retained privately.

Synthetic ListPlatform artist-duration coverage now exercises the shared adapter and managed worker with 131 seconds plus 706,000,000 nanoseconds (131.706 seconds), and omitted nanoseconds (131 seconds). Both focused fixture files pass. A private adapter negative control changes only the expected fractional value to 132.706 and fails; restoring it passes, proving the selected assertion executes under Node 26's file-level reporting. The worker fixture passes but was not independently negative-controlled. This is test coverage, not a live-response or backfill claim; no current rounding defect was demonstrated. Evidence is retained in `.cache/qualification-fruit-unpinned-webview/measured20/duration-fixtures/` and the integrated focused-test log.
