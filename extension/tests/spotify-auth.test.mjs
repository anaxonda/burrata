import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

const source=readFileSync(new URL('../spotify-auth.js',import.meta.url),'utf8');
const context=vm.createContext({crypto:globalThis.crypto,TextEncoder,BigInt,AbortController,setTimeout,clearTimeout});
vm.runInContext(source,context);
const auth=context.BurrataSpotifyAuth;

test('Meld TOTP secret registry selects the highest valid public version',()=>{
 const longSecret='A'.repeat(96);
 const registry={files:{'ignored.txt':{content:'not json'},'nuances.json':{content:JSON.stringify([{s:'bad',v:99},{s:'GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ',v:9},{s:longSecret,v:61}] )}}};
 assert.deepEqual(JSON.parse(JSON.stringify(auth.parseRegistry(registry))),{s:longSecret,v:61});
 assert.throws(()=>auth.parseRegistry({files:{x:{content:'[]'}}}),/no valid secret/);
 const meldCompatible={files:{'nuances.json':{content:JSON.stringify([{s:`${'GEZDGNBVGY3TQOJQ'.repeat(4)}-_09`,v:61}] )}}};
 assert.equal(auth.parseRegistry(meldCompatible).v,61);
});

test('Meld TOTP generation matches the RFC 6238 SHA-1 vector',async()=>{
 assert.equal(await auth.totp('GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ',59),'287082');
});

test('public TOTP registry cache avoids repeat network calls and validates stale cache fallback',async()=>{
 let calls=0,saved={};
 const storage={get:async key=>({[key]:saved[key]}),set:async value=>Object.assign(saved,value)};
 const fetcher=async()=>{calls++;const payload={files:{nuance:{content:JSON.stringify([{s:'GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ',v:8}])}}};return {ok:true,json:async()=>payload};};
 const first=await auth.loadRegistry(storage,fetcher,1000);const second=await auth.loadRegistry(storage,fetcher,2000);
 assert.equal(first.version,8);assert.equal(second.version,8);assert.equal(calls,1);
 saved.spotifyTotpNuance={...saved.spotifyTotpNuance,fetchedAt:0};
 const fallback=await auth.loadRegistry(storage,async()=>{throw new Error('offline');},30*60*60*1000);
 assert.equal(fallback.version,8);
});

test('public TOTP registry fetch has a bounded abort path',async()=>{
 const timeoutContext=vm.createContext({crypto:globalThis.crypto,TextEncoder,BigInt,AbortController,setTimeout:fn=>{queueMicrotask(fn);return 1;},clearTimeout(){}});
 vm.runInContext(source,timeoutContext);
 const storage={get:async()=>({}),set:async()=>{}};
 const fetcher=(_url,{signal})=>new Promise((_resolve,reject)=>signal.addEventListener('abort',()=>reject(new Error('aborted')),{once:true}));
 await assert.rejects(timeoutContext.BurrataSpotifyAuth.loadRegistry(storage,fetcher),/timed out/);
});
