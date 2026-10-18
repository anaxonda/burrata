import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';

test('both browser manifests grant optional access and parser injection on supported provider hosts',()=>{
  const expected=[
    'https://www.beatport.com/*','https://www.traxsource.com/*','https://open.spotify.com/*','https://www.youtube.com/*','https://music.youtube.com/*',
    'https://www.shazam.com/*','https://shazam.com/*','https://www.last.fm/*','https://last.fm/*',
    'https://bandcamp.com/*','https://*.bandcamp.com/*'
  ];
  for(const filename of ['manifest.json','manifest.chromium.json']){
    const manifest=JSON.parse(readFileSync(new URL(`../${filename}`,import.meta.url)));
    assert.match(manifest.version,/^\d+\.\d+\.\d+$/);
    assert.deepEqual(manifest.optional_host_permissions,[...expected.slice(0,5),'https://francescograzioso.github.io/Meld/*','https://api.github.com/gists/*',...expected.slice(5)]);
    assert.deepEqual(manifest.web_accessible_resources[0].matches,expected);
  }
  assert.deepEqual(JSON.parse(readFileSync(new URL('../manifest.json',import.meta.url))).background.scripts,['spotify-auth.js','background.js']);
  assert.equal(JSON.parse(readFileSync(new URL('../manifest.chromium.json',import.meta.url))).background.service_worker,'background.js');
});


test('browser toolbar and extension manager icons resolve to correctly sized PNG assets',()=>{
  for(const filename of ['manifest.json','manifest.chromium.json']){
    const manifest=JSON.parse(readFileSync(new URL(`../${filename}`,import.meta.url)));
    assert.deepEqual(manifest.action.default_icon,manifest.icons);
    assert.equal(manifest.action.default_title,'Burrata Browser Companion');
    for(const size of [16,32,48,128]){
      const png=readFileSync(new URL(`../${manifest.icons[size]}`,import.meta.url));
      assert.equal(png.subarray(1,4).toString(),'PNG');
      assert.equal(png.readUInt32BE(16),size);
      assert.equal(png.readUInt32BE(20),size);
    }
  }
});
