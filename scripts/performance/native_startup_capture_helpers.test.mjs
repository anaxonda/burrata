import assert from 'node:assert/strict';
import test from 'node:test';
import {idleIpcBucketsGate,parseIdleIpcCounter,reportedErrorEvidence,reportedPcmPhaseComplete,safePlaybackFailureSummary,safeReportedLibraryRowLookup,safeReportedPlaybackDiagnosticExport,safeStartupDiagnosticExport,safeStartupMarkerMetrics,safeStoreMutexMetrics,startupObservation,startupPauseGate} from './native_startup_capture_helpers.mjs';

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

test('first-playing marker projection reads nested diagnostics and operation trace safely',()=>{
  const state={trackId:target,generation:4,status:'playing',id:'secret',diagnostics:{firstAudioMs:51,sourceReadyMs:5,
    operationKind:'startup',operationTrace:[{phase:'ipc_received',atMs:0},{phase:'first_audio',atMs:54}],
    private:'secret'}};
  assert.deepEqual(safeStartupMarkerMetrics(state,4),{
    generation:4,firstAudioMs:51,sourceReadyMs:5,operationFirstAudioMs:54,
  });
  assert.equal(safeStartupMarkerMetrics(state,5),null);
  assert.equal(safeStartupMarkerMetrics({...state,diagnostics:{...state.diagnostics,firstAudioMs:'secret'}},4).firstAudioMs,null);
});

test('startup pause gate retains the same track and generation',()=>{
  const observation=startupObservation(playing,target);
  assert.equal(startupPauseGate(observation,paused,target),true);
  assert.equal(startupPauseGate(observation,{...paused,generation:5},target),false);
  assert.equal(startupPauseGate(observation,{...paused,trackId:'different-track'},target),false);
});

test('reported PCM phase requires both five seconds elapsed and five seconds of playhead progress',()=>{
  assert.equal(reportedPcmPhaseComplete(4996.089,5),false);
  assert.equal(reportedPcmPhaseComplete(5000,5),true);
  assert.equal(reportedPcmPhaseComplete(6000,4.999),false);
  assert.equal(reportedPcmPhaseComplete(Infinity,5),false);
  assert.equal(reportedPcmPhaseComplete(NaN,5),false);
  assert.equal(reportedPcmPhaseComplete('5000',5),false);
  assert.equal(reportedPcmPhaseComplete(5000,Infinity),false);
  assert.equal(reportedPcmPhaseComplete(5000,NaN),false);
  assert.equal(reportedPcmPhaseComplete(5000,'5'),false);
});

