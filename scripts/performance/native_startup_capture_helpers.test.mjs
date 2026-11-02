import assert from 'node:assert/strict';
import test from 'node:test';
import {idleIpcBucketsGate,parseIdleIpcCounter,reportedErrorEvidence,safePlaybackFailureSummary,safeStoreMutexMetrics,startupObservation,startupPauseGate} from './native_startup_capture_helpers.mjs';

const target='fixture-track';
const playing={trackId:target,generation:4,status:'playing',position:3.2,diagnostics:{firstAudioMs:275}};
const paused={trackId:target,generation:4,status:'paused',paused:true,position:3.25};

test('startup metric is bound to the requested track and active generation',()=>{
  assert.deepEqual(startupObservation(playing,target),{
    trackId:target,generation:4,firstAudioMs:275,position:3.2,status:'playing',
  });
  assert.equal(startupObservation({...playing,trackId:'different-track'},target),null);
});

test('startup requires first-audio metrics and at least three seconds of playback',()=>{
  assert.equal(startupObservation({...playing,diagnostics:{firstAudioMs:null}},target),null);
  assert.equal(startupObservation({...playing,position:2.99},target),null);
});

test('startup pause gate retains the same track and generation',()=>{
  const observation=startupObservation(playing,target);
  assert.equal(startupPauseGate(observation,paused,target),true);
  assert.equal(startupPauseGate(observation,{...paused,generation:5},target),false);
  assert.equal(startupPauseGate(observation,{...paused,trackId:'different-track'},target),false);
});

test('idle IPC buckets require exact fields, registration, monotonic counts, and all intervals',()=>{
  const initial=parseIdleIpcCounter('{"registered":1,"count":8}');
  assert.deepEqual(initial,{registered:1,count:8});
  assert.equal(parseIdleIpcCounter('{"registered":1,"count":"11"}'),null);
  assert.equal(parseIdleIpcCounter('{"registered":1,"count":11,"payload":"secret"}'),null);
  const warmups=Array.from({length:3},(_,index)=>({registered:1,count:9+index,windowMs:1000}));
  const measured=Array.from({length:20},(_,index)=>({registered:1,count:12+index,windowMs:1000,countDelta:1}));
  assert.equal(idleIpcBucketsGate(initial,warmups,measured,23000),true);
  assert.equal(idleIpcBucketsGate(initial,warmups.slice(1),measured,22000),false);
  assert.equal(idleIpcBucketsGate(initial,warmups,[...measured.slice(0,19),{...measured[19],count:1}],23000),false);
  assert.equal(idleIpcBucketsGate(initial,warmups,[...measured.slice(0,19),{...measured[19],registered:2}],23000),false);
  assert.equal(idleIpcBucketsGate({...initial,registered:0},warmups,measured,23000),false);
  assert.equal(idleIpcBucketsGate(initial,warmups,measured.slice(0,19),23000),false);
});

test('store mutex sanitizer preserves global baseline waits without inventing hold counters',()=>{
  const baseline=safeStoreMutexMetrics({diagnostics:{storeConnectionWait:{scope:'global_store_connection_mutex',
    lockCount:10,totalWaitNs:200,maxWaitNs:50,sqliteEngineBusyWait:'not_measured'}}});
  assert.deepEqual(baseline,{scope:'global_store_connection_mutex',lockCount:10,totalWaitNs:200,maxWaitNs:50,
    operations:null,holdMetricsAvailable:false,sqliteEngineBusyWait:'not_measured'});
  const final=safeStoreMutexMetrics({diagnostics:{storeConnectionWait:{scope:'global_store_connection_mutex',
    lockCount:2,totalWaitNs:3,maxWaitNs:2,operations:{'store.rs:21':{lockCount:2,totalWaitNs:3,maxWaitNs:2,totalHoldNs:5,maxHoldNs:4}}}}});
  assert.equal(final.holdMetricsAvailable,true);
  assert.equal(final.operations['store.rs:21'].totalHoldNs,5);
  assert.equal(safeStoreMutexMetrics({diagnostics:{storeConnectionWait:{scope:'other'}}}),null);
});

test('reported failure summary keeps only allowlisted category, outcome, and provider',()=>{
  const state={diagnostics:{recentTrackFailures:[
    {trackId:target,category:'source_identity_mismatch',outcome:'queue_exhausted',provider:'youtube',
      reason:'https://media.example/?token=test-fixture-credential-deadbeef00'},
  ]}};
  assert.deepEqual(safePlaybackFailureSummary(state,target),{
    category:'source_identity_mismatch',outcome:'queue_exhausted',provider:'youtube',
  });
  assert.deepEqual(safePlaybackFailureSummary({diagnostics:{recentTrackFailures:[
    {trackId:target,category:'unexpected',outcome:'unexpected',provider:'other',reason:'private'},
  ]}},target),{category:null,outcome:null,provider:null});
  assert.equal(safePlaybackFailureSummary({diagnostics:{recentTrackFailures:[]}},target),null);
});

test('reported error evidence drops arbitrary query, header, and response body text',()=>{
  const arbitrary=[
    'GET /resolve?token=test-fixture-credential-deadbeef00 HTTP/1.1',
    'Authorization: Bearer test-fixture-credential-deadbeef00',
    '{"requestHeaders":{"X-Account-Token":"test-fixture-credential-deadbeef00"},"body":"private response text"}',
  ];
  const state={playerMessage:arbitrary.join('\n'),diagnostics:{recentTrackFailures:[
    {trackId:target,category:'source_identity_mismatch',outcome:'queue_exhausted',provider:'youtube',reason:arbitrary.join('\n')},
  ]}};
  const evidence=reportedErrorEvidence(arbitrary.join('\n'),state,target);
  assert.deepEqual(evidence,{receiptFailed:true,failureEvidence:{
    category:'source_identity_mismatch',outcome:'queue_exhausted',provider:'youtube',
  }});
  const serialized=JSON.stringify(evidence);
  for(const value of arbitrary)assert.equal(serialized.includes(value),false);
});
