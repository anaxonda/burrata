import test from 'node:test';
import assert from 'node:assert/strict';
import {auditionQueueMetadataRetained} from './source_audition_gates.mjs';
const saved={id:'saved-track',provider:'catalog',providerId:'recording-1',title:'Saved Recording',artist:'Saved Artist',assetId:'download-1',localAvailability:'available'};
test('missing or wrapped metadata cannot pass a source-audition queue gate',()=>{
  assert.equal(auditionQueueMetadataRetained(undefined,undefined),false);
  assert.equal(auditionQueueMetadataRetained({},{}),false);
  assert.equal(auditionQueueMetadataRetained(saved,{track:saved}),false);
  for(const key of ['id','provider','providerId','title','artist']){
    const missing={...saved};delete missing[key];
    assert.equal(auditionQueueMetadataRetained(missing,missing),false);
  }
});
test('source audition requires retained meaningful recording and asset metadata',()=>{
  assert.equal(auditionQueueMetadataRetained({...saved},saved),true);
  assert.equal(auditionQueueMetadataRetained({...saved,title:'Changed'},saved),false);
  assert.equal(auditionQueueMetadataRetained({...saved,assetId:undefined},saved),false);
});