test('opt-in startup diagnostics retain only bounded numeric and enum timing fields',()=>{
  const playingSnapshot={trackId:target,generation:4,status:'playing',candidate:{id:'private-id',url:'https://secret/?token=abc'},
    diagnostics:{operationId:'private-operation-id',operationKind:'startup',sourceReadyMs:125,firstAudioMs:178,
      operationTrace:[{phase:'ipc_received',atMs:0},{phase:'source_ready',atMs:122},{phase:'first_audio',atMs:176},
        {phase:'private-text',atMs:1}],decoder:{decoderKind:'rodio',readerSetupMs:11,decoderSetupMs:13,firstDecodedBlockMs:17,
        bytesRead:4096,bytesReceived:0,inputContentLength:2048,sampleRate:48000,channels:2,readerReused:false,decoderReused:true,
        codec:'private',codecHint:'private codec',path:'/private/file',url:'https://secret/?token=abc'},
      lastSourceFailure:{message:'private text'}}};
  const pausedSnapshot={...playingSnapshot,status:'paused',diagnostics:{...playingSnapshot.diagnostics,operationKind:'pause',
    operationTrace:[{phase:'ipc_received',atMs:0}]}};
  const diagnosticsReport={status:{enabled:true,eventCount:5,nativeEventCount:2,droppedEvents:0,droppedNativeEvents:0},
    events:[
      {resolutionId:'private-resolution',nativeOperationId:'private-operation-id',phase:'decoder_start',provider:'other',elapsedMs:127,
        atUnixMs:1_800_000_000_001,details:{generation:4}},
      {resolutionId:'private-resolution',phase:'decoder_end',provider:'other',elapsedMs:178,atUnixMs:1_800_000_000_052,
        details:{generation:4,durationMs:51,outcome:'success',category:'decoder',raw:'private body'}},
      {resolutionId:'private-resolution',phase:'decoder_end',provider:'other',elapsedMs:180,
        details:{generation:5,durationMs:52,outcome:'success',category:'decoder'}},
      {resolutionId:'private-resolution',phase:'private-event',provider:'other',elapsedMs:180,details:{generation:4}},
    ],
    nativePlayback:[
      {sequence:1,atUnixMs:1_800_000_000_001,elapsedMs:127,moment:'snapshot',playback:{status:'buffering',generation:4,sourceReadyMs:125,
        decoderKind:'rodio',decoderEvidence:'lastStartedSnapshot',decoderGenerationBinding:'notRecorded',trackId:'private-track'}},
      {sequence:2,atUnixMs:1_800_000_000_052,elapsedMs:178,moment:'snapshot',playback:{status:'playing',generation:4,firstAudioMs:178,
        operationId:'private-id',diagnostics:'private'}},
      {sequence:3,atUnixMs:456,elapsedMs:180,moment:'snapshot',playback:{status:'playing',generation:5,firstAudioMs:180}},
    ]};
  const result=safeStartupDiagnosticExport({playingSnapshot,pausedSnapshot,diagnosticsReport,targetGeneration:4});
  assert.equal(result.reportAvailable,true);
  assert.deepEqual(result.playingSnapshot,{generation:4,status:'playing',operationKind:'startup',sourceReadyMs:125,
    firstAudioMs:178,operationFirstAudioMs:176,operationTrace:[{phase:'ipc_received',atMs:0},
      {phase:'source_ready',atMs:122},{phase:'first_audio',atMs:176}],
    decoder:{readerSetupMs:11,decoderSetupMs:13,firstDecodedBlockMs:17,bytesRead:4096,bytesReceived:0,
      inputContentLength:2048,sampleRate:48000,channels:2,decoderReused:true,readerReused:false,decoderKind:'rodio'}});
  assert.equal(result.pausedSnapshot.operationKind,'pause');
  assert.deepEqual(result.decoderEvents,[
    {phase:'decoder_start',elapsedMs:127,generation:4,provider:'other',atUnixMs:1_800_000_000_001},
    {phase:'decoder_end',elapsedMs:178,generation:4,provider:'other',atUnixMs:1_800_000_000_052,durationMs:51,outcome:'success',category:'decoder'},
  ]);
  assert.deepEqual(result.nativePlaybackEvents,[
    {moment:'snapshot',elapsedMs:127,generation:4,status:'buffering',atUnixMs:1_800_000_000_001,sourceReadyMs:125,
      decoderKind:'rodio',decoderEvidence:'lastStartedSnapshot',decoderGenerationBinding:'notRecorded'},
    {moment:'snapshot',elapsedMs:178,generation:4,status:'playing',atUnixMs:1_800_000_000_052,firstAudioMs:178},
  ]);
  const serialized=JSON.stringify(result);
  for(const value of ['private-id','private-operation-id','private-resolution','private-track','https://secret','private body','private text','private codec','/private/file'])
    assert.equal(serialized.includes(value),false);
});

