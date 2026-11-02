"""Check that qualification gates reject plausible but incorrect audio captures."""
from pathlib import Path
from contextlib import closing
import tempfile
import json
import sqlite3
import unittest
import numpy as np
from native_runtime_capture import summarize_capture, assess_normalization, assess_source_audition_pcm, assess_full_recording_pcm, assess_stereo_markers, assess_audition_queue_checkpoint, load_reported_track_fixture, parse_streaming_fallback_sources, reported_track_case_gate


class ReportedTrackFixtureTests(unittest.TestCase):
    def track(self, **changes):
        result={'id':'catalog:fixture:1','provider':'musicbrainz','providerId':'fixture-recording',
            'sourceUrl':'https://musicbrainz.org/recording/fixture-recording','artist':'Fixture artist',
            'title':'Fixture track','favorite':False,'localPath':None,'localAvailability':'none',
            'assetId':None,'provenance':{'artistCredits':[{'name':'Fixture artist','role':'Artist'}]}}
        result.update(changes)
        return result

    def write_fixture(self, folder, track):
        path=Path(folder)/'tracks.json'
        path.write_text(json.dumps({'schemaVersion':1,'tracks':[track]}))
        return path

    def test_reported_tracks_are_copied_into_private_safe_fixture(self):
        with tempfile.TemporaryDirectory() as folder:
            source=self.write_fixture(folder,self.track(previewUrl='https://example.invalid/preview',artworkUrl='https://example.invalid/art'))
            destination=Path(folder)/'private/reported-tracks.json';destination.parent.mkdir()
            tracks,digest=load_reported_track_fixture(source,destination)
            saved=json.loads(destination.read_text())
            self.assertEqual(tracks[0]['id'],'catalog:fixture:1')
            self.assertIsNone(saved['tracks'][0]['previewUrl'])
            self.assertIsNone(saved['tracks'][0]['artworkUrl'])
            self.assertEqual(saved['tracks'][0]['localAvailability'],'none')
            self.assertEqual(len(digest),64)
            self.assertEqual(destination.stat().st_mode & 0o777,0o600)

    def test_reported_fixture_rejects_local_assets_tokens_and_unknown_fields(self):
        invalid=[self.track(localPath='/home/user/music.wav',assetId='local:1'),
                 self.track(provenance={'fixture':'access_token=test-fixture-credential-deadbeef00'}),
                 self.track(extraField='not a Track field'),
                 self.track(sourceUrl='file:///tmp/track'),
                 self.track(id='catalog/invalid/identifier'),
                 self.track(title='track://source')]
        with tempfile.TemporaryDirectory() as folder:
            destination=Path(folder)/'private.json'
            for track in invalid:
                source=self.write_fixture(folder,track)
                with self.subTest(track=track):
                    with self.assertRaises(ValueError):
                        load_reported_track_fixture(source,destination)

    def test_provider_fallback_list_is_bounded_to_unique_other_known_providers(self):
        self.assertEqual(parse_streaming_fallback_sources('monochrome,soundcloud','youtube'),['monochrome','soundcloud'])
        self.assertEqual(parse_streaming_fallback_sources('', 'youtube'),[])
        for value in ('youtube','monochrome,monochrome','bandcamp','monochrome,unknown'):
            with self.subTest(value=value):
                with self.assertRaises(ValueError):
                    parse_streaming_fallback_sources(value,'youtube')

    def test_reported_playback_requires_expected_provider_and_real_monitor_pcm(self):
        passed=reported_track_case_gate(True,5.0,True,'youtube','youtube',19000,20000,5100,6000)
        self.assertTrue(passed['passed'])
        self.assertFalse(reported_track_case_gate(True,5.0,True,'monochrome','youtube',19000,20000,5100,6000)['passed'])
        self.assertFalse(reported_track_case_gate(True,0,True,'youtube','youtube',19000,20000,5100,6000)['passed'])
        self.assertFalse(reported_track_case_gate(True,5.0,False,'youtube','youtube',19000,20000,5100,6000)['passed'])
        self.assertFalse(reported_track_case_gate(False,5.0,True,'youtube','youtube',19000,20000,5100,6000)['passed'])
        self.assertTrue(reported_track_case_gate(True,5.0,True,'monochrome',None,19000,20000,5100,6000)['passed'])
        self.assertFalse(reported_track_case_gate(True,5.0,True,None,None,19000,20000,5100,6000)['passed'])

    def test_reported_startup_and_pcm_phases_keep_separate_caps(self):
        self.assertTrue(reported_track_case_gate(True,4.5,True,'youtube','youtube',20000,20000,6000,6000)['passed'])
        self.assertFalse(reported_track_case_gate(True,4.5,True,'youtube','youtube',20001,20000,6000,6000)['passed'])
        self.assertFalse(reported_track_case_gate(True,4.5,True,'youtube','youtube',10000,20000,4999,6000)['passed'])
        self.assertFalse(reported_track_case_gate(True,4.5,True,'youtube','youtube',10000,20000,6251,6000)['passed'])


