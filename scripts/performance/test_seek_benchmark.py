import unittest
from seek_benchmark import summarize


class SeekSummaryTests(unittest.TestCase):
    def test_positions_stay_separate_and_subprocess_cpu_is_not_reported_as_total(self):
        rows = [dict(codec=codec, strategy='retained', fraction=fraction,
                     firstPcmMs=value, decodeThreadCpuMs=1, wallMs=value + 1)
                for codec in ['mp3', 'opus'] for fraction in [.01, .95] for value in [2, 8]]
        summaries = summarize(rows)
        self.assertEqual(len(summaries), 4)
        for summary in summaries:
            self.assertEqual(summary['attempts'], 2)
            self.assertEqual(summary['firstPcmMs'], dict(median=5, p95=8, worst=8))
            self.assertEqual('decodeThreadCpuMs' in summary, summary['strategy'] == 'retained')

    def test_ffmpeg_comparison_reports_child_cpu_but_not_worker_cpu(self):
        summary = summarize([dict(codec='opus', strategy='ffmpegFresh', fraction=.5,
                                  firstPcmMs=12, decodeThreadCpuMs=1,
                                  decoderChildCpuMs=7, wallMs=13)])[0]
        self.assertNotIn('decodeThreadCpuMs', summary)
        self.assertEqual(summary['decoderChildCpuMs']['median'], 7)


if __name__ == '__main__':
    unittest.main()