test('reported-track Library lookup keeps only the matching saved row identity and public metadata',()=>{
  const track={id:'spotify:fixture123',title:'Ámame',artist:'Maryto y su Salsón',durationSeconds:237.257};
  const result=safeReportedLibraryRowLookup(track,{total:2,items:[
    {id:'spotify:other',title:'Other',artist:'Maryto',provider:'spotify',providerId:'other'},
    {id:track.id,title:track.title,artist:track.artist,album:'Ámame',durationSeconds:237.257,
      provider:'spotify',providerId:'fixture123',sourceUrl:'https://private.invalid/?token=should-not-escape',provenance:{private:'value'}},
  ]});
  assert.equal(result.scope,'read-only saved Library page lookup before play_tracks');
  assert.equal(result.matched,true);
  assert.equal(result.titleMatches,true);
  assert.equal(result.artistMatches,true);
  assert.equal(result.durationMatches,true);
  assert.deepEqual(result.row,{id:track.id,provider:'spotify',providerId:'fixture123',title:'Ámame',artist:'Maryto y su Salsón',album:'Ámame',durationSeconds:237.257});
  assert.equal(JSON.stringify(result).includes('should-not-escape'),false);
  assert.equal(JSON.stringify(result).includes('private.invalid'),false);
});

test('reported playback diagnostics retain bounded provider phases and omit raw diagnostic data',()=>{
  const start=1_800_000_000_000;
  const result=safeReportedPlaybackDiagnosticExport({
    status:{enabled:true,eventCount:4,droppedEvents:0},
    events:[
      {resolutionId:'550e8400-e29b-41d4-a716-446655440000',atUnixMs:start,elapsedMs:10,phase:'provider_attempt',provider:'monochrome',
        details:{attempt:1,attemptPhase:'search',durationMs:10,retryable:true,category:'timeout',query:'private query',sourcePageUrl:'https://private.invalid'}},
      {atUnixMs:start+10,elapsedMs:15,phase:'provider_attempt',provider:'youtube',details:{attempt:2,outcome:'failed',raw:'private'}},
      {atUnixMs:start-100,elapsedMs:5,phase:'provider_attempt',provider:'youtube',details:{attempt:0}},
      {atUnixMs:start+20,elapsedMs:20,phase:'not a phase',provider:'youtube',details:{attempt:3}},
    ],
    nativePlayback:[{playback:{private:'must not be copied'}}],
  },start,start+15);
  assert.equal(result.enabled,true);
  assert.equal(result.events.length,2);
  assert.deepEqual(result.events[0],{atUnixMs:start,elapsedMs:10,phase:'provider_attempt',provider:'monochrome',
    resolutionId:'550e8400-e29b-41d4-a716-446655440000',details:{attempt:1,attemptPhase:'search',durationMs:10,retryable:true,category:'timeout'}});
  assert.deepEqual(result.events[1].details,{attempt:2,outcome:'failed'});
  assert.equal(JSON.stringify(result).includes('private'),false);
  assert.equal(JSON.stringify(result).includes('private.invalid'),false);
});

