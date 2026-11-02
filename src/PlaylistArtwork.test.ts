// @vitest-environment jsdom
// @ts-expect-error Use the browser runtime in jsdom.
import { mount, tick, unmount } from '../node_modules/svelte/src/index-client.js';
import { afterEach, describe, expect, it, vi } from 'vitest';
// @ts-expect-error Use the browser runtime in jsdom.
vi.mock('svelte', async () => await import('../node_modules/svelte/src/index-client.js'));
import PlaylistArtwork from './PlaylistArtwork.svelte';
let destroy: (() => void) | undefined;
afterEach(() => { destroy?.(); document.body.innerHTML=''; });
async function settle() { await Promise.resolve(); await tick(); }
function render(props: Record<string,unknown>) {
  const target=document.createElement('div');document.body.append(target);
  const instance=mount(PlaylistArtwork,{target,props});destroy=()=>unmount(instance);
  return target;
}
describe('playlist artwork',()=>{
  it('prefers a provider cover and falls back to a four-cover collage if it fails',async()=>{
    const urls=[1,2,3,4].map(i=>`https://example.test/${i}.jpg`);
    const target=render({coverUrl:'https://example.test/playlist.jpg',urls});await settle();
    expect(target.querySelectorAll('img')).toHaveLength(1);
    expect(target.querySelector('img')?.getAttribute('src')).toContain('playlist.jpg');
    target.querySelector('img')!.dispatchEvent(new Event('error'));await settle();
    expect(target.querySelectorAll('.collage img')).toHaveLength(4);
    expect([...target.querySelectorAll('img')].every(img=>img.alt==='' && img.getAttribute('loading')==='lazy')).toBe(true);
  });
  it('uses one cover for fewer than four unique covers and a stable icon for missing or unsafe images',async()=>{
    const target=render({urls:['https://example.test/a.jpg','https://example.test/a.jpg','https://example.test/b.jpg','javascript:bad'],fallback:'▤'});await settle();
    expect(target.querySelectorAll('img')).toHaveLength(1);
    target.querySelector('img')!.dispatchEvent(new Event('error'));await settle();
    target.querySelector('img')!.dispatchEvent(new Event('error'));await settle();
    expect(target.querySelectorAll('img')).toHaveLength(0);
    expect(target.textContent).toContain('▤');
  });
});
