<script lang="ts">
  import { onDestroy } from "svelte";
  import { ProviderReadCancellation } from "./lib/provider-read-cancellation";

  type AlbumCrosswalkSummary = { verified: true; spotifyTrackId: string; musicbrainzReleaseId: string; spotifyPosition: number };
  type RecordingCreditSummary = {
    status: "recording_credits_verified";
    spotifyTrackId: string;
    musicbrainzRecordingId: string;
    sourceIsrc: string;
    artistCreditCount: number;
    albumLinkVerified: false;
  };
  type CrosswalkSummary = AlbumCrosswalkSummary | RecordingCreditSummary;
  type VerificationMode = "album" | "recording";
  let { trackId, albumIdHint, call, onRetryPreview }: {
    trackId: string;
    albumIdHint?: string;
    call: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
    onRetryPreview: () => void;
  } = $props();

  const providerReads = new ProviderReadCancellation((command, args) => call(command, args ?? {}));
  let generation = 0;
  let busy = $state(false);
  let error = $state("");
  let verified = $state<"album" | "recording" | "">("");
  let mode = $state<VerificationMode>("album");
  let recordingLocator = $state("");

  const validAlbumHint = $derived(!!albumIdHint && /^[A-Za-z0-9]{22}$/.test(albumIdHint));

  $effect(() => {
    const identity = trackId;
    const locator = albumIdHint;
    const owner = ++generation;
    providerReads.cancelAll();
    busy = false;
    error = "";
    verified = "";
    mode = "album";
    recordingLocator = "";
    if (!identity) return;
    return () => {
      if (generation === owner) generation++;
      providerReads.cancelAll();
    };
  });

  onDestroy(() => {
    generation++;
    providerReads.cancelAll();
  });

  function parseRecordingId(value: string): string | undefined {
    const input = value.trim();
    const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
    if (uuid.test(input)) return input.toLowerCase();
    try {
      const url = new URL(input);
      if (url.protocol !== "https:" || url.hostname !== "musicbrainz.org" || url.username || url.password || url.search || url.hash) return;
      const match = /^\/recording\/([0-9a-f-]{36})\/?$/i.exec(url.pathname);
      return match && uuid.test(match[1]) ? match[1].toLowerCase() : undefined;
    } catch {
      return undefined;
    }
  }

  async function verify() {
    const identity = trackId;
    const locator = albumIdHint;
    const selectedMode = mode;
    const recordingId = selectedMode === "recording" ? parseRecordingId(recordingLocator) : undefined;
    const owner = ++generation;
    busy = true;
    error = "";
    verified = "";
    try {
      if (selectedMode === "recording" && !recordingId) {
        throw new Error("Enter a MusicBrainz recording URL or UUID.");
      }
      const args: Record<string, unknown> = { trackId: identity };
      if (selectedMode === "album" && locator && /^[A-Za-z0-9]{22}$/.test(locator)) args.albumIdHint = locator;
      if (selectedMode === "recording" && recordingId) args.musicBrainzRecordingIdHint = recordingId;
      const result = await providerReads.read<CrosswalkSummary>("prepare_spotify_recording_crosswalk", args);
      if (generation !== owner || trackId !== identity || albumIdHint !== locator || mode !== selectedMode) return;
      if (!result || result.spotifyTrackId !== identity) {
        throw new Error("The verified recording did not match this Spotify track.");
      }
      if (selectedMode === "album") {
        if (!("verified" in result) || result.verified !== true) {
          throw new Error("The Spotify album occurrence could not be verified.");
        }
        verified = "album";
      } else {
        if (!("status" in result) || result.status !== "recording_credits_verified"
          || result.musicbrainzRecordingId !== recordingId || result.albumLinkVerified !== false
          || !/^[A-Z0-9]{12}$/.test(result.sourceIsrc) || !Number.isInteger(result.artistCreditCount) || result.artistCreditCount < 1) {
          throw new Error("The artist credits could not be verified for this recording.");
        }
        verified = "recording";
      }
    } catch (failure) {
      if (generation !== owner || trackId !== identity || albumIdHint !== locator || mode !== selectedMode) return;
      const detail = typeof failure === "string" ? failure.trim() : failure instanceof Error ? failure.message.trim() : "";
      error = detail ? `Could not verify recording identity: ${detail.slice(0, 180)}` : "Could not verify recording identity. Check the Spotify connection and try again.";
    } finally {
      if (generation === owner && trackId === identity && albumIdHint === locator && mode === selectedMode) busy = false;
    }
  }
</script>

<section class="identity-check" aria-label="Spotify recording identity">
  <fieldset disabled={busy}>
    <legend>Verification method</legend>
    <label><input type="radio" name="identity-mode" value="album" bind:group={mode} onchange={() => { verified = ""; error = ""; }} /> Verify Spotify recording identity{#if validAlbumHint} against album occurrence{/if}</label>
    <label><input type="radio" name="identity-mode" value="recording" bind:group={mode} onchange={() => { verified = ""; error = ""; }} /> Verify artist credits from a MusicBrainz recording</label>
  </fieldset>
  {#if mode === "recording"}
    <label class="locator-label" for="musicbrainz-recording-locator">MusicBrainz recording URL or UUID</label>
    <input id="musicbrainz-recording-locator" class="locator" type="text" bind:value={recordingLocator} disabled={busy} placeholder="musicbrainz.org/recording/…" autocomplete="off" spellcheck="false" />
    <small>Checks the saved Spotify row against this recording’s artist credits. It does not verify an album link.</small>
  {/if}
  <button type="button" disabled={busy || !trackId} onclick={() => void verify()}>
    {busy ? "Verifying recording…" : verified ? "Verify again" : mode === "recording" ? "Verify artist credits" : validAlbumHint ? "Verify recording identity (album occurrence)" : "Verify recording identity"}
  </button>
  {#if busy}<small role="status">Checking the selected recording identity. This read is limited to 30 seconds.</small>
  {:else if verified === "album"}<small role="status">{validAlbumHint ? "Spotify album occurrence verified." : "Recording identity verified."} Retry Preview to use its catalog credits.</small><button type="button" class="retry" onclick={onRetryPreview}>Retry Preview</button>
  {:else if verified === "recording"}<small role="status">Artist credits verified for this Spotify track and MusicBrainz recording. Album link not verified.</small><button type="button" class="retry" onclick={onRetryPreview}>Retry Preview</button>
  {:else if error}<small class="error" role="alert">{error}</small>{/if}
</section>

<style>
  .identity-check{display:flex;flex-wrap:wrap;align-items:center;gap:.4rem .55rem;margin-top:.35rem;font-size:.78rem;color:var(--muted,#666)}
  fieldset{display:flex;flex-wrap:wrap;gap:.35rem .7rem;border:0;padding:0;margin:0;width:100%}
  legend{font-weight:600;margin-bottom:.15rem}
  label{display:inline-flex;align-items:center;gap:.3rem}
  input[type="radio"]{margin:0}
  .locator-label{width:100%;font-weight:600}
  .locator{flex:1 1 15rem;min-width:0;padding:.35rem .45rem}
  small{flex-basis:100%;line-height:1.35}
  button{font:inherit;padding:.35rem .55rem}
  button.retry{flex-basis:auto}
  .error{color:var(--danger,#a22)}
</style>
