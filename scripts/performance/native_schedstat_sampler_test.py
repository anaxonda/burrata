#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest

MODULE_PATH=Path(__file__).with_name('native_schedstat_sampler.py')
SPEC=importlib.util.spec_from_file_location('native_schedstat_sampler',MODULE_PATH)
SAMPLER=importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SAMPLER)


class NativeSchedstatSamplerTests(unittest.TestCase):
    def test_parses_only_three_nonnegative_decimal_counters(self):
        self.assertEqual(SAMPLER.parse_schedstat('120 30 4\n'),(120,30,4))
        for value in ('1 2', '1 2 3 4', '-1 0 1', '1 x 2', ''):
            self.assertIsNone(SAMPLER.parse_schedstat(value))

    def test_counter_delta_is_monotonic_and_reports_runtime_wait_and_slices(self):
        self.assertEqual(SAMPLER.schedstat_delta((10,20,3),(25,28,5)),
                         {'runningNs':15,'runqueueWaitNs':8,'timeslices':2})
        self.assertIsNone(SAMPLER.schedstat_delta((10,20,3),None))
        self.assertIsNone(SAMPLER.schedstat_delta((10,20,3),(9,20,3)))

    def test_interval_summarizes_role_counters_without_pid_or_thread_names(self):
        before={'monotonicNs':100,'tasks':[{'role':'burrata','processIndex':0,'threadIndex':0,
            'threadClass':'audio','runningNs':10,'runqueueWaitNs':20,'timeslices':1,'pid':123,'comm':'secret title'}]}
        after={'monotonicNs':200,'tasks':[{'role':'burrata','processIndex':0,'threadIndex':0,
            'threadClass':'audio','runningNs':40,'runqueueWaitNs':55,'timeslices':4,'pid':123,'comm':'secret title'}]}
        result=SAMPLER.summarize_interval([before,after],110,190)
        self.assertTrue(result['available'])
        self.assertEqual(result['roleTotals']['burrata'],
                         {'threadCount':1,'runningNs':30,'runqueueWaitNs':35,'timeslices':3})
        self.assertEqual(result['boundarySkewNs'],{'start':10,'end':10})
        serialized=str(result)
        self.assertNotIn('secret title',serialized)
        self.assertNotIn("'pid'",serialized)

    def test_interval_marks_disappearing_task_unavailable(self):
        before={'monotonicNs':100,'tasks':[{'role':'pipewire','processIndex':0,'threadIndex':1,
            'threadClass':'pipewire','runningNs':10,'runqueueWaitNs':20,'timeslices':1}]}
        after={'monotonicNs':200,'tasks':[]}
        result=SAMPLER.summarize_interval([before,after],110,190)
        self.assertFalse(result['threads'][0]['available'])
        self.assertEqual(result['threads'][0]['reason'],'task_missing_at_interval_boundary')

    def test_phase_intervals_use_only_valid_markers_and_native_trace_offsets(self):
        offset=2_000_000_000_000
        data={'markers':[{'phase':'control_started','monotonicNs':offset+1_000_000,'wallClockUnixMs':1_800_000_000_001,'clockPairSpanNs':100},
                         {'phase':'play_request','monotonicNs':offset+2_000_000,'wallClockUnixMs':1_800_000_000_002,'clockPairSpanNs':100},
                         {'phase':'first_playing_snapshot_observed','monotonicNs':offset+90_000_000,
                          'wallClockUnixMs':1_800_000_000_090,'clockPairSpanNs':100,
                          'metrics':{'generation':4,'firstAudioMs':80,'private':'secret'}},
                         {'phase':'diagnostics_report_received','monotonicNs':offset+3_000_000_000,
                          'wallClockUnixMs':1_800_000_003_000,'clockPairSpanNs':100}],
              'samples':[{'monotonicNs':100,'tasks':[]},{'monotonicNs':200,'tasks':[]}]}
        # Samples are aligned to the synthetic monotonic clock event range.
        data['samples']=[{'monotonicNs':offset+1_000_000,'tasks':[]},
                         {'monotonicNs':offset+2_000_000_000,'tasks':[]}]
        runtime={'startupDiagnosticExport':{'targetGeneration':4,'playingSnapshot':{'operationTrace':[
            {'phase':'source_ready','atMs':10},{'phase':'first_audio','atMs':80},
            {'phase':'bad','atMs':100000}]},
            'decoderEvents':[{'phase':'decoder_start','generation':4,'atUnixMs':1_800_000_000_010},
                             {'phase':'decoder_end','generation':4,'atUnixMs':1_800_000_000_060}],
            'nativePlaybackEvents':[{'moment':'snapshot','status':'playing','generation':4,'atUnixMs':1_800_000_000_090},
                                    {'moment':'snapshot','status':'playing','generation':5,'atUnixMs':1_800_000_000_080}]}}
        SAMPLER.enrich_phase_intervals(data,runtime)
        intervals=data['phaseIntervals']
        self.assertEqual(intervals['internalOperationTraceOffsetsMs'],
                         {'source_ready':10,'first_audio':80})
        self.assertFalse(intervals['internalOffsetsAlignedToNodeMarkers'])
        self.assertEqual(intervals['firstPlayingSnapshotMetrics'],{'generation':4,'firstAudioMs':80})
        joins=intervals['generationBoundEventJoins']
        self.assertEqual([row['name'] for row in joins],['decoder_start_to_end','decoder_end_to_first_playing_snapshot'])
        self.assertTrue(all(row['available'] for row in joins))
        self.assertIn('event-record timestamp',joins[0]['timestampSemantics'])
        self.assertNotIn('private',str(intervals))

    def test_wall_monotonic_calibration_jump_makes_join_unavailable(self):
        markers=[{'monotonicNs':2_000_000_000_000,'wallClockUnixMs':1_800_000_000_000,'clockPairSpanNs':100},
                 {'monotonicNs':2_000_020_000_000,'wallClockUnixMs':1_800_000_000_000,'clockPairSpanNs':100}]
        calibration=SAMPLER.calibrate_wall_monotonic(markers)
        self.assertFalse(calibration['withinBound'])
        self.assertEqual(calibration['reason'],'clock_join_uncertainty_exceeds_bound')

    def test_insufficient_clock_pairs_and_wrong_generation_do_not_join(self):
        calibration=SAMPLER.calibrate_wall_monotonic([{'monotonicNs':1,'wallClockUnixMs':1_800_000_000_000,'clockPairSpanNs':0}])
        self.assertFalse(calibration['withinBound'])
        data={'markers':[{'phase':'play_request','monotonicNs':2_000_000_000_000,
                          'wallClockUnixMs':1_800_000_000_000,'clockPairSpanNs':0},
                         {'phase':'first_playing_snapshot_observed','monotonicNs':2_050_000_000_000,
                          'wallClockUnixMs':1_800_000_000_050,'clockPairSpanNs':0}],
              'samples':[]}
        runtime={'startupDiagnosticExport':{'targetGeneration':3,'playingSnapshot':{'operationTrace':[]},
            'decoderEvents':[{'phase':'decoder_start','generation':2,'atUnixMs':1_800_000_000_010},
                             {'phase':'decoder_end','generation':2,'atUnixMs':1_800_000_000_020}],
            'nativePlaybackEvents':[]}}
        SAMPLER.enrich_phase_intervals(data,runtime)
        self.assertEqual(data['phaseIntervals']['generationBoundEventJoins'],[])

    def test_three_second_observation_gate_is_not_treated_as_first_playing(self):
        data={'markers':[{'phase':'play_request','monotonicNs':10},
                         {'phase':'startup_observation_gate','monotonicNs':3_000_000_010}],
              'samples':[]}
        SAMPLER.enrich_phase_intervals(data,{'startupDiagnosticExport':{}})
        self.assertFalse(data['phaseIntervals']['available'])
        self.assertEqual(data['phaseIntervals']['reason'],'playback_markers_missing')


if __name__=='__main__':
    unittest.main()