test('reported playback diagnostics preserve bounded candidate gate and full identity evidence',()=>{
  const start=1_800_000_000_000;
  const result=safeReportedPlaybackDiagnosticExport({status:{enabled:true,eventCount:3,droppedEvents:0},events:[
    {atUnixMs:start,elapsedMs:18,phase:'candidate_gate_evidence',provider:'youtube',details:{
      scope:'candidatesEvaluatedBeforeForegroundCap',candidateRowsLimit:200,candidateRowsTruncated:false,
      durationDeltaBasis:'request-vs-candidate-search-metadata; descriptive-only',
      counts:{candidatesEvaluatedBeforeForegroundCap:1,rejectedAllGates:1,permissionRestricted:0,'private key':'secret',providerId:'private'},
      private:'secret',
    }},
    {atUnixMs:start+1,elapsedMs:19,phase:'candidate_gate_evidence_rows',provider:'youtube',details:{
      firstOrdinal:1,totalCandidates:1,candidateRowsLimit:200,candidateRowsTruncated:false,rows:[{
        ordinal:1,providerClass:'youtube',confidenceClass:'review',permissionClass:'allowed',gate:'rejected',accepted:false,
        catalogDurationSeconds:237.25,candidateDurationSeconds:241.5,
        requestedTitlePhrasePresent:true,catalogTitleExact:false,canonicalArtistPhrasePresent:true,
        verifiedAliasArtistPhrasePresent:false,recordingCanonicalMatch:false,recordingVerifiedAliasMatch:false,
        topicCanonicalMatch:false,topicVerifiedAliasMatch:false,provisionalCanonicalMatch:false,
        provisionalVerifiedAliasMatch:false,descriptiveDurationWithinFiveSeconds:true,requestedVersionPresent:false,
        descriptiveVersionTextPresentOrNotRequired:true,versionConflictFlag:false,
        sharedAction:'inspect',sharedRelationship:'unresolved',
        sharedReasons:['artistConflict','extractorNeedsInspection','private reason'],
        sharedDimensions:{title:'agree',artists:'conflict',guests:'unknown',version:'agree',runtime:'agree',isrc:'unknown',work:'unknown',extra:'secret'},
        sourcePageUrl:'https://private.invalid/watch?token=secret',title:'Private title',artist:'Private artist',
        authorization:'Bearer nested-secret',candidateId:'private-source-id',
      }]}},
    {atUnixMs:start+2,elapsedMs:21,phase:'full_identity_check',provider:'youtube',details:{
      ok:false,proofKind:'none',providerIdMatches:true,resolvedTitlePresent:true,topicUploader:false,requestedVersion:true,
      catalogDuration:237.25,resolvedDuration:241.5,sourcePageUrl:'https://private.invalid/?token=secret',title:'Private title',
    }},
  ]},start,start+3);
  const [summary,rows,identity]=result.events.map(event=>event.details);
  assert.deepEqual(summary,{scope:'candidatesEvaluatedBeforeForegroundCap',counts:{
    candidatesEvaluatedBeforeForegroundCap:1,rejectedAllGates:1,permissionRestricted:0,
  },candidateRowsLimit:200,candidateRowsTruncated:false,durationDeltaBasis:'request-vs-candidate-search-metadata; descriptive-only'});
  assert.equal(rows.rows.length,1);
  assert.deepEqual(rows.rows[0],{
    ordinal:1,providerClass:'youtube',confidenceClass:'review',permissionClass:'allowed',gate:'rejected',accepted:false,
    catalogDurationSeconds:237.25,candidateDurationSeconds:241.5,
    requestedTitlePhrasePresent:true,catalogTitleExact:false,canonicalArtistPhrasePresent:true,
    verifiedAliasArtistPhrasePresent:false,recordingCanonicalMatch:false,recordingVerifiedAliasMatch:false,
    topicCanonicalMatch:false,topicVerifiedAliasMatch:false,provisionalCanonicalMatch:false,
    provisionalVerifiedAliasMatch:false,descriptiveDurationWithinFiveSeconds:true,requestedVersionPresent:false,
    descriptiveVersionTextPresentOrNotRequired:true,versionConflictFlag:false,
    sharedAction:'inspect',sharedRelationship:'unresolved',sharedReasons:['artistConflict','extractorNeedsInspection'],
    sharedDimensions:{title:'agree',artists:'conflict',guests:'unknown',version:'agree',runtime:'agree',isrc:'unknown',work:'unknown'},
  });
  assert.deepEqual(identity,{proofKind:'none',ok:false,providerIdMatches:true,resolvedTitlePresent:true,
    topicUploader:false,requestedVersion:true,catalogDuration:237.25,resolvedDuration:241.5});
  const encoded=JSON.stringify(result);
  for(const privateValue of ['private.invalid','token=secret','Private title','Private artist','nested-secret','private-source-id','private reason','authorization'])
    assert.equal(encoded.includes(privateValue),false,`unexpected diagnostic export value: ${privateValue}`);
});

