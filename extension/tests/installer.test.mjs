import test from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {mkdtempSync,readFileSync,realpathSync,rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createHash} from 'node:crypto';
test('Linux host fixture manifests restrict Firefox and Chromium identities', {skip:process.platform!=='linux'},()=>{
 const directory=mkdtempSync(join(tmpdir(),'burrata-hosts-'));
 try{
  execFileSync('python3',['scripts/install_browser_host.py','--executable','/bin/true','--browser','all','--output-dir',directory]);
  const firefox=JSON.parse(readFileSync(join(directory,'firefox/org.burrata.browser.json')));assert.deepEqual(firefox.allowed_extensions,['burrata-browser@burrata.local']);assert.equal(firefox.type,'stdio');
  const chromium=JSON.parse(readFileSync(join(directory,'chromium/org.burrata.browser.json')));
  const key=JSON.parse(readFileSync('extension/manifest.chromium.json')).key;
  const id=[...createHash('sha256').update(Buffer.from(key,'base64')).digest('hex').slice(0,32)].map(c=>String.fromCharCode(97+parseInt(c,16))).join('');
  assert.deepEqual(chromium.allowed_origins,[`chrome-extension://${id}/`]);assert.ok(readFileSync(firefox.path,'utf8').includes(`exec ${realpathSync('/bin/true')} --native-messaging`));
 }finally{rmSync(directory,{recursive:true,force:true});}
});
