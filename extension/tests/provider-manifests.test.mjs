import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';

test('both browser manifests grant optional access and parser injection on supported provider hosts',()=>{
  const expected=[
    'https://www.beatport.com/*','https://www.traxsource.com/*','https://open.spotify.com/*',
    'https://www.shazam.com/*','https://shazam.com/*','https://www.last.fm/*','https://last.fm/*',
    'https://bandcamp.com/*','https://*.bandcamp.com/*'
  ];
  for(const filename of ['manifest.json','manifest.chromium.json']){
    const manifest=JSON.parse(readFileSync(new URL(`../${filename}`,import.meta.url)));
    assert.equal(manifest.version,'0.1.8');
    assert.deepEqual(manifest.optional_host_permissions,expected);
    assert.deepEqual(manifest.web_accessible_resources[0].matches,expected);
  }
});