test('reported candidate gate diagnostics reject unknown enums, nonfinite durations and oversized row arrays',()=>{
  const start=1_800_000_000_000;
  const validRow=ordinal=>({ordinal,providerClass:'youtube',confidenceClass:'high',permissionClass:'unknown',gate:'policy_inspect',accepted:false,
    catalogDurationSeconds:238,candidateDurationSeconds:239,sharedAction:'inspect',sharedRelationship:'probable',
    sharedReasons:['titleAgrees'],sharedDimensions:{title:'agree',artists:'unknown',guests:'unknown',version:'agree',runtime:'agree',isrc:'unknown',work:'unknown'}});
  const result=safeReportedPlaybackDiagnosticExport({status:{enabled:true},events:[
    {atUnixMs:start,elapsedMs:1,phase:'candidate_gate_evidence_rows',provider:'youtube',details:{
      firstOrdinal:1,totalCandidates:15,candidateRowsLimit:200,candidateRowsTruncated:true,
      rows:Array.from({length:15},(_,index)=>validRow(index+1)),
    }},
    {atUnixMs:start+1,elapsedMs:2,phase:'candidate_gate_evidence_rows',provider:'youtube',details:{
      rows:[
        {...validRow(1),catalogDurationSeconds:Infinity,candidateDurationSeconds:1e99,sharedAction:'authorization',
          sharedRelationship:'not-an-enum',sharedReasons:['artistConflict','private'],
          sharedDimensions:{title:'private',artists:'unknown',credentials:'secret'},nested:{token:'hidden'}},
        {...validRow(2),gate:'token=secret',providerClass:'secret-provider',confidenceClass:'private',permissionClass:'mystery'},
      ],
    }},
    {atUnixMs:start+2,elapsedMs:3,phase:'full_identity_check',provider:'youtube',details:{
      ok:true,proofKind:'private proof',catalogDuration:Infinity,resolvedDuration:-1,sourcePageUrl:'https://private.invalid',
    }},
  ]},start,start+3);
  const [rows,unsafeRows,identity]=result.events.map(event=>event.details);
  assert.equal(rows.rows.length,12);
  assert.equal(rows.rows.at(-1).ordinal,12);
  assert.equal(rows.rows[0].catalogDurationSeconds,238);
  assert.equal(rows.rows[0].sharedDimensions.artists,'unknown');
  assert.equal(unsafeRows.rows.length,1);
  assert.equal(unsafeRows.rows[0].ordinal,1);
  assert.equal('catalogDurationSeconds' in unsafeRows.rows[0],false);
  assert.equal('candidateDurationSeconds' in unsafeRows.rows[0],false);
  assert.equal('sharedAction' in unsafeRows.rows[0],false);
  assert.equal('sharedRelationship' in unsafeRows.rows[0],false);
  assert.deepEqual(unsafeRows.rows[0].sharedReasons,['artistConflict']);
  assert.deepEqual(unsafeRows.rows[0].sharedDimensions,{artists:'unknown'});
  assert.deepEqual(identity,{ok:true});
  const encoded=JSON.stringify(result);
  for(const secret of ['secret-provider','authorization','credentials','private proof','private.invalid','hidden','token=secret'])
    assert.equal(encoded.includes(secret),false,`unexpected adversarial value: ${secret}`);
});

