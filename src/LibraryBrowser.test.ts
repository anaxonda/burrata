// @vitest-environment jsdom
// @ts-expect-error Use the browser runtime in jsdom.
import { mount, tick, unmount } from '../node_modules/svelte/src/index-client.js';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
// @ts-expect-error Use the browser runtime in jsdom.
vi.mock('svelte', async () => await import('../node_modules/svelte/src/index-client.js'));
import LibraryBrowser from './LibraryBrowser.svelte';
import SaveArtistButton from './SaveArtistButton.svelte';
import { initialLibraryBrowserState, saveCompleteLibraryAlbum, completeLibraryAlbumTrackCount, type LibraryEntity } from './lib/library';
import type { Track } from './lib/types';

let destroy: (() => void) | undefined;
beforeEach(() => { globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver; });
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ''; });
async function settle() { for (let i=0;i<15;i++) await Promise.resolve(); await tick(); }
const artist: LibraryEntity = { id: 'artist:ref:spotify:one', kind: 'artist', name: 'Artist', artist: '', provider: 'spotify', providerId: 'one', explicitlySaved: true, trackCount: 1, albumCount: 1 };
const saved: Track = { id: 'fixture:saved', provider: 'fixture', providerId: 'saved', title: 'Saved song', artist: 'Artist', sourceUrl: '', favorite: false, provenance: {} };
function render(extra: Record<string, unknown> = {}) {
  const target = document.createElement('div'); document.body.append(target);
  const props = { browserState: { ...initialLibraryBrowserState(), tab: 'artists' }, call: vi.fn(async (_command: string, _args?: Record<string, unknown>) => ({ items: [artist], total: 1 })), onNavigate: vi.fn(), onStateChange: vi.fn(), onBrowse: vi.fn(), onPlay: vi.fn(), onQueue: vi.fn(), onDownload: vi.fn(), onFavorite: vi.fn(), onPlaylist: vi.fn(), onArtist: vi.fn(), onAlbum: vi.fn(), onContext: vi.fn(), onAddSelectedToPlaylist: vi.fn(), ...extra };
  const instance = mount(LibraryBrowser, { target, props: props as never }); destroy = () => unmount(instance);
  return { target, props };
}

