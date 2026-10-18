<script lang="ts">
  import { onDestroy } from "svelte";
  import { ProviderReadCancellation } from "./lib/provider-read-cancellation";

  type CrosswalkSummary = { verified: boolean; spotifyTrackId: string; musicbrainzReleaseId: string; spotifyPosition: number };
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
  let verified = $state(false);

  $effect(() => {
    const identity = trackId;
    const locator = albumIdHint;
    const owner = ++generation;
    providerReads.cancelAll();
    busy = false;
    error = "";
    verified = false;
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

  async function verify() {
    const identity = trackId;
    const locator = albumIdHint;
    const owner = ++generation;
    busy = true;
    error = "";
    verified = false;
    try {
      const args: Record<string, unknown> = { trackId: identity };
      if (locator && /^[A-Za-z0-9]{22}$/.test(locator)) args.albumIdHint = locator;
      const result = await providerReads.read<CrosswalkSummary>("prepare_spotify_recording_crosswalk", args);
      if (generation !== owner || trackId !== identity || albumIdHint !== locator) return;
      if (!result || result.verified !== true || result.spotifyTrackId !== identity) {
        throw new Error("The verified recording did not match this Spotify track.");
      }
      verified = true;
    } catch (failure) {
      if (generation !== owner || trackId !== identity || albumIdHint !== locator) return;
      const detail = typeof failure === "string" ? failure.trim() : failure instanceof Error ? failure.message.trim() : "";
      error = detail ? `Could not verify recording identity: ${detail.slice(0, 180)}` : "Could not verify recording identity. Check the Spotify connection and try again.";
    } finally {
      if (generation === owner && trackId === identity && albumIdHint === locator) busy = false;
    }
  }
</script>

<section class="identity-check" aria-label="Spotify recording identity">
  <button type="button" disabled={busy} onclick={() => void verify()}>
    {busy ? "Verifying recording…" : verified ? "Verify again" : "Verify recording identity"}
  </button>
  {#if busy}<small role="status">Checking the exact Spotify album occurrence. This read is limited to 30 seconds.</small>
  {:else if verified}<small role="status">Recording identity verified. Retry Preview to use its catalog credits.</small><button type="button" class="retry" onclick={onRetryPreview}>Retry Preview</button>
  {:else if error}<small class="error" role="alert">{error}</small>{/if}
</section>

<style>
  .identity-check{display:flex;align-items:center;gap:.45rem;flex-wrap:wrap;margin:.35rem 0}
  .identity-check small{color:var(--muted,#a3a8ae);font-size:.76rem}
  .identity-check .error{color:var(--error,#ff9b9b)}
  .identity-check .retry{color:var(--accent,#9dbfff)}
</style>