test('resolved recording assessment keeps only bounded shared proof enums and counts',()=>{
  const start=1_800_000_000_000;
  const result=safeReportedPlaybackDiagnosticExport({status:{enabled:true},events:[
    {atUnixMs:start,elapsedMs:27,phase:'resolved_recording_assessment',provider:'youtube',details:{
      sharedAction:'inspect',sharedRelationship:'alternative',
      sharedDimensions:{title:'agree',artists:'conflict',guests:'unknown',version:'agree',runtime:'conflict',isrc:'unknown',work:'agree'},
      sharedReasons:['titleAgrees','artistConflict','durationLimitExceeded','durationLimitUnavailable','private reason'],
      alternativeKind:'performance',proofStrength:'recordingIdentifier',
      missingPerformerCount:2,extraPerformerCount:1,unresolvedAnnotationCount:4,
      title:'Private song',artist:'Private performer',query:'private query',sourcePageUrl:'https://private.invalid/?token=secret',
      authorization:'Bearer nested-secret',nested:{body:'secret body',header:'private header'},
    }},
    {atUnixMs:start+1,elapsedMs:28,phase:'resolved_recording_assessment',provider:'youtube',details:{
      sharedAction:'secret',sharedRelationship:'unresolved?',
      sharedDimensions:{title:'secret',artists:'unknown',extra:'credential'},
      sharedReasons:['artistConflict','unknownReason'],alternativeKind:'remix token=secret',
      proofStrength:'private-proof',missingPerformerCount:-1,extraPerformerCount:Infinity,
      unresolvedAnnotationCount:1_000_000,raw:{title:'private',authorization:'secret'},
    }},
  ]},start,start+2);
  assert.deepEqual(result.events[0].details,{
    sharedAction:'inspect',sharedRelationship:'alternative',
    sharedDimensions:{title:'agree',artists:'conflict',guests:'unknown',version:'agree',runtime:'conflict',isrc:'unknown',work:'agree'},
    sharedReasons:['titleAgrees','artistConflict','durationLimitExceeded','durationLimitUnavailable'],alternativeKind:'performance',
    proofStrength:'recordingIdentifier',missingPerformerCount:2,extraPerformerCount:1,unresolvedAnnotationCount:4,
  });
  assert.deepEqual(result.events[1].details,{sharedDimensions:{artists:'unknown'},sharedReasons:['artistConflict'],unresolvedAnnotationCount:256});
  const encoded=JSON.stringify(result);
  for(const secret of ['Private song','Private performer','private query','private.invalid','token=secret','nested-secret','secret body','private header','credential','private-proof','unknownReason'])
    assert.equal(encoded.includes(secret),false,`unexpected assessment detail: ${secret}`);
});

test('opt-in startup diagnostics reject unknown phases, wrong generations, and invalid timings',()=>{
  const result=safeStartupDiagnosticExport({targetGeneration:7,
    playingSnapshot:{generation:7,status:'playing',diagnostics:{sourceReadyMs:86_400_001,firstAudioMs:-1,
      operationKind:'raw text',operationTrace:[{phase:'first_audio',atMs:NaN},{phase:'private',atMs:2}],
      decoder:{decoderKind:'secret',decoderSetupMs:-1,firstDecodedBlockMs:1.5,bytesRead:1e20,bytesReceived:'secret',
        inputContentLength:128,sampleRate:48000,channels:100,decoderReused:'yes',readerReused:false,private:'secret'}}},
    diagnosticsReport:{events:[{phase:'decoder_end',atUnixMs:1_800_000_000_000,elapsedMs:4,details:{generation:8,durationMs:3}},
      {phase:'decoder_start',atUnixMs:'private timestamp',elapsedMs:4,details:{generation:7,durationMs:'private'}}]}});
  assert.deepEqual(result.playingSnapshot,{generation:7,status:'playing',operationKind:null,
    sourceReadyMs:null,firstAudioMs:null,operationFirstAudioMs:null,operationTrace:[],
    decoder:{inputContentLength:128,sampleRate:48000,readerReused:false}});
  assert.deepEqual(result.decoderEvents,[{phase:'decoder_start',elapsedMs:4,generation:7,provider:null}]);
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

 test('Spotify role diagnostics survive export without arbitrary provider text',()=>{
  const atUnixMs=1800000000000;
  const report=details=>({status:{enabled:true},events:[{atUnixMs,elapsedMs:5,phase:'spotify_track_role_context',provider:'spotify',details}]});
  const details={durationMs:5,available:false,roleStage:'account_status',roleOutcome:'operation_failed'};
  const good=safeReportedPlaybackDiagnosticExport(report({...details,rawError:'private-canary',query:'private-canary',url:'https://private.invalid'}),atUnixMs,atUnixMs);
  assert.deepEqual(good.events[0].details,details);
  assert.equal(JSON.stringify(good).includes('private'),false);
  const bad=safeReportedPlaybackDiagnosticExport(report({roleStage:'private-canary',roleOutcome:'private-canary',available:'private-canary'}),atUnixMs,atUnixMs);
  assert.deepEqual(bad.events[0].details,{});
 });
