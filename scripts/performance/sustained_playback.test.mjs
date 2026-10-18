import test from 'node:test';
import assert from 'node:assert/strict';
import {createSustainedPlaybackCheck} from './sustained_playback.mjs';
const frame=(position,extra={})=>({engine:'native',status:'playing',trackId:'fixture:audio',generation:3,seekSerial:1,position,
 candidate:{provider:'monochrome',id:'source-a'},diagnostics:{firstAudioMs:20,seekFirstAudioMs:10,operationId:'new',seekOperationId:'seek-new'},...extra});
test('playing state and a first mixer event cannot substitute for advancing audio',()=>{
 const check=createSustainedPlaybackCheck();let result;
 for(let ms=0;ms<=6000;ms+=250)result=check.observe(frame(1),ms);
 assert.equal(result.status,'pending');assert.equal(result.advancedMs,0);
});
test('requires five measured seconds and five seconds of stable mixer-derived progress',()=>{
 const check=createSustainedPlaybackCheck();let result;
 for(let ms=0;ms<5000;ms+=250){result=check.observe(frame(ms/1000),ms);assert.equal(result.status,'pending');}
 result=check.observe(frame(5),5000);assert.equal(result.status,'ready');assert.equal(result.advancedMs,5000);
});
test('five seconds of position jumping in less wall time fails',()=>{
 const check=createSustainedPlaybackCheck();check.observe(frame(0),0);
 assert.equal(check.observe(frame(5),250).failureCategory,'continued_playback_position_jump');
});
test('buffering interruption remains a failure after playback resumes',()=>{
 const check=createSustainedPlaybackCheck();check.observe(frame(0),0);
 assert.equal(check.observe(frame(.25,{status:'buffering'}),250).failureCategory,'continued_playback_interrupted');
 assert.equal(check.observe(frame(1),1000).status,'failed');
});
test('a generation, seek or source change cannot accumulate the old session progress',()=>{
 for(const extra of [{generation:4},{seekSerial:2},{candidate:{provider:'youtube',id:'other'}}]){
  const check=createSustainedPlaybackCheck();check.observe(frame(0),0);
  assert.equal(check.observe(frame(.25,extra),250).failureCategory,'continued_playback_session_changed');
 }
});
test('unobserved multi-second gap cannot prove uninterrupted consumption',()=>{
 const check=createSustainedPlaybackCheck();check.observe(frame(0),0);
 assert.equal(check.observe(frame(5),5000).failureCategory,'continued_playback_observation_gap');
});
test('WebView state and missing mixer diagnostics stay pending',()=>{
 for(const extra of [{engine:'webview'},{diagnostics:{firstAudioMs:null}},{diagnostics:{firstAudioMs:-1}}]){
  assert.equal(createSustainedPlaybackCheck().observe(frame(1,extra),0).status,'pending');
 }
});
test('a seek requires its own mixer event rather than the startup event',()=>{
 const check=createSustainedPlaybackCheck();
 assert.equal(check.observe(frame(1,{diagnostics:{firstAudioMs:20}}),0,'seekFirstAudioMs').status,'pending');
});

test('a buffering event between otherwise playing polls remains visible through counters',()=>{
 const check=createSustainedPlaybackCheck();
 check.observe(frame(0,{diagnostics:{firstAudioMs:20,operationId:'new',bufferingCount:0}}),0);
 assert.equal(check.observe(frame(.25,{diagnostics:{firstAudioMs:20,operationId:'new',bufferingCount:1}}),250).failureCategory,'continued_playback_interrupted');
});

test('held telemetry does not reset elapsed time for a batched mixer position report',()=>{
 // Native album exploration: last changed report at9749ms, held at9990ms,
 // then325.079ms consumed progress at10048ms. It is299ms since the changed
 // report, not58ms since the immediately preceding identical snapshot.
 const check=createSustainedPlaybackCheck();
 check.observe(frame(2.213560090702948),9749);
 check.observe(frame(2.213560090702948),9817);
 check.observe(frame(2.213560090702948),9874);
 check.observe(frame(2.213560090702948),9930);
 check.observe(frame(2.213560090702948),9990);
 const result=check.observe(frame(2.538639455782313),10048);
 assert.equal(result.status,'pending');
 assert.ok(Math.abs(result.advancedMs-325.079365079365)<.001);
 assert.equal(result.wallMs,299);
});
test('five consumed seconds reported in batches still require five measured wall seconds',()=>{
 const check=createSustainedPlaybackCheck();let result;
 for(let ms=0;ms<=5500;ms+=50){
   result=check.observe(frame(Math.floor(ms/300)*.3),ms);
   if(ms<5000)assert.equal(result.status,'pending');
 }
 assert.equal(result.status,'ready');
 assert.ok(result.wallMs>=5000);assert.ok(result.advancedMs>=5000);
});
test('held samples cannot conceal an unexplained large jump or backward jump',()=>{
 for(const position of [6,.5]) {
   const check=createSustainedPlaybackCheck();
   check.observe(frame(1),0);check.observe(frame(1),50);check.observe(frame(1),100);
   const result=check.observe(frame(position),150);
   assert.equal(result.failureCategory,'continued_playback_position_jump');
 }
});
test('held telemetry still enforces every poll gap and interruption counters',()=>{
 const gap=createSustainedPlaybackCheck();gap.observe(frame(1),0);gap.observe(frame(1),50);
 assert.equal(gap.observe(frame(1),2100).failureCategory,'continued_playback_observation_gap');
 for(const counter of ['bufferingCount','recoveryCount']) {
   const check=createSustainedPlaybackCheck();
   check.observe(frame(1,{diagnostics:{firstAudioMs:20,operationId:'new',[counter]:0}}),0);
   assert.equal(check.observe(frame(1,{diagnostics:{firstAudioMs:20,operationId:'new',[counter]:1}}),50).failureCategory,'continued_playback_interrupted');
 }
});
