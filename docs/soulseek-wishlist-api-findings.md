# Soulseek wishlist API findings

Checked 2026-10-22 against upstream slskd master.

The current search controller starts network searches. Its request DTO exposes text, timeout and response/file/peer filters, but no wishlist scope, saved schedule or recurring-search controls. An upstream wishlist feature request remains open. Therefore Burrata cannot enable wishlist behavior by setting a documented slskd request flag.

Sources:
- https://github.com/slskd/slskd/blob/master/src/slskd/Search/API/Controllers/SearchesController.cs
- https://github.com/slskd/slskd/blob/master/src/slskd/Search/API/DTO/SearchRequest.cs
- https://github.com/slskd/slskd/issues/1611

Implementation still required: persist explicit user-created search subscriptions in Burrata, run bounded serial ordinary searches while the app is open, retain next-run/backoff state across restarts, and offer results for review through the existing recording-matching and acquisition path. Enable/disable/remove controls and cancellation must accompany the scheduler. This would be recurring searches implemented by Burrata, not protocol-level wishlist searches or guaranteed background operation while the app is closed. Do not automatically broaden matching or download merely because a saved search returns a candidate.

Search timeout compatibility is already handled locally: `SlskdClient::start_search` sends milliseconds, with a regression asserting 30,000 for a 30-second request. Upstream issue https://github.com/slskd/slskd/issues/1820 describes the seconds/milliseconds documentation discrepancy. No additional timeout change is justified by this audit.
