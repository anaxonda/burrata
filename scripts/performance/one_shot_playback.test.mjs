import test from 'node:test';
import assert from 'node:assert/strict';
import {isVerifiedBandcampInput, qualifyPreparedStartup, runOneShotProtocol} from './one_shot_playback.mjs';
const expected={trackId:'fixture:ergo',expectedProvider:'bandcamp',expectedRecordingId:'bandcamp:292222166',expectedDurationSeconds:310.254};
const frame=(position,extra={})=>({engine:'native',status:'playing',trackId:expected.trackId,entryId:'occurrence-one',generation:2,seekSerial:0,position,duration:310.253333333,candidate:{provider:'bandcamp',id:expected.expectedRecordingId},diagnostics:{operationId:'one',sourceReadyMs:100,firstAudioMs:1793,bufferingCount:0,recoveryCount:0},...extra});
test('one-shot requires exact Bandcamp provider identity and independently supplied usable duration',()=>{
 assert.equal(isVerifiedBandcampInput('bandcamp','bandcamp:292222166',310.254),true);
 for(const args of [['youtube','bandcamp:292222166',310.254],['bandcamp','292222166',310.254],['bandcamp','bandcamp:0',310.254],['bandcamp','bandcamp:292222166',NaN],['bandcamp','bandcamp:292222166',100]])assert.equal(isVerifiedBandcampInput(...args),false);
});
async function observe(change=()=>({}),held=false){let time=0;return qualifyPreparedStartup(frame(1),{...expected,deadline:7000,now:()=>time,sleep:async()=>{time+=100;},state:async()=>frame(held?1:1+time/1000,change(time))});}
test('single prepared startup consumes five actual seconds without replay or seek',async()=>{
 const result=await observe();assert.equal(result.failureCategory,null);assert.equal(result.continuedPlayback.status,'ready');assert.equal(result.continuedPlayback.advancedMs,5000);assert.equal(result.continuedPlayback.wallMs,5000);
});
test('held position does not pass and terminal failure is retained',async()=>{
 assert.equal((await observe(()=>({}),true)).failureCategory,'continued_playback_timeout');
 assert.equal((await observe(time=>time>=100?{status:'failed'}:{})).failureCategory,'startup_failed');
});
test('same recording with different occurrence, source or interruption cannot qualify',async()=>{
 for(const change of [()=>({entryId:'different'}),()=>({candidate:{provider:'bandcamp',id:'bandcamp:2'}}),()=>({diagnostics:{operationId:'one',sourceReadyMs:100,firstAudioMs:1,bufferingCount:1,recoveryCount:0}})])assert.notEqual((await observe(change)).failureCategory,null);
});
test('actual one-shot protocol performs only late then backward seek and fails fast',async()=>{
 const calls=[];let result=await runOneShotProtocol(async()=>{calls.push('start');return{failureCategory:null};},async(fraction)=>{calls.push(fraction);return{failureCategory:null};});
 assert.deepEqual(calls,['start',.95,.01]);assert.equal(result.seeks.length,2);
 calls.length=0;result=await runOneShotProtocol(async()=>({failureCategory:'original_start_failure'}),async()=>{calls.push('unexpected');});assert.equal(result.start.failureCategory,'original_start_failure');assert.deepEqual(calls,[]);
 result=await runOneShotProtocol(async()=>({failureCategory:null}),async(fraction)=>{calls.push(fraction);return{failureCategory:'original_late_seek_failure'};});assert.deepEqual(calls,[.95]);assert.equal(result.seeks[0].failureCategory,'original_late_seek_failure');
});
