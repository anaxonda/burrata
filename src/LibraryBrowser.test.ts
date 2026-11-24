// @vitest-environment jsdom
// @ts-expect-error Use the browser runtime in jsdom.
import { mount, tick, unmount } from '../node_modules/svelte/src/index-client.js';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
// @ts-expect-error Match the browser runtime for reactive controlled props.
import { fromStore, writable } from '../node_modules/svelte/src/store/index-client.js';
// @ts-expect-error Use the browser runtime in jsdom.
vi.mock('svelte', async () => await import('../node_modules/svelte/src/index-client.js'));
import LibraryBrowser from './LibraryBrowser.svelte';
import libraryBrowserSource from './LibraryBrowser.svelte?raw';
import albumDetailViewSource from './AlbumDetailView.svelte?raw';
import SaveArtistButton from './SaveArtistButton.svelte';
import { initialLibraryBrowserState, saveCompleteLibraryAlbum, completeLibraryAlbumTrackCount, type LibraryBrowserState, type LibraryEntity } from './lib/library';
import type { Track } from './lib/types';

let destroy: (() => void) | undefined;
const layoutPreferenceKey = 'burrata.library-browser-layouts.v1';
const layoutPrefs = new Map<string, string>();
beforeEach(() => { globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver; layoutPrefs.clear(); vi.stubGlobal('localStorage', { getItem: (key: string) => layoutPrefs.get(key) ?? null, setItem: (key: string, value: string) => layoutPrefs.set(key, value), removeItem: (key: string) => layoutPrefs.delete(key) }); });
afterEach(() => { destroy?.(); destroy = undefined; document.body.innerHTML = ''; vi.unstubAllGlobals(); });
async function settle() { for (let i=0;i<15;i++) await Promise.resolve(); await tick(); }
function clearRendered() { const dispose = destroy as (() => void) | undefined; dispose?.(); destroy = undefined; document.body.innerHTML = ''; }
const artist: LibraryEntity = { id: 'artist:ref:spotify:one', kind: 'artist', name: 'Artist', artist: '', provider: 'spotify', providerId: 'one', explicitlySaved: true, trackCount: 1, albumCount: 1 };
const albumVersion = (id: string, name: string, artistName: string, provider: string, releaseDate: string): LibraryEntity => ({ id, kind: 'album', name, artist: artistName, provider, providerId: `${provider}-id`, releaseDate, explicitlySaved: false, trackCount: 3, totalTracks: 12, albumCount: 0 });
const saved: Track = { id: 'fixture:saved', provider: 'fixture', providerId: 'saved', title: 'Saved song', artist: 'Artist', sourceUrl: '', favorite: false, provenance: {} };
function render(extra: Record<string, unknown> = {}, activeState?: { readonly current: LibraryBrowserState }) {
  const target = document.createElement('div'); document.body.append(target);
  const initialState: LibraryBrowserState = { ...initialLibraryBrowserState(), tab: 'artists' };
  const props = { get browserState() { return activeState?.current ?? initialState; }, call: vi.fn(async (_command: string, _args?: Record<string, unknown>) => ({ items: [artist], total: 1 })), onNavigate: vi.fn(), onStateChange: vi.fn(), onBrowse: vi.fn(), onPlay: vi.fn(), onQueue: vi.fn(), onDownload: vi.fn(), onFavorite: vi.fn(), onPlaylist: vi.fn(), onArtist: vi.fn(), onAlbum: vi.fn(), onContext: vi.fn(), onAddSelectedToPlaylist: vi.fn(), ...extra };
  const instance = mount(LibraryBrowser, { target, props: props as never }); destroy = () => unmount(instance);
  return { target, props };
}

