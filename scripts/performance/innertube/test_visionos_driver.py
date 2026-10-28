"""Offline subprocess fixtures; no provider, decoder or account access."""
import hashlib
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import visionos_driver as driver

FIXTURE = '''import json,os,sys,time
from pathlib import Path
request=json.loads(sys.stdin.readline())
base=Path(__file__).parent
with (base/'attempts.jsonl').open('a') as log:
 log.write(json.dumps({'pid':os.getpid(),'request':request,'home':os.environ.get('HOME'),
  'config':os.environ['XDG_CONFIG_HOME'],'cache':os.environ['XDG_CACHE_HOME'],
  'data':os.environ['XDG_DATA_HOME'],'tmp':os.environ['TMPDIR']})+'\\n')
duration=request['expectedMetadata']['durationSeconds']
full=request.get('fullRecording',False)
visitor=request.get('visitorMode')=='anonymous-source'
frames=int((duration if full else 5)*48000)
response={'id':request['id'],'ok':True,'qualification':False,'transportParity':False,
 'profileId':'VISIONOS_0_1','constructionVersion':'innertubex-93b-profile-meld-9c3-anonymous-visitor-v3' if visitor else 'innertubex-93b-profile-meld-9c3-defaults-v2',
 'tokenWork':'not-requested','playerJavaScript':'not-requested','formatId':251,'codec':'audio/webm; codecs="opus"',
 'playabilityStatus':'OK','playabilityCategory':'OK','requestedVideoId':request['videoId'],'returnedVideoId':request['videoId'],
 'identityMatchesRequestedId':True,'metadata':{'videoId':request['videoId'],'durationSeconds':duration,
  'titlePresent':True,'authorPresent':True,'matchesExpectedMetadata':True},
 'positionSeconds':duration*request.get('positionFraction',0),'fullRecording':full,'fullRecordingComplete':full,
 'expectedDecodeSeconds':duration if full else 5,'decoderLimitSeconds':duration+1 if full else 5,
 'decodedFrames':frames,'decodedAudioMs':frames/48,'mediaHeaderNames':[],'firstPcmFromCommandMs':100,
 'requestHeaderNames':['X-Goog-Visitor-Id'] if visitor else [],
 'visitorContext':'anonymous-source-service-worker; ephemeral-no-session-cache' if visitor else 'omitted-nullable-profile; no-session-fetch',
 'accountingComplete':True,'decoderCpuMs':10,'decoderMaxRssBytes':1000,'accountingWrapperCpuMs':2,
 'accountingWrapperMaxRssBytes':1000,'helperCpuMs':3,'helperRssBytes':1000,'helperMaxRssBytes':1000,
 'totalCpuMs':15,'stages':{'visitorServiceWorker':10,'player':20,'decode':70}}
BEHAVIOR
print(json.dumps(response),flush=True)
time.sleep(60)
'''


