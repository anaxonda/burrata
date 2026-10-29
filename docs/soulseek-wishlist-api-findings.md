# Soulseek wishlist API findings

Checked 2026-10-22 against upstream slskd master.

The current search controller starts network searches. Its request DTO exposes text, timeout and response/file/peer filters, but no wishlist scope, saved schedule or recurring-search controls. An upstream wishlist feature request remains open. Therefore Burrata cannot enable wishlist behavior by setting a documented slskd request flag.

Sources:
- https://github.com/slskd/slskd/blob/master/src/slskd/Search/API/Controllers/SearchesController.cs
- https://github.com/slskd/slskd/blob/master/src/slskd/Search/API/DTO/SearchRequest.cs
- https://github.com/slskd/slskd/issues/1611

Burrata now implements recurring ordinary searches itself. The original saved-query list persists explicit subscriptions locally and offers results for review; it does not download automatically. Open it from **Downloads → More download options → Saved Soulseek searches**. Existing subscriptions are retained.

Recording-aware waiting and lossless upgrades are separate download policies, configured in **Settings → Downloads**. They persist in Burrata's database, use the normal recording assessor and acquisition pipeline, and appear in Downloads. See [Soulseek download policies](soulseek-download-policies.md) for ordering, quality, cancellation and restore behavior. These are not protocol-level wishlist searches, and they do not run while Burrata is closed.

Search timeout compatibility is already handled locally: `SlskdClient::start_search` sends milliseconds, with a regression asserting 30,000 for a 30-second request. Upstream issue https://github.com/slskd/slskd/issues/1820 describes the seconds/milliseconds documentation discrepancy. No additional timeout change is justified by this audit.