describe('saved library views', () => {
  it('loads paginated saved artists and opens their library albums', async () => {
    const { target, props } = render(); await settle();
    expect(props.call).toHaveBeenCalledWith('list_library_entities', expect.objectContaining({ kind: 'artist', artistId: null, offset: 0, limit: 200 }));
    target.querySelector<HTMLButtonElement>('.entity-card')!.click(); await settle();
    expect(props.onNavigate).toHaveBeenCalledWith(expect.objectContaining({ entity: artist, section: 'albums', tab: 'artists' }));
  });

  it('plays only returned saved tracks and never requests a provider catalog', async () => {
    const call = vi.fn(async (_command: string, _args?: Record<string, unknown>) => ({ tracks: [saved], total: 1 }));
    const { target, props } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'artists', entity: artist, section: 'tracks' }, call });
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>('button')].find(b => b.textContent === 'Play saved tracks')!.click();
    expect(props.onPlay).toHaveBeenCalledWith(saved, [saved], 0);
    expect(call.mock.calls.every(([command]) => command === 'list_library_entity_tracks')).toBe(true);
  });

  it('scopes albums to the selected artist and preserves browse as an explicit action', async () => {
    const call = vi.fn(async (_command: string, _args?: Record<string, unknown>) => ({ items: [], total: 0 }));
    const { target, props } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'artists', entity: artist }, call });
    await settle();
    expect(call).toHaveBeenCalledWith('list_library_entities', expect.objectContaining({ kind: 'album', artistId: artist.id }));
    expect(props.onBrowse).not.toHaveBeenCalled();
    [...target.querySelectorAll<HTMLButtonElement>('button')].find(b => b.textContent === 'Browse catalog')!.click();
    expect(props.onBrowse).toHaveBeenCalledWith(artist);
  });

  it('shows a retryable error when loading saved music fails', async () => {
    const call = vi.fn().mockRejectedValueOnce(new Error('Database unavailable')).mockResolvedValue({ items: [artist], total: 1 });
    const { target } = render({ call }); await settle();
    expect(target.querySelector('[role="alert"]')?.textContent).toContain('Database unavailable');
    [...target.querySelectorAll<HTMLButtonElement>('button')].find(b => b.textContent === 'Retry')!.click(); await settle();
    expect(target.querySelector('.entity-card')?.textContent).toContain('Artist');
  });

  it('saves an artist without saving any tracks, then removes only its explicit save', async () => {
    const call = vi.fn(async (_command: string, _args?: Record<string, unknown>) => ({ items: [], total: 0 }));
    const target = document.createElement('div'); document.body.append(target);
    const instance = mount(SaveArtistButton, { target, props: { name: 'Artist', provider: 'spotify', providerId: 'one', call: call as never } });
    destroy = () => unmount(instance); await settle();
    target.querySelector<HTMLButtonElement>('button')!.click(); await settle();
    expect(call).toHaveBeenCalledWith('save_library_artist', expect.objectContaining({ entry: expect.objectContaining({ name: 'Artist', providerId: 'one' }) }));
    expect(target.querySelector('button')?.textContent).toBe('Remove artist save');
    target.querySelector<HTMLButtonElement>('button')!.click(); await settle();
    expect(call).toHaveBeenCalledWith('remove_library_entity_save', { id: artist.id });
    expect(call.mock.calls.some(([command]) => command === 'save_library_tracks')).toBe(false);
  });

  it('never writes a partial or unknown album tracklist', async () => {
    const call = vi.fn();
    const album = { ...artist, kind: 'album' as const };
    await expect(saveCompleteLibraryAlbum(call, album, [saved], 2)).rejects.toThrow('complete album');
    await expect(saveCompleteLibraryAlbum(call, album, [saved], null)).rejects.toThrow('complete album');
    expect(call).not.toHaveBeenCalled();
    await saveCompleteLibraryAlbum(call, album, [saved], 1);
    expect(call).toHaveBeenCalledWith('save_library_album', { entry: { ...album, totalTracks: 1 }, tracks: [saved], expectedCount: 1 });
  });

  it('accepts verified complete releases with no card count and rejects partial release evidence', () => {
    const complete={...saved,provenance:{libraryAlbumSnapshot:{complete:true,total:1}}};
    expect(completeLibraryAlbumTrackCount([complete],null)).toBe(1);
    expect(completeLibraryAlbumTrackCount([complete],12)).toBe(1);
    expect(completeLibraryAlbumTrackCount([{...saved,provenance:{libraryAlbumSnapshot:{complete:false,total:2}}}],1)).toBeNull();
    expect(completeLibraryAlbumTrackCount([complete,saved],2)).toBeNull();
  });

  it('recognizes an artist saved through another provider without relying on its display name', async () => {
    const call=vi.fn(async()=>({ ...artist,id:'artist:ref:musicbrainz:canonical',name:'Different catalog spelling',aliases:[artist.id,'artist:ref:musicbrainz:canonical'] }));
    const target=document.createElement('div');document.body.append(target);
    const instance=mount(SaveArtistButton,{target,props:{name:'Spotify spelling',provider:'spotify',providerId:'one',call:call as never}});
    destroy=()=>unmount(instance);await settle();
    expect(call).toHaveBeenCalledWith('get_library_entity',{id:artist.id});
    expect(target.querySelector('button')?.textContent).toBe('Remove artist save');
  });

  it('does not let an older saved-status lookup overwrite a completed save', async () => {
    let finishLookup!: (value:null)=>void;
    const pending=new Promise<null>(resolve=>{finishLookup=resolve;});
    const call=vi.fn(async(command:string)=>command==='get_library_entity' ? pending : undefined);
    const target=document.createElement('div');document.body.append(target);
    const instance=mount(SaveArtistButton,{target,props:{name:'Artist',provider:'spotify',providerId:'one',call:call as never}});
    destroy=()=>unmount(instance);await settle();
    target.querySelector<HTMLButtonElement>('button')!.click();await settle();
    finishLookup(null);await settle();
    expect(target.querySelector('button')?.textContent).toBe('Remove artist save');
  });
});