describe('saved library views', () => {
  it('loads a missing portrait by exact provider ID in the artist list', async () => {
    const id = 'AAAAAAAAAAAAAAAAAAAAAA';
    const portrait = 'https://i.scdn.co/image/library-portrait';
    const entry = { ...artist, id: `artist:ref:spotify:${id}`, provider: 'spotify', providerId: id, artworkUrl: null };
    const call = vi.fn(async (command: string) => command === 'get_artist_artwork'
      ? { imageUrl: portrait } : { items: [entry], total: 1 });
    const { target } = render({ call });
    await vi.waitFor(() => expect(target.querySelector<HTMLImageElement>('.entity-card img')?.src).toBe(portrait));
    expect(call).toHaveBeenCalledWith('get_artist_artwork', { artist: { provider: 'spotify', id } });
    expect(target.querySelector('[aria-label="Filter saved artists"]')).not.toBeNull();
  });

  it('keeps a non-own-scrolling album in the outer scroll flow so Load more remains below it', async () => {
    const pageTracks = Array.from({ length: 200 }, (_, index) => ({
      ...saved, id: `fixture:page:${index + 1}`, providerId: `page:${index + 1}`, title: `Fixture track ${index + 1}`,
    }));
    const album = { ...albumVersion('album:fixture:long', 'Long fixture album', 'Fixture artist', 'musicbrainz', '2000'),
      trackCount: 400, totalTracks: 400 };
    const call = vi.fn(async (command: string) => command === 'list_library_entity_tracks'
      ? { tracks: pageTracks, total: 400 } : { items: [], total: 0 });
    const { target } = render({
      browserState: { ...initialLibraryBrowserState(), tab: 'albums', entity: album }, call,
    });
    await settle();

    const browser = target.querySelector<HTMLElement>('.library-browser')!;
    const detail = browser.querySelector<HTMLElement>('.album-detail')!;
    const loadMore = [...browser.querySelectorAll<HTMLButtonElement>('button')]
      .find(button => button.textContent?.trim() === 'Load more')!;
    // jsdom doesn't perform layout. Pin the production CSS contract and use the
    // real rendered DOM to verify the detail's ownership and sibling ordering.
    expect(libraryBrowserSource).toContain('.library-browser{min-height:0;flex:1;overflow:auto;');
    expect(albumDetailViewSource).toContain('.album-detail:not(.owns-scroll){height:auto;overflow:visible;flex-shrink:0}');
    expect(detail.classList.contains('owns-scroll')).toBe(false);
    expect(detail.closest('.library-browser')).toBe(browser);
    expect(loadMore.parentElement).toBe(browser);
    expect(detail.compareDocumentPosition(loadMore) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('remembers independent artist and album layouts and omits the toggle in track views', async () => {
    expect(initialLibraryBrowserState()).toMatchObject({ artistLayout: 'grid', albumLayout: 'grid' });
    const stateChange = vi.fn((_next: LibraryBrowserState) => {});
    const { target } = render({ onStateChange: stateChange }); await settle();
    const list = target.querySelector<HTMLButtonElement>('[aria-label="View artists as a list"]')!;
    const grid = target.querySelector<HTMLButtonElement>('[aria-label="View artists as a grid"]')!;
    expect(list.getAttribute('aria-pressed')).toBe('false');
    expect(grid.getAttribute('aria-pressed')).toBe('true');
    list.click();
    expect(stateChange).toHaveBeenLastCalledWith(expect.objectContaining({ artistLayout: 'list', albumLayout: 'grid' }));
    expect(initialLibraryBrowserState()).toMatchObject({ artistLayout: 'list', albumLayout: 'grid' });
    clearRendered();

    const albums = render({ browserState: { ...initialLibraryBrowserState(), tab: 'albums' }, onStateChange: stateChange }); await settle();
    expect(albums.target.querySelector('[aria-label="View albums as a grid"]')?.getAttribute('aria-pressed')).toBe('true');
    expect(albums.target.querySelector('.entity-grid')?.classList.contains('entity-list')).toBe(false);
    albums.target.querySelector<HTMLButtonElement>('[aria-label="View albums as a list"]')!.click();
    expect(stateChange).toHaveBeenLastCalledWith(expect.objectContaining({ artistLayout: 'list', albumLayout: 'list' }));
    clearRendered();

    const trackView = render({ browserState: { ...initialLibraryBrowserState(), tab: 'albums', entity: albumVersion('album:test', 'Test', 'Artist', 'spotify', '2000') } }); await settle();
    expect(trackView.target.querySelector('[aria-label="Library layout"]')).toBeNull();
  });

  it('keeps long artist and album card text inside cards, preserves separate profiles, and uses singular counts', async () => {
    const longName = 'The Absolutely Impossibly Long Artist Name With Several Collaborating Performers & Guests';
    const longAlbum = 'An Extremely Long Album Title That Should Wrap Cleanly Across Several Lines (Expanded Edition)';
    const profiles: LibraryEntity[] = [
      { ...artist, id: 'artist:ref:beatport:long-one', name: longName, provider: 'beatport', providerId: 'beatport-profile-with-long-reference-123456789', trackCount: 1, albumCount: 1 },
      { ...artist, id: 'artist:ref:traxsource:long-two', name: longName, provider: 'traxsource', providerId: 'traxsource-profile-with-long-reference-123456789', trackCount: 1, albumCount: 1 },
    ];
    const artistCall = vi.fn(async () => ({ items: profiles, total: profiles.length }));
    const artistView = render({ showCatalogProviderFilter: true, browserState: { ...initialLibraryBrowserState(), tab: 'artists' }, call: artistCall }); await settle();
    const artistCard = artistView.target.querySelector<HTMLElement>('.artist-family')!;
    expect(artistCard.querySelector(':scope > strong')?.textContent).toBe(longName);
    expect(artistCard.querySelectorAll('.version-choice')).toHaveLength(2);
    expect(artistCard.querySelector('details')?.hasAttribute('open')).toBe(false);
    expect(artistView.target.querySelector<HTMLSelectElement>('[aria-label="Filter artists by source"]')?.innerHTML).toContain('value="beatport"');
    expect(artistView.target.querySelector<HTMLSelectElement>('[aria-label="Filter artists by source"]')?.innerHTML).toContain('value="traxsource"');
    artistCard.querySelector('summary')!.click();
    await settle();
    expect(artistCard.querySelector('details')?.hasAttribute('open')).toBe(true);
    expect(artistCard.querySelectorAll('.version-choice')).toHaveLength(2);
    expect(artistCard.textContent).toContain('1 saved track');
    expect(artistCard.textContent).toContain('1 album');
    clearRendered();

    const longEntry = { ...albumVersion('album:ref:beatport:long', longAlbum, longName, 'beatport', '2001-11-03'), trackCount: 1, totalTracks: 1 };
    const albumCall = vi.fn(async () => ({ items: [longEntry], total: 1 }));
    const albumView = render({ browserState: { ...initialLibraryBrowserState(), tab: 'albums' }, call: albumCall }); await settle();
    const albumCard = albumView.target.querySelector<HTMLButtonElement>('.entity-card')!;
    expect(albumCard.querySelector(':scope > strong')?.textContent).toBe(longAlbum);
    expect(albumCard.querySelector(':scope > span')?.textContent).toBe(longName);
    expect(albumCard.textContent).toContain('1 of 1 track saved');
    expect(libraryBrowserSource).toContain('.entity-grid .entity-card>strong,.entity-grid .entity-card>span,.entity-grid .entity-card>small');
    expect(libraryBrowserSource).toContain('overflow-wrap:anywhere');
    expect(libraryBrowserSource).toContain('.entity-grid .entity-card>details{min-width:0;max-width:100%;box-sizing:border-box}');
  });

  it('renders artist source profile disclosures in list layout', async () => {
    const profiles = [artist, { ...artist, id: 'artist:ref:discogs:one', provider: 'discogs', providerId: 'one' }];
    const call = vi.fn(async () => ({ items: profiles, total: 2 }));
    const { target } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'artists', artistLayout: 'list' }, call }); await settle();
    expect(target.querySelector('.entity-grid')?.classList.contains('entity-list')).toBe(true);
    expect(target.querySelector('.artist-family')).not.toBeNull();
    target.querySelector<HTMLElement>('.artist-family summary')!.click();
    expect(target.querySelectorAll('.artist-family .version-choice')).toHaveLength(2);
  });

  it('loads paginated saved artists and opens their library albums', async () => {
    const { target, props } = render(); await settle();
    expect(props.call).toHaveBeenCalledWith('list_library_entities', expect.objectContaining({ kind: 'artist', artistId: null, artistGrouping: 'individual', offset: 0, limit: 200 }));
    target.querySelector<HTMLButtonElement>('.entity-card')!.click(); await settle();
    expect(props.onNavigate).toHaveBeenCalledWith(expect.objectContaining({ entity: artist, section: 'albums', tab: 'artists' }));
  });

  it('switches to credited combinations, remembers the grouping, and opens their saved album scope', async () => {
    const combination: LibraryEntity = {
      ...artist, id: 'artist:combination:nipsey-bino', name: 'Nipsey Hussle, Bino Rideaux',
      provider: null, providerId: null, explicitlySaved: false, trackCount: 3, albumCount: 2,
    };
    const call = vi.fn(async (_command: string, args?: Record<string, unknown>) => {
      if (args?.kind === 'album') return { items: [], total: 0 };
      return { items: [args?.artistGrouping === 'credited' ? combination : artist], total: 1 };
    });
    const initialState: LibraryBrowserState = {
      ...initialLibraryBrowserState(), tab: 'artists', artistLayout: 'list', query: 'Nipsey',
      entitySource: 'discogs', entitySort: 'tracks', entityDescending: true, loadedCount: 400, scroll: 80,
    };
    const state = writable(initialState);
    const onNavigate = vi.fn((next: LibraryBrowserState) => state.set(next));
    const model = fromStore(state);
    const { target } = render({ call, onNavigate, onStateChange: state.set, showCatalogProviderFilter: true }, model);
    await settle();
    const grouping = target.querySelector<HTMLSelectElement>('[aria-label="Artist grouping"]')!;
    expect(grouping.value).toBe('individual');
    grouping.value = 'credited';
    grouping.dispatchEvent(new Event('change', { bubbles: true }));
    await settle();

    expect(call).toHaveBeenLastCalledWith('list_library_entities', expect.objectContaining({
      kind: 'artist', artistGrouping: 'credited', query: 'Nipsey', source: 'discogs',
      sort: 'tracks', descending: true, limit: 200, offset: 0,
    }));
    expect(model.current).toMatchObject({ artistLayout: 'list', loadedCount: 200, scroll: 0 });
    expect(initialLibraryBrowserState().artistGrouping).toBe('credited');
    expect(target.textContent).toContain('1 of 1 credited combinations');
    target.querySelector<HTMLButtonElement>('.entity-card')!.click();
    await settle();

    expect(onNavigate).toHaveBeenCalledWith(expect.objectContaining({ entity: combination, artistGrouping: 'credited' }));
    const lastArgs = call.mock.lastCall?.[1];
    expect(lastArgs).toMatchObject({ kind: 'album', artistId: combination.id });
    expect(lastArgs).not.toHaveProperty('artistGrouping');
    expect(target.querySelector('[aria-label="Artist grouping"]')).toBeNull();
    expect(target.textContent).not.toContain('Browse catalog');
  });

  it('labels album and artist filters by the fields users can search', async () => {
    const { target } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'albums' } }); await settle();
    expect(target.querySelector<HTMLInputElement>('input[type="search"]')?.getAttribute('aria-label')).toBe('Filter saved albums');
    expect(target.querySelector<HTMLInputElement>('input[type="search"]')?.placeholder).toContain('title, artist, year, or source');
    destroy?.(); destroy = undefined; document.body.innerHTML = '';
    const artistView = render({ browserState: { ...initialLibraryBrowserState(), tab: 'artists' } }); await settle();
    expect(artistView.target.querySelector<HTMLInputElement>('input[type="search"]')?.getAttribute('aria-label')).toBe('Filter saved artists');
    expect(artistView.target.querySelector<HTMLInputElement>('input[type="search"]')?.placeholder).toContain('name');
  });

  it('shows exact-title edition families once and opens only the selected version ID', async () => {
    const catalog = albumVersion('album:ref:musicbrainz:catalog-one', 'Night Drive', 'The Band', 'musicbrainz', '1999-03-01');
    const metadata = albumVersion('album:metadata:["the band","night drive",""]', ' NIGHT   DRIVE ', 'the band', 'spotify', '2001');
    const otherArtist = albumVersion('album:ref:spotify:other-artist', 'Night Drive', 'Other Band', 'spotify', '1999');
    const deluxe = albumVersion('album:ref:spotify:deluxe', 'Night Drive (Deluxe Edition)', 'The Band', 'spotify', '1999');
    const call = vi.fn(async () => ({ items: [catalog, metadata, otherArtist, deluxe], total: 4 }));
    const { target, props } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'albums' }, call }); await settle();
    expect(target.querySelectorAll('.album-family')).toHaveLength(1);
    expect(target.querySelectorAll('.entity-card')).toHaveLength(3);
    const family = target.querySelector<HTMLElement>('.album-family')!;
    expect(family.textContent).toContain('2 versions');
    expect(family.textContent).toContain('2 sources');
    const choices = family.querySelectorAll<HTMLButtonElement>('.version-choice');
    expect(choices).toHaveLength(2);
    expect(choices[0].textContent).toContain('musicbrainz-id');
    expect(choices[0].textContent).toContain('1999-03-01');
    expect(choices[1].textContent).toContain('spotify-id');
    expect(choices[1].textContent).toContain('2001');
    choices[1].click();
    expect(props.onNavigate).toHaveBeenCalledWith(expect.objectContaining({ entity: metadata }));
    expect(props.onNavigate.mock.calls[0][0].entity.id).toBe(metadata.id);
  });

  it('groups exact artist names as separate source profiles and leaves unstructured collaborations intact', async () => {
    const metadataProfile: LibraryEntity = { ...artist, id: 'artist:metadata:["gillian welch"]', name: 'Gillian Welch', provider: null, providerId: null, explicitlySaved: false, trackCount: 14, albumCount: 0 };
    const unresolvedDiscogs: LibraryEntity = { ...artist, id: 'artist:ref:discogs:0', name: 'Gillian Welch', provider: 'discogs', providerId: '0', explicitlySaved: false, trackCount: 16, albumCount: 1 };
    const catalogProfile: LibraryEntity = { ...artist, id: 'artist:ref:musicbrainz:canonical', name: 'Gillian Welch', provider: 'discogs', providerId: '255403', aliases: ['artist:ref:musicbrainz:canonical', 'artist:ref:discogs:255403'], explicitlySaved: false, trackCount: 48, albumCount: 4 };
    const jellyfinProfile: LibraryEntity = { ...artist, id: 'artist:ref:jellyfin:scoped-jellyfin-id', name: 'Gillian Welch', provider: 'jellyfin', providerId: 'scoped-jellyfin-id', explicitlySaved: false, trackCount: 11, albumCount: 1 };
    const subsonicProfile: LibraryEntity = { ...jellyfinProfile, id: 'artist:ref:subsonic:scoped-subsonic-id', provider: 'subsonic', providerId: 'scoped-subsonic-id' };
    const rawCollaboration: LibraryEntity = { ...artist, id: 'artist:metadata:["gillian welch & david rawlings"]', name: 'Gillian Welch & David Rawlings', provider: null, providerId: null, explicitlySaved: false, trackCount: 1, albumCount: 0 };
    const creditedDuet: LibraryEntity = { ...artist, id: 'artist:metadata:["gillian welch, alison krauss"]', name: 'Gillian Welch, Alison Krauss', provider: null, providerId: null, explicitlySaved: false, trackCount: 1, albumCount: 0 };
    const call = vi.fn(async () => ({ items: [metadataProfile, unresolvedDiscogs, catalogProfile, jellyfinProfile, subsonicProfile, rawCollaboration, creditedDuet], total: 7 }));
    const { target, props } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'artists' }, call }); await settle();
    expect(target.querySelectorAll('.artist-family')).toHaveLength(1);
    expect(target.querySelectorAll('.entity-card')).toHaveLength(3);
    const family = target.querySelector<HTMLElement>('.artist-family')!;
    expect(family.textContent).toContain('5 profiles');
    expect(family.textContent).toContain('4 sources');
    const profiles = family.querySelectorAll<HTMLButtonElement>('.version-choice');
    expect(profiles).toHaveLength(5);
    expect(profiles[0].textContent).toContain('Local library');
    expect(profiles[0].textContent).toContain('14 saved tracks');
    expect(profiles[1].textContent).toContain('identity unresolved');
    expect(profiles[2].textContent).toContain('4 albums');
    expect(profiles[3].getAttribute('aria-label')).toBe('Open Gillian Welch, Jellyfin, 11 tracks, 1 album');
    expect(profiles[4].getAttribute('aria-label')).toContain('Subsonic');
    expect(family.innerHTML).not.toContain('scoped-jellyfin-id');
    expect(family.innerHTML).not.toContain('scoped-subsonic-id');
    profiles[3].click();
    expect(props.onNavigate).toHaveBeenCalledWith(expect.objectContaining({ entity: jellyfinProfile }));
    expect(props.onNavigate.mock.calls[0][0].entity.id).toBe(jellyfinProfile.id);
    const separateCards = [...target.querySelectorAll('.entity-card')].filter(card => !card.classList.contains('artist-family'));
    expect(separateCards.some(card => card.textContent?.includes('Gillian Welch & David Rawlings'))).toBe(true);
    expect(separateCards.some(card => card.textContent?.includes('Gillian Welch, Alison Krauss'))).toBe(true);
  });

  it.each(['jellyfin', 'subsonic'])('loads the validated %s cover into the album hero through a track from the album', async (provider) => {
    const account = 'a'.repeat(64);
    const album = { ...albumVersion(`album:ref:${provider}:${account}.Y292ZXI`, 'Server Album', 'Artist', provider, '2021'), trackCount: 1 };
    const track = { ...saved, id: `${provider}:${account}.track`, provider, providerId: `${account}.track`, album: album.name, provenance: { serverAlbumId: 'cover' } };
    const cover = 'data:image/png;base64,iVBORw0KGgo=';
    const commandName = `get_${provider}_track_artwork`;
    const call = vi.fn(async (command: string) => command === commandName ? cover : { tracks: [track], total: 1 });
    const { target } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'albums', entity: album }, call });
    await vi.waitFor(() => expect(target.querySelector('[aria-label="Album overview"] .portrait img')?.getAttribute('src')).toBe(cover));
    expect(call).toHaveBeenCalledWith(commandName, { trackId: track.id });
  });

  it('lazily loads the exact Subsonic artist banner and the server version of a grouped album', async () => {
    const account = 'b'.repeat(64);
    const provider = 'subsonic';
    const artistProviderId = `${account}.YXJ0aXN0`;
    const albumProviderId = `${account}.YWxidW0`;
    const artistEntry: LibraryEntity = { ...artist, provider, providerId: artistProviderId, id: `artist:ref:${provider}:${artistProviderId}` };
    const serverAlbum: LibraryEntity = { ...albumVersion(`album:ref:${provider}:${albumProviderId}`, 'Fixture Album', artistEntry.name, provider, '2021'), providerId: albumProviderId };
    const otherVersion = albumVersion('album:musicbrainz:fixture', serverAlbum.name, serverAlbum.artist, 'musicbrainz', '2021');
    const track: Track = { ...saved, provider, providerId: `${account}.track`, id: `${provider}:${account}.track`, album: serverAlbum.name, albumArtist: artistEntry.name, provenance: { serverArtistId: 'artist', serverAlbumId: 'album' } };
    const unrelated: Track = { ...track, providerId: `${account}.unrelated`, id: `${provider}:${account}.unrelated`, albumArtist: 'Different album artist' };
    const cover = 'data:image/png;base64,iVBORw0KGgo=';
    const intersects: (() => void)[] = [];
    vi.stubGlobal('IntersectionObserver', class {
      constructor(callback: IntersectionObserverCallback) {
        intersects.push(() => callback([{ isIntersecting: true } as IntersectionObserverEntry], this as unknown as IntersectionObserver));
      }
      observe() {}
      disconnect() {}
    });
    const call = vi.fn(async (command: string, args?: Record<string, unknown>) => {
      if (command === 'list_library_entities') return { items: [otherVersion, serverAlbum], total: 2 };
      if (command === 'list_library_entity_tracks') return { tracks: args?.id === artistEntry.id ? [unrelated, track] : [track], total: 1 };
      if (command === 'get_subsonic_track_artwork' && args?.trackId === track.id) return cover;
      return null;
    });
    const { target } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'artists', entity: artistEntry }, call });
    await settle();
    expect(target.querySelector('.album-family')).not.toBeNull();
    expect(call.mock.calls.some(([command]) => command === 'list_library_entity_tracks')).toBe(false);
    for (const enter of intersects) enter();
    await vi.waitFor(() => {
      expect(target.querySelector('header .portrait img')?.getAttribute('src')).toBe(cover);
      expect(target.querySelector('.album-family .card-art img')?.getAttribute('src')).toBe(cover);
    });
    expect(call).toHaveBeenCalledWith('list_library_entity_tracks', expect.objectContaining({ id: artistEntry.id, limit: 8 }));
    expect(call).toHaveBeenCalledWith('list_library_entity_tracks', expect.objectContaining({ id: serverAlbum.id, limit: 1 }));
    expect(call).not.toHaveBeenCalledWith('get_subsonic_track_artwork', { trackId: unrelated.id });
  });

  it('sends explicit source, year, and sort filters to the album query', async () => {
    const call = vi.fn(async () => ({ items: [], total: 0 }));
    const { target } = render({ showCatalogProviderFilter: true, browserState: { ...initialLibraryBrowserState(), tab: 'albums', entitySource: 'musicbrainz', entityYear: '1998', entitySort: 'year' }, call }); await settle();
    expect(target.querySelector('[aria-label="Filter albums by source"]')).not.toBeNull();
    expect(target.querySelector('[aria-label="Filter albums by year"]')).not.toBeNull();
    expect(target.querySelector('[aria-label="Sort saved albums and artists"]')).not.toBeNull();
    expect(call).toHaveBeenLastCalledWith('list_library_entities', expect.objectContaining({ source: 'musicbrainz', year: '1998', sort: 'year', descending: false }));
  });

  it('hides the source controls without a server or the optional catalog filter', async () => {
    const call = vi.fn(async () => ({ items: [], total: 0 }));
    const { target } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'artists', entitySource: 'musicbrainz' }, call });
    await settle();
    expect(target.querySelector('.library-source-filter')).toBeNull();
    expect(call).toHaveBeenCalledWith('list_library_entities', expect.objectContaining({ source: null }));
  });

  it('plays only returned saved tracks and never requests a provider catalog', async () => {
    const call = vi.fn(async (_command: string, _args?: Record<string, unknown>) => ({ tracks: [saved], total: 1 }));
    const { target, props } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'artists', entity: artist, section: 'tracks' }, call });
    await settle();
    [...target.querySelectorAll<HTMLButtonElement>('button')].find(b => b.textContent === 'Play saved tracks')!.click();
    await settle();
    expect(props.onPlay).toHaveBeenCalledWith(saved, [saved], 0);
    expect(call.mock.calls.every(([command]) => command === 'list_library_entity_tracks')).toBe(true);
  });

  it('loads every matching page before queueing all saved tracks', async () => {
    const all = Array.from({ length: 401 }, (_, index): Track => ({ ...saved, id: `fixture:saved-${index}`, providerId: `saved-${index}`, title: `Saved song ${index}` }));
    const call = vi.fn(async (_command: string, args?: Record<string, unknown>) => {
      const offset = Number(args?.offset ?? 0);
      return { tracks: all.slice(offset, offset + 200), total: all.length };
    });
    const browserState = { ...initialLibraryBrowserState(), tab: 'artists' as const, entity: artist, section: 'tracks' as const, query: 'rita', sort: 'title', descending: true, availability: 'local', minDuration: '60', maxDuration: '300' };
    const { target, props } = render({ browserState, call }); await settle();
    expect(target.textContent).toContain('200 of 401 saved tracks loaded');
    [...target.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'Queue saved tracks')!.click();
    await settle();
    expect(props.onQueue).toHaveBeenCalledWith(all, 'append');
    expect(target.textContent).toContain('200 of 401 saved tracks loaded');
    expect([...target.querySelectorAll<HTMLButtonElement>('button')].some(button => button.textContent === 'Load more')).toBe(true);
    const actionPages = call.mock.calls.filter(([, args]) => args?.query === 'rita').slice(1);
    expect(actionPages.map(([, args]) => args?.offset)).toEqual([0, 200, 400]);
    expect(actionPages.every(([, args]) => args?.sort === 'title' && args?.descending === true && args?.availability === 'local' && args?.minDuration === 60 && args?.maxDuration === 300)).toBe(true);
  });

  it('plays every matching track across the 200-row page boundary', async () => {
    const all = Array.from({ length: 401 }, (_, index): Track => ({ ...saved, id: `fixture:play-${index}`, providerId: `play-${index}`, title: `Saved song ${index}` }));
    const call = vi.fn(async (_command: string, args?: Record<string, unknown>) => {
      const offset = Number(args?.offset ?? 0);
      return { tracks: all.slice(offset, offset + 200), total: all.length };
    });
    const { target, props } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'artists', entity: artist, section: 'tracks' }, call }); await settle();
    [...target.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'Play saved tracks')!.click(); await settle();
    expect(props.onPlay).toHaveBeenCalledWith(all[0], all, 0);
  });

  it('cancels the action instead of queueing a partial oversized result', async () => {
    const tracks = Array.from({ length: 200 }, (_, index): Track => ({ ...saved, id: `fixture:large-${index}`, providerId: `large-${index}` }));
    const call = vi.fn(async () => ({ tracks, total: 10001 }));
    const { target, props } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'artists', entity: artist, section: 'tracks' }, call }); await settle();
    const buttons = [...target.querySelectorAll<HTMLButtonElement>('button')];
    buttons.find(button => button.textContent === 'Queue saved tracks')!.click(); await settle();
    expect(target.querySelector('[role="alert"]')?.textContent).toContain('up to 10,000 matching tracks');
    expect(props.onQueue).not.toHaveBeenCalled();
  });

  it('cancels an in-flight all-tracks action when the library refreshes', async () => {
    let finishAction!: (page: { tracks: Track[]; total: number }) => void;
    let callCount = 0;
    const call = vi.fn(async () => {
      callCount++;
      if (callCount === 2) return await new Promise<{ tracks: Track[]; total: number }>(resolve => { finishAction = resolve; });
      return { tracks: [saved], total: 1 };
    });
    const { target, props } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'artists', entity: artist, section: 'tracks' }, call }); await settle();
    [...target.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'Queue saved tracks')!.click();
    await settle();
    expect(callCount).toBe(2);
    window.dispatchEvent(new Event('library-changed')); await settle();
    finishAction({ tracks: [saved], total: 1 }); await settle();
    expect(props.onQueue).not.toHaveBeenCalled();
  });

  it('uses compact playlist rows for artist-scoped saved tracks without reorder controls', async () => {
    const call = vi.fn(async () => ({ tracks: [saved], total: 1 }));
    const { target } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'artists', entity: artist, section: 'tracks' }, call, playingTrack: saved, bufferingTrackId: saved.id, playbackFailed: true });
    await settle();
    expect(target.querySelector('.track-list-viewport.playlist .shared-track-list')).not.toBeNull();
    const row = target.querySelector('.shared-track-list li[data-track-index]')!;
    expect(row.querySelector('.position-slot .track-position')?.textContent).toBe('1');
    expect(row.querySelector('.artwork')).not.toBeNull();
    expect(row.querySelector('.track-title')?.textContent).toContain('Saved song');
    expect(row.querySelector('.row-actions .more-trigger')).not.toBeNull();
    expect(row.classList.contains('failed')).toBe(true);
    expect(target.querySelectorAll('.playlist-reorder-handle, .playlist-move')).toHaveLength(0);
  });

  it('uses the same compact positions and actions for saved album tracklists', async () => {
    const album = { ...artist, id: 'album:ref:spotify:album', kind: 'album' as const, name: 'Album title' };
    const second: Track = { ...saved, id: 'fixture:saved-two', providerId: 'saved-two', title: 'Second song' };
    const call = vi.fn(async () => ({ tracks: [saved, second], total: 2 }));
    const { target } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'albums', entity: album }, call }); await settle();
    const list = target.querySelector('.track-list-viewport.playlist .shared-track-list')!;
    expect([...list.querySelectorAll('.position-slot .track-position')].map(node => node.textContent)).toEqual(['1', '2']);
    expect(list.querySelectorAll('.artwork')).toHaveLength(2);
    expect(list.querySelectorAll('.track-title')).toHaveLength(2);
    expect(list.querySelectorAll('.row-actions .more-trigger')).toHaveLength(2);
    expect(target.querySelectorAll('.playlist-reorder-handle, .playlist-move')).toHaveLength(0);
  });

  it('keeps a partial saved album detail actionable through the canonical unfiltered saved scope', async () => {
    const album: LibraryEntity = { ...albumVersion('album:ref:spotify:partial', 'Borrowed Light', 'Nora Vale', 'spotify', '2008'), trackCount: 2, totalTracks: 5 };
    const second: Track = { ...saved, id: 'fixture:saved-two', providerId: 'saved-two', title: 'Second saved song' };
    const call = vi.fn(async (_command: string, args?: Record<string, unknown>) => args?.query === 'missing'
      ? { tracks: [] as Track[], total: 0 }
      : { tracks: [saved, second], total: 2 });
    const onDownloadAlbum = vi.fn();
    const browserState = { ...initialLibraryBrowserState(), tab: 'albums' as const, entity: album, query: 'missing', sort: 'title', descending: true, availability: 'local', minDuration: '60', maxDuration: '300' };
    const { target } = render({ browserState, call, onDownloadAlbum }); await settle();

    expect(target.querySelector('.album-detail h1')?.textContent).toBe('Borrowed Light');
    expect(target.querySelector('.album-detail .artists')?.textContent).toBe('Nora Vale');
    expect(target.textContent).toContain('2 of 5 catalog tracks saved');
    expect(target.textContent).toContain('Play saved tracks');
    expect(target.textContent).toContain('Queue saved tracks');
    expect(target.textContent).toContain('No saved tracks match this filter');
    expect(target.querySelector('[aria-label="More album actions for Borrowed Light"]')).toBeNull();
    target.querySelector<HTMLElement>('[aria-label="About saved album tracks"]')!.click();
    expect(target.querySelector('.info-popover[open]')?.textContent).toContain('Album loudness analysis requires the complete album');
    expect([...target.querySelectorAll<HTMLButtonElement>('button')].some(button => button.textContent?.includes('Analyze album loudness'))).toBe(false);

    target.querySelector<HTMLElement>('[aria-label="Download album options for Borrowed Light"]')!.click();
    [...target.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'Download saved tracks')!.click();
    await settle();
    expect(onDownloadAlbum).toHaveBeenCalledWith([saved, second]);
    const actionRead = call.mock.calls.find(([, args]) => args?.query === '');
    expect(actionRead?.[1]).toMatchObject({ sort: 'sourceOrder', descending: false, availability: 'all', minDuration: null, maxDuration: null });
    expect(call.mock.calls.some(([, args]) => args?.query === 'missing' && args?.sort === 'title')).toBe(true);
  });

  it('rechecks exact saved album completeness before starting loudness analysis', async () => {
    const album: LibraryEntity = { ...albumVersion('album:ref:spotify:stale-complete', 'False Promise', 'Nora Vale', 'spotify', '2008'), trackCount: 3, totalTracks: 3 };
    const second: Track = { ...saved, id: 'fixture:saved-two', providerId: 'saved-two', title: 'Second saved song' };
    const call = vi.fn(async () => ({ tracks: [saved, second], total: 2 }));
    const onAnalyzeLoudness = vi.fn(async () => ({ jobId: 'should-not-start', playbackTracks: [saved, second] }));
    const { target } = render({ browserState: { ...initialLibraryBrowserState(), tab: 'albums', entity: album }, call, onAnalyzeLoudness }); await settle();
    target.querySelector<HTMLElement>('[aria-label="More album actions for False Promise"]')!.click();
    [...target.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.includes('Analyze album loudness'))!.click(); await settle();
    expect(onAnalyzeLoudness).not.toHaveBeenCalled();
    expect(target.querySelector('[role="alert"]')?.textContent).toContain('complete album tracklist is no longer saved');
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