class CaptureGate(unittest.TestCase):
    def tone(self, left, right, seconds=12, active=10):
        t=np.arange(48000*seconds)/48000
        result=np.column_stack([.18*np.sin(2*np.pi*f*t) for f in (left,right)])
        result[int(active*48000):]=0
        return result

    def assess(self, pcm, mode='natural'):
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder)/'monitor.f32le'
            pcm.astype('<f4').tofile(path)
            return summarize_capture(path,mode)

    def test_complete_stereo_and_real_tail_pass(self):
        capture=np.concatenate([self.tone(440,660),self.tone(880,1100)])
        self.assertTrue(self.assess(capture)['passed'])

    def test_catalog_truncation_fails(self):
        capture=np.concatenate([self.tone(440,660)[:4*48000],self.tone(880,1100)])
        self.assertFalse(self.assess(capture)['passed'])

    def test_phantom_silent_tail_fails(self):
        capture=np.concatenate([self.tone(440,660),np.zeros((12*48000,2)),self.tone(880,1100)])
        self.assertFalse(self.assess(capture)['passed'])

    def test_final_phantom_tail_fails(self):
        capture=np.concatenate([self.tone(440,660),self.tone(880,1100),np.zeros((12*48000,2))])
        self.assertFalse(self.assess(capture)['passed'])

    def test_natural_requires_real_trailing_silence_not_repeated_tones(self):
        capture=np.concatenate([self.tone(440,660,active=12),self.tone(880,1100,active=12)])
        self.assertFalse(self.assess(capture)['passed'])
        duplicate=np.concatenate([self.tone(440,660),self.tone(440,660,active=12)])
        self.assertFalse(self.assess(duplicate,'duplicates')['passed'])

    def test_natural_rejects_unexpected_source_overlap(self):
        capture=np.concatenate([self.tone(440,660),self.tone(880,1100)])
        # Replay an outgoing-source burst during the incoming recording.
        capture[13*48000:14*48000]+=self.tone(440,660,seconds=1,active=1)
        self.assertFalse(self.assess(capture)['passed'])

    def test_seek_requires_the_real_silent_tail(self):
        valid=np.concatenate([self.tone(440,660,seconds=1,active=1),np.zeros((2*48000,2))])
        self.assertTrue(self.assess(valid,'seek')['passed'])
        audible=np.concatenate([self.tone(440,660,seconds=3,active=3),np.zeros((48000//2,2))])
        self.assertFalse(self.assess(audible,'seek')['passed'])
        unrelated=np.concatenate([self.tone(440,660,seconds=1,active=1),self.tone(220,330,seconds=2,active=2),np.zeros((48000//2,2))])
        self.assertFalse(self.assess(unrelated,'seek')['passed'])
        immediate=np.concatenate([self.tone(440,660,seconds=1,active=1),np.zeros((48000//2,2))])
        self.assertFalse(self.assess(immediate,'seek')['passed'])

    def crossfade_fixture(self):
        outgoing=self.tone(440,660,active=12)
        incoming=self.tone(880,1100,active=12)
        fade=np.linspace(0,1,2*48000)[:,None]
        outgoing[10*48000:]*=1-fade
        incoming[:2*48000]*=fade
        pcm=np.zeros((22*48000,2))
        pcm[:12*48000]+=outgoing
        pcm[10*48000:]+=incoming
        return pcm

    def test_crossfade_accepts_two_second_tail_overlap_and_real_pause(self):
        capture=self.crossfade_fixture()
        self.assertTrue(self.assess(capture,'crossfade')['passed'])
        paused=np.concatenate([capture[:11*48000],np.zeros((int(.8*48000),2)),capture[11*48000:]])
        self.assertTrue(self.assess(paused,'crossfade-pause')['passed'])

    def test_crossfade_rejects_simultaneous_or_excessive_overlap(self):
        simultaneous=self.tone(440,660,active=12)+self.tone(880,1100,active=12)
        self.assertFalse(self.assess(simultaneous,'crossfade')['passed'])
        excessive=np.zeros((18*48000,2))
        excessive[:12*48000]+=self.tone(440,660,active=12)
        excessive[6*48000:]+=self.tone(880,1100,active=12)
        self.assertFalse(self.assess(excessive,'crossfade')['passed'])

    def test_crossfade_rejects_a_phantom_silent_final_tail(self):
        capture=np.concatenate([self.crossfade_fixture(),np.zeros((12*48000,2))])
        self.assertFalse(self.assess(capture,'crossfade')['passed'])

    def test_swapped_channels_fail(self):
        capture=np.concatenate([self.tone(660,440),self.tone(1100,880)])
        self.assertFalse(self.assess(capture)['passed'])

    def test_silence_fails_despite_full_capture_length(self):
        self.assertFalse(self.assess(np.zeros((24*48000,2)))['passed'])

    def test_normalization_rejects_unchanged_audio(self):
        bins=[dict(firstLeft=.183,firstRight=.183,secondLeft=.183,secondRight=.183)]
        self.assertFalse(assess_normalization(bins,'track')['passed'])

    def test_normalization_rejects_one_unprocessed_channel(self):
        bins=[dict(firstLeft=.092,firstRight=.183,secondLeft=.046,secondRight=.046)]
        self.assertFalse(assess_normalization(bins,'track')['passed'])

    def test_album_normalization_accepts_equal_measured_attenuation(self):
        bins=[dict(firstLeft=.065,firstRight=.065,secondLeft=.065,secondRight=.065)]
        self.assertTrue(assess_normalization(bins,'album')['passed'])

    def test_priming_audio_cannot_replace_complete_main_playback(self):
        prefix=self.tone(440,660,seconds=3,active=3)
        main=np.concatenate([self.tone(440,660),self.tone(880,1100)])
        with tempfile.TemporaryDirectory() as folder:
            path=Path(folder)/'monitor.f32le'
            np.concatenate([prefix,main]).astype('<f4').tofile(path)
            self.assertFalse(summarize_capture(path,'natural')['passed'])
            self.assertTrue(summarize_capture(path,'natural',start_seconds=3)['passed'])

    def test_audition_checkpoint_requires_real_retained_local_path(self):
        with tempfile.TemporaryDirectory(prefix='burrata-launch.', dir='/tmp') as folder:
            profile = Path(folder)
            (profile / '.burrata-isolated-profile').write_text('burrata isolated memory run\n')
            (profile / 'fixture').mkdir()
            expected = str(profile / 'fixture/control.wav')
            fixture = profile / 'fixture/runtime-cases.json'
            fixture.write_text(json.dumps({'incidents': [{'id': 'saved-track', 'localPath': expected}]}))
            database = profile / 'data/org.burrata.discovery/burrata.sqlite'
            database.parent.mkdir(parents=True)
            with closing(sqlite3.connect(database)) as db:
                db.execute('CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT)')
                db.commit()
            for track, passed in [({}, False), ({'id': 'saved-track'}, False),
                                  ({'id': 'other', 'localPath': expected}, False),
                                  ({'id': 'saved-track', 'localPath': '/tmp/unowned.wav'}, False),
                                  ({'id': 'saved-track', 'localPath': expected}, True)]:
                with closing(sqlite3.connect(database)) as db:
                    db.execute('INSERT OR REPLACE INTO settings VALUES(?,?)',
                               ('nativePlaybackQueue.v1', json.dumps({'tracks': [track]})))
                    db.commit()
                self.assertEqual(assess_audition_queue_checkpoint(profile)['passed'], passed)
            fixture.write_text(json.dumps({'incidents': [{'id': 'saved-track'}]}))
            self.assertFalse(assess_audition_queue_checkpoint(profile)['passed'])

    def test_audition_rejects_relabelled_downloaded_audio(self):
        local=self.tone(440,660,seconds=2,active=2)
        self.assertFalse(assess_source_audition_pcm(local,self.tone(440,660))['passed'])
        self.assertFalse(assess_source_audition_pcm(local,self.tone(660,440))['passed'])

    def test_audition_requires_captured_control_and_distinct_audible_output(self):
        local=self.tone(440,660,seconds=2,active=2)
        self.assertTrue(assess_source_audition_pcm(local,self.tone(880,1100))['passed'])
        self.assertFalse(assess_source_audition_pcm(np.zeros_like(local),self.tone(880,1100))['passed'])
        self.assertFalse(assess_source_audition_pcm(local,np.zeros((48000*12,2)))['passed'])

    def full_recording_fixture(self):
        cases=[dict(title=str(index),maximumPosition=duration-.2,decodedDuration=duration,
                    startedAtUnixMs=start*1000,**({"endedAtUnixMs":end*1000} if index!=1 else {}))
               for index,(start,end,duration) in enumerate([(0,80,80),(80,120,40),(120,200,80)])]
        return np.full((48000*200,2),.1,dtype=np.float32),cases

    def test_full_capture_can_use_the_next_owned_occurrence_boundary(self):
        pcm,cases=self.full_recording_fixture()
        result=assess_full_recording_pcm(pcm,cases,0)
        self.assertTrue(all(item['passed'] for item in result))
        self.assertEqual(result[1]['endedAtUnixMs'],120000)
        self.assertEqual(result[1]['endBoundarySource'],'next occurrence first-playing timestamp')

    def test_other_tracks_cannot_hide_a_silent_recording(self):
        pcm,cases=self.full_recording_fixture();pcm[80*48000:120*48000]=0
        result=assess_full_recording_pcm(pcm,cases,0)
        self.assertFalse(result[1]['passed'])
        self.assertTrue(result[0]['passed'] and result[2]['passed'])

    def test_audio_without_near_complete_native_progress_is_not_a_complete_recording(self):
        pcm,cases=self.full_recording_fixture();cases[0]['maximumPosition']=60
        self.assertFalse(assess_full_recording_pcm(pcm,cases,0)[0]['passed'])

    def stereo_marker_fixture(self,lag=0):
        pcm=np.zeros((48000*12,2),dtype=np.float32);rng=np.random.default_rng(71)
        for second in [1,3,5,7,9]:
            pulse=rng.uniform(-.5,.5,3840).astype(np.float32);at=second*48000
            pcm[at:at+len(pulse),0]=pulse;pcm[at+lag:at+lag+len(pulse),1]=pulse
        return pcm

    def test_stereo_markers_require_sample_aligned_channels(self):
        self.assertTrue(assess_stereo_markers(self.stereo_marker_fixture(),5)['passed'])
        self.assertFalse(assess_stereo_markers(self.stereo_marker_fixture(7),5)['passed'])

    def test_stereo_alignment_requires_every_expected_marker(self):
        pcm=self.stereo_marker_fixture();pcm[9*48000:]=0
        self.assertFalse(assess_stereo_markers(pcm,5)['passed'])

    def test_incomplete_aac_recording_fails(self):
        self.assertFalse(self.assess(self.tone(440,660,seconds=6,active=6),'aac')['passed'])


if __name__=='__main__':unittest.main()
