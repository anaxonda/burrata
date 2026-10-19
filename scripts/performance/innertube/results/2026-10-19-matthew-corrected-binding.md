# Corrected token-binding capability probe

On 2026-10-19, the isolated MWEB worker used anonymous BgUtils tokens with separate bindings: the player-request token used the session `visitorData`, and the GVS stream token used the explicit video ID. The probe targeted public upload `uZGRATckoBM` (John Denver – Matthew), returned five seconds of PCM, and recorded `firstPcmFromCommandMs = 11,837.7`. The worker request timer starts when the JSONL request is received; worker process launch is excluded. Session initialization took 5,214.7 ms and token work took 5,829.2 ms.

This confirms that the corrected token-binding path can obtain decodable audio for this explicit video ID under the tested conditions. `recordingVerified` here means only that the resolved response matched the requested provider video ID. It does not verify that this upload is the catalog's intended recording by acoustic comparison, nor establish version/credit equivalence, broad availability, production fallback behavior, or a speed advantage. The complete redacted success row is in [`2026-10-19-matthew-corrected-binding.json`](./2026-10-19-matthew-corrected-binding.json); the original denied `STREAM_DENIED` result is preserved in [`2026-10-19-matthew-direct-capability-denied.json`](./2026-10-19-matthew-direct-capability-denied.json).

The next diagnostic is a strict-itag transport matrix of three explicit public music uploads, not a matched-catalog or acoustic-identity study. A tracked flat-search discovery result and manifest preserve each video ID, title, channel, and duration. The Poso upload is 251 seconds while the catalog recording is 245.48 seconds; it remains a source/version-difference candidate. Each track gets a fresh-worker request, a same-ID repeat, and a warm different-ID request; triplet order rotates across tracks. Every request pins itag 251 with no fallback, sends the youtubei MWEB stream-header policy to FFmpeg, requires the exact requested response ID, and reports metadata/format matches. Each request has a 20-second cap and the full nine-request matrix a 240-second cap. No new playback request has been run.

```bash
python3 scripts/performance/innertube/serial_same_worker.py \
  --manifest scripts/performance/innertube/fixtures/transport-music-tracks.json \
  --output scripts/performance/innertube/results/2026-10-19-transport-format251-triplets.json
```
