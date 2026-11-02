<script lang="ts">
  import { call as invoke } from './lib/api';
  import type { LibraryEntity } from './lib/library';
  let { name, provider, providerId, sourceUrl, artworkUrl, call = invoke }: {
    name: string; provider: string; providerId: string; sourceUrl?: string | null; artworkUrl?: string | null;
    call?: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  } = $props();
  let saved = $state(false), busy = $state(false), error = $state('');
  let actionGeneration = 0;
  const id = $derived(`artist:ref:${provider}:${providerId}`);
  $effect(() => {
    const current = id;
    const generation = ++actionGeneration;
    saved = false; busy = false; error = '';
    let active = true;
    void call<LibraryEntity | null>('get_library_entity', { id: current }).then(entry => {
      if (active && current === id && generation === actionGeneration) saved = entry?.explicitlySaved ?? false;
    }).catch(() => {});
    return () => { active = false; };
  });
  async function toggle() {
    actionGeneration++;
    const current = id, wasSaved = saved;
    busy = true; error = '';
    try {
      if (wasSaved) await call('remove_library_entity_save', { id: current });
      else {
        const entry: LibraryEntity = { id: current, kind: 'artist', name, artist: '', provider, providerId, sourceUrl, artworkUrl, explicitlySaved: true, trackCount: 0, albumCount: 0 };
        await call('save_library_artist', { entry });
      }
      if (current === id) saved = !wasSaved;
      window.dispatchEvent(new Event('library-changed'));
    } catch (reason) { if (current === id) error = String(reason instanceof Error ? reason.message : reason); }
    finally { if (current === id) busy = false; }
  }
</script>
<button type="button" disabled={busy} onclick={() => void toggle()}>{busy ? 'Saving…' : saved ? 'Remove artist save' : 'Save artist'}</button>
{#if saved}<small>Saved to Library</small>{/if}
{#if error}<p role="alert">{error}</p>{/if}
