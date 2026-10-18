import test from 'node:test';
import assert from 'node:assert/strict';
import {validateSasaFixture} from './sasa_qualification_manifest.mjs';
const fixture=()=>({trackId:'local:control',durationSeconds:30,sasaCases:[
 {trackId:'fixture:sasa-clean',metadataShape:'separate_fields',expectedDurationSeconds:281},
 {trackId:'fixture:sasa-merged',metadataShape:'merged_title',expectedDurationSeconds:281}],
 remoteCase:{trackId:'spotify:catalog',expectedDurationSeconds:245.48}});
test('explicit fixture records both Sasa metadata shapes and independent control durations',()=>{
 assert.equal(validateSasaFixture(fixture()).sasaCases.length,2);
});
test('missing or duplicate shapes and unbounded identities cannot select arbitrary cases',()=>{
 for(const mutate of [f=>f.sasaCases.pop(),f=>f.sasaCases[1].metadataShape='separate_fields',
  f=>f.sasaCases[1].trackId=f.sasaCases[0].trackId,f=>f.sasaCases[0].expectedDurationSeconds=1,
  f=>f.trackId='https://example.test/',f=>f.remoteCase.expectedDurationSeconds=null]){
  const value=fixture();mutate(value);assert.throws(()=>validateSasaFixture(value),/invalid_sasa_fixture/);
 }
});
