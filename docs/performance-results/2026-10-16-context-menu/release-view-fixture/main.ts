import { mount } from 'svelte';
import ReleaseView from '../../../../src/ReleaseView.svelte';
import './style.css';
const release = { release: { provider: 'musicbrainz', id: 'fixture' }, title: 'A Portrait of Nikolai Kaufman', year: 1981, label: 'Balkanton', primaryType: 'Album', trackCount: 14, sourceUrl: 'https://musicbrainz.org/release/fixture', tracks: [], credits: [{ name: 'Bulgarian State Television Female Choir', role: 'artist', sourceUrl: 'https://musicbrainz.org/artist/fixture' }] };
const tracks = Array.from({length: 14}, (_, i) => ({ id: `fixture-${i}`, provider: 'musicbrainz', providerId: `fixture-${i}`, sourceUrl: `https://example.com/${i}`, title: ['Kaval Sviri','Polegnala e Todora','Pilentze Pee','Ergen Deda'][i % 4] + (i > 3 ? ` ${i + 1}` : ''), artist: 'Bulgarian State Television Female Choir', album: release.title, durationSeconds: 180 + i * 11, favorite: false, provenance: { position: String(i + 1) } }));
mount(ReleaseView, { target: document.querySelector('#app')!, props: { release, tracks, onPlay: () => {}, onQueue: () => {}, onDownloadAlbum: () => {}, onDownload: () => {}, onArtist: () => {}, onPlaylist: () => {}, onAddSelectedToPlaylist: () => {} } });
const params = new URLSearchParams(location.search);
if (params.has('focus')) document.querySelector<HTMLSelectElement>('[aria-label="Sort tracks"]')?.focus();
if (params.has('geometry')) {
  const card = document.querySelector<HTMLElement>('.release-view');
  const controls = [...document.querySelectorAll<HTMLElement>('.track-controls input,.track-controls select')];
  const report = {
    viewport: { width: innerWidth, height: innerHeight },
    document: { width: document.documentElement.scrollWidth, height: document.documentElement.scrollHeight, bodyScrollWidth: document.body.scrollWidth, horizontalOverflow: document.body.scrollWidth > innerWidth },
    card: card && { x: card.getBoundingClientRect().x, y: card.getBoundingClientRect().y, width: card.getBoundingClientRect().width, height: card.getBoundingClientRect().height, right: card.getBoundingClientRect().right, bottom: card.getBoundingClientRect().bottom, scrollWidth: card.scrollWidth, clientWidth: card.clientWidth, scrollHeight: card.scrollHeight, clientHeight: card.clientHeight, horizontalOverflow: card.scrollWidth > card.clientWidth },
    controls: controls.map(el => { const r = el.getBoundingClientRect(); return { label: el.getAttribute('aria-label'), x:r.x, y:r.y, width:r.width, height:r.height, right:r.right, bottom:r.bottom }; }),
    primary: (() => { const el = document.querySelector<HTMLElement>('.hero-actions .primary')!; const r=el.getBoundingClientRect(); return {x:r.x,y:r.y,width:r.width,height:r.height,right:r.right,bottom:r.bottom,color:getComputedStyle(el).color,background:getComputedStyle(el).backgroundColor}; })(),
    rows: [...document.querySelectorAll<HTMLElement>('.shared-track-list li')].length, focus: (() => { const el = document.querySelector<HTMLElement>('[aria-label="Sort tracks"]')!; return { focused:document.activeElement === el, visible:getComputedStyle(el).outlineStyle !== 'none', outlineWidth:getComputedStyle(el).outlineWidth }; })(),
  };
  const pre = document.createElement('pre'); pre.id='fixture-geometry'; pre.textContent=JSON.stringify(report); document.body.append(pre);
}