@unittest.skipUnless(os.name == 'posix', 'POSIX process-group driver')
class VisionosDriverTests(unittest.TestCase):
    def test_source_and_manifest_hashes_support_python310_without_file_digest(self):
        with tempfile.TemporaryDirectory(prefix='visionos-hashes-') as scratch:
            source = Path(scratch) / 'fixture-tool'
            manifest = Path(scratch) / 'tracks.json'
            payload = bytes(range(256)) * 8193
            source.write_bytes(payload)
            manifest.write_bytes(b'{"exact":"manifest bytes"}\n')
            with patch.object(hashlib, 'file_digest', create=True,
                              side_effect=AssertionError('file_digest requires Python 3.11')), \
                    patch.object(driver, 'SOURCE_FILES', [source]), \
                    patch.object(driver, 'ROOT', Path(scratch)):
                self.assertEqual(driver.hashes(manifest), {
                    source.name: hashlib.sha256(payload).hexdigest(),
                    manifest.name: hashlib.sha256(manifest.read_bytes()).hexdigest(),
                })

    def exercise(self, directory, behavior='pass', mode='durability', attempts=20, **kwargs):
        worker = directory / 'fixture.py'; worker.write_text(FIXTURE.replace('BEHAVIOR', behavior))
        manifest = directory / 'tracks.json'; manifest.write_bytes(driver.MANIFEST.read_bytes())
        output = directory / 'report.json'
        processes = []
        original = driver.terminate_group
        def cleanup(process):
            original(process, grace=0.005); processes.append(process)
        with patch.multiple(driver, NODE=Path(sys.executable), FFMPEG=Path(sys.executable), WORKER=worker,
                            SOURCE_FILES=[Path(driver.__file__), worker]), \
                patch.object(driver, 'terminate_group', side_effect=cleanup):
            result = driver.run(output, mode=mode, manifest=manifest, attempts=attempts,
                                visitor_context=True, **kwargs)
        self.assertTrue(all(process.poll() is not None for process in processes))
        self.assertTrue(all(row.get('cleanupCompleted') for row in result['requests'] if row['status'] != 'not_run'))
        self.assertEqual(json.loads(output.read_text()), result)
        return result, output

    def test_durability_has_nine_metadata_derived_seeks_and_three_full_recordings_in_fresh_processes(self):
        with tempfile.TemporaryDirectory() as scratch:
            base = Path(scratch); report, _ = self.exercise(base)
            log = [json.loads(line) for line in (base / 'attempts.jsonl').read_text().splitlines()]
            self.assertEqual(len(log), 12); self.assertEqual(len({row['pid'] for row in log}), 12)
            self.assertEqual([row['phase'] for row in report['requests']], ['seek-1','seek-50','seek-95','full'] * 3)
            self.assertEqual(report['eligibleRequests'], 12)
            self.assertFalse(report['qualification']); self.assertTrue(report['visitorContextEnabled'])
            for key in ('config','cache','data','tmp'):
                self.assertEqual(len({row[key] for row in log}), 12)
            for row in log:
                request = row['request']
                self.assertEqual(row['home'], os.environ.get('HOME'))
                self.assertEqual(request['visitorMode'], 'anonymous-source')
                self.assertEqual(request['formatId'], 251)
                self.assertEqual(request['expectedVideoId'], request['videoId'])
                self.assertLessEqual(request['budgetMs'], 19000)
                if request.get('fullRecording'):
                    self.assertNotIn('seconds', request)
                else:
                    self.assertEqual(request['seconds'], 5)

    def test_cold_default_is_twenty_fresh_rotated_calls_without_warmups(self):
        with tempfile.TemporaryDirectory() as scratch:
            report, _ = self.exercise(Path(scratch), mode='cold')
            rows = report['requests']
            self.assertEqual(len(rows), 20); self.assertEqual(report['eligibleRequests'], 20)
            self.assertEqual([r['track'] for r in rows[:9]],
                ['matthew','poso-lyric-video','timmy-topic','poso-lyric-video','timmy-topic','matthew',
                 'timmy-topic','matthew','poso-lyric-video'])
            self.assertTrue(all(row['phase'] == 'cold' for row in rows))

    def test_full_short_preview_wrong_identity_and_wrong_format_are_ineligible(self):
        behavior = '''if request['videoId']=='uZGRATckoBM':
 response.update(decodedFrames=60*48000,decodedAudioMs=60000)
elif request['videoId']=='hRNx674Z4QE': response['returnedVideoId']='AAAAAAAAAAA'
else: response['formatId']=140'''
        with tempfile.TemporaryDirectory() as scratch:
            report, _ = self.exercise(Path(scratch), behavior=behavior, mode='full')
            self.assertEqual(len(report['requests']), 3); self.assertEqual(report['eligibleRequests'], 0)
            self.assertTrue(all(row['ok'] for row in report['requests']))

    def test_safe_response_discards_signed_urls_visitors_raw_bodies_and_unknown_failure_text(self):
        behavior = '''response.update(url='https://private.example/?sig=secret',visitorData='secret-visitor',body='secret-body')
response['metadata']['title']='secret-title'
response['stages']['https://private.example']=123
response['failure']='https://private.example/secret'
response['requestHeaderNames'].append('private-secret-header')'''
        with tempfile.TemporaryDirectory() as scratch:
            report, _ = self.exercise(Path(scratch), behavior=behavior, mode='exploratory')
            text = json.dumps(report)
            self.assertNotIn('secret', text); self.assertNotIn('private.example', text)
            self.assertEqual(report['eligibleRequests'], 0)  # malformed header list cannot authorize v3

    def test_partial_and_oversized_protocol_frames_obey_deadline_and_cleanup(self):
        behavior = '''if request['videoId']=='uZGRATckoBM':
 sys.stdout.write('{"id":');sys.stdout.flush();time.sleep(60);sys.exit()
elif request['videoId']=='hRNx674Z4QE':
 sys.stdout.write('x'*70000+'\\n');sys.stdout.flush();time.sleep(60);sys.exit()
else: response['id']='wrong-id' '''
        with tempfile.TemporaryDirectory() as scratch:
            report, _ = self.exercise(Path(scratch), behavior=behavior, mode='exploratory', request_s=0.2)
            self.assertEqual([row['failure'] for row in report['requests']], ['DRIVER_DEADLINE','PROTOCOL_ERROR','PROTOCOL_ERROR'])
            self.assertEqual(report['eligibleRequests'], 0)

    def test_retained_output_is_never_overwritten_and_source_mutation_invalidates_all_rows(self):
        behavior = "Path(__file__).write_text(Path(__file__).read_text()+'\\n# source mutation')"
        with tempfile.TemporaryDirectory() as scratch:
            report, output = self.exercise(Path(scratch), behavior=behavior, mode='exploratory')
            self.assertEqual(report['status'], 'source_changed_during_run'); self.assertEqual(report['eligibleRequests'], 0)
            before = output.read_bytes()
            with self.assertRaisesRegex(ValueError, 'already exists'):
                driver.run(output, mode='cold')
            self.assertEqual(output.read_bytes(), before)

    def test_invalid_plan_and_outer_deadline_do_not_launch_extra_provider_calls(self):
        tracks = json.loads(driver.MANIFEST.read_text())
        with self.assertRaises(ValueError): driver.plan_requests(tracks, 'cold', 61)
        with self.assertRaises(ValueError): driver.plan_requests(tracks, 'unsupported')
        tracks[0]['durationSeconds'] = 601
        with self.assertRaises(ValueError): driver.plan_requests(tracks, 'full')
        with tempfile.TemporaryDirectory() as scratch:
            report, _ = self.exercise(Path(scratch), behavior='time.sleep(60)', mode='exploratory', outer_s=0.15, request_s=0.1)
            self.assertEqual(report['eligibleRequests'], 0)
            self.assertTrue(any(row['status'] == 'not_run' for row in report['requests']))

    def test_explicit_single_midpoint_repeat_is_one_new_call_without_automatic_retry(self):
        behavior = "response.update(ok=False,failure='NETWORK_FAILED',networkCause='UND_ERR_SOCKET',stage='player')"
        with tempfile.TemporaryDirectory() as scratch:
            base = Path(scratch)
            report, _ = self.exercise(base, behavior=behavior, mode='seek', only='matthew:seek-50')
            self.assertEqual(report['plannedRequests'], 1); self.assertEqual(len(report['requests']), 1)
            self.assertEqual(report['explicitSingleDiagnostic'], 'matthew:seek-50')
            self.assertEqual(report['eligibleRequests'], 0)
            self.assertEqual(report['requests'][0]['networkCause'], 'UND_ERR_SOCKET')
            log = [json.loads(line) for line in (base / 'attempts.jsonl').read_text().splitlines()]
            self.assertEqual(len(log), 1); self.assertEqual(log[0]['request']['positionFraction'], 0.5)
        raw = {'id': 'fixture', 'networkCause': 'https://private.example/secret',
               'visitorAttempts': [{'source':'service-worker','networkCause': ['secret']}]}
        self.assertNotIn('secret', json.dumps(driver.safe_response(json.dumps(raw).encode(), 'fixture')))


if __name__ == '__main__':
    unittest.main()
