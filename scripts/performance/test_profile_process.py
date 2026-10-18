import unittest
from profile_process import group_row, interval_rows


class ProcessSamplingTests(unittest.TestCase):
    def test_process_and_thread_are_separate_and_reused_ids_are_ignored(self):
        before = {(10, 0): {"ticks": 100, "start": 1, "readBytes": 20},
                  (10, 10): {"ticks": 60, "start": 1},
                  (10, 11): {"ticks": 900, "start": 1}}
        after = {(10, 0): {"ticks": 150, "start": 1, "name": "app", "rss": 4096, "readBytes": 60},
                 (10, 10): {"ticks": 80, "start": 1, "name": "audio"},
                 (10, 11): {"ticks": 1000, "start": 2, "name": "reused"}}
        rows = interval_rows(before, after, 0.5, 1.5, 100)
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[0]["cpuPercent"], 100)
        self.assertEqual(rows[0]["readBytes"], 40)
        self.assertEqual(rows[1]["cpuPercent"], 40)
        self.assertNotIn("rssBytes", rows[1])

    def test_group_rollup_uses_process_rows_once(self):
        rows = [{"kind":"process", "cpuPercent":60, "rssBytes":100,
                 "readBytes":4, "writeBytes":8},
                {"kind":"thread", "cpuPercent":60, "rssBytes":100}]
        result = group_row(rows, 0.5, 1.0, "app", "attempt-1")
        self.assertEqual(result["cpuSeconds"], 0.3)
        self.assertEqual(result["rssBytes"], 100)
        self.assertEqual(result["readBytes"], 4)
        self.assertEqual(result["operationId"], "attempt-1")


if __name__ == "__main__":
    unittest.main()
