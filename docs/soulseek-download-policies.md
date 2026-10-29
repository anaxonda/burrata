# Soulseek download policies

Burrata connects directly to **slskd** through its HTTP API. Other Soulseek clients are not API adapters in Burrata; their completed files can be imported normally. Remote peers may use any compatible Soulseek client.

Configure slskd in Connections, then choose download behavior under **Settings → Downloads**. The small information controls explain each option without adding persistent instruction blocks.

## Waiting at Soulseek's position

**When Soulseek has no match** defaults to **Continue to the next source**. **Wait for Soulseek** holds the recording when the configured source order reaches Soulseek and no acceptable file can be acquired. Sources before Soulseek still run. Later sources are held for that recording; other downloads continue.

A waiting request is durable, identified by its recording, output profile, playlist context and quality requirement. It preserves the destination, folder layout and recording-match policy captured when it was queued. Changing matching strictness does not silently broaden an existing request. Waiting consumes no transfer or processing slot. A specific file already queued with a peer is instead labelled **Waiting for peer**; slskd manages that transfer.

## Lossless checks after a lossy save

**Keep looking for a lossless copy** defaults off and is independent of fallback order. After a permitted lossy source download, Burrata can keep a separate request for a lossless copy of the same recording. MP3 output and chapter downloads do not create upgrade requests. A strict lossless request still cannot silently accept a lossy fallback.

Candidates must pass the saved recording policy. Ambiguous results become **Needs review**. A lossless filename or advertised format alone is insufficient: the acquired file also passes the decoded-codec quality guard.

The existing copy remains available until a validated replacement can commit. Replacement uses the original asset ID and expected content hash, preserves the library's asset identity and saves the old file as a backup. A changed or unmanaged target cannot be silently replaced. Native playback ownership is checked before starting and again under the playback state lock at commit. A downloaded upgrade can wait for the recording to leave playback without occupying a transfer/processing slot. Automatic replacement is deferred when the active playback route cannot confirm the file is idle.

## Downloads controls and scheduling

Waiting requests and upgrades share the Downloads view, with a filter and a compact per-request menu: **Check now**, **Choose source**, **Pause**, **Resume checks**, **Cancel request**. Choosing another source pauses automatic selection first. Once a source is queued, the existing transfer row owns progress and cancellation; a duplicate waiting row is not shown. Cancelling a check leaves the saved playable copy in place.

A single backend scheduler performs bounded ordinary slskd searches serially while Burrata is open, at least a minute apart. The oldest-due request is selected first so large lists remain fair; explicit checks take priority. Normal rechecks are hourly; failed searches back off to at most eight hours. There are at most 500 retained active requests. Checks require a configured connection. Cancellation/revision checks prevent a late search response from enqueuing a cancelled request. Restart reconciles stamped durable jobs instead of submitting them twice.

Full-profile backups retain waiting requests in a paused state and remove the source computer's destination and in-flight job association. Resuming explicitly binds the request to this computer's configured download destination. Completed and cancelled requests are not reactivated by restore.

The separate **Saved Soulseek searches** dialog retains the older free-text subscriptions. These offer search results for manual review and never automatically download a recording. They are not substitutes for recording-aware download waiting or lossless checks.
