# French Apple Silicon Mac v0.1.9 follow-up

The new report contains 1,762 events and no dropped events. Resolution outcomes:
17 successes, 10 unknown failures, one identity mismatch, one timeout, and two
cancellations. These are resolver outcomes, not proof of audible playback.

All 10 completed SoundCloud extractions failed with `extractorReason=other`.
YouTube probes returned 72 HTTP 403, 37 HTTP 206, and 26 HTTP 302 responses.
All recorded redirects were Google video CDN to Google video CDN. Direct mapped
recovery succeeded in nine of thirteen attempts. Parallel SoundCloud extraction
can spend the second normal extraction slot before YouTube's denied-stream
recovery runs. The timeout operation spent about 10 seconds searching and eight
seconds extracting; recovery had insufficient time left in the absolute deadline.

The discography screenshots establish a separate application error: valid
MusicBrainz responses over the provider cache's 1 MB limit were discarded because
cache insertion was fallible in the provider request path. Full tracklists for
100 editions made list response sizes artist-dependent. This is unrelated to
French spelling or Mac audio decoding. Profile and release warnings were also
rendered twice. The source artist identity is absent from the diagnostics, so
this screenshot association is based on the user's description.

## Changes

Provider cache insertion is best effort for artist JSON requests. Release lists
use a new slim cache namespace and omit recordings/ISRCs; exact edition tracklists
remain lazy. Warning display is deduplicated and failed continuation pages stop
automatic pagination and expose an explicit retry.

Same-recording mapped transport recovery has one dedicated atomic slot separate
from the two normal extraction slots. Identity, session, deadline, and cancellation
gates remain in place. Eligible denied sources bypass the same-URL 403 wait.
YouTube media probing permits at most three HTTPS Google video CDN redirects,
validates/pins public DNS on every hop, checks bounded head/tail ranges against the
same final URL, and passes that final URL to playback. Generic and account
redirects remain rejected.

Diagnostic exports gain bounded helper exit codes, fixed TLS/exception/provider
response reasons, and recovery skip reasons. No raw stderr or signed URLs are
added to exports. The EJS executable check is inconclusive, not proof it is missing.

## Open root causes and qualification

The original YouTube authorization failure and SoundCloud extractor error remain
unidentified. The report cannot distinguish token/client behavior, regional access,
TLS environment, or an upstream helper defect. Do not close these issues based
on extra retries or on success from a different network.

On the affected Mac, run `scripts/performance/inspect_source_failure.py` with the
exact public source page, `--helper` pointing to the bundled executable and,
optionally, `--comparison-helper` pointing to another version. It emits safe
extraction outcomes. `--private-error-file` explicitly retains stderr locally in
a new mode-0600 file; that file must not be attached to an issue or diagnostic
export. Compare the app's managed YouTube route separately: this script does not
reproduce its token broker and does not prove playback. Exact failing source links
were not provided. Installed Apple Silicon audio progress and seeking still need
qualification on the affected device/network.
