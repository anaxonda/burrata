import json
import tempfile
import unittest
from pathlib import Path

from run_isolated_memory import summarize_samples


class FrontendProfilerGroups(unittest.TestCase):
    def summarize(self, owner, with_vite=False):
        row = {"kind": "group", "group": "app", "cpuPercent": 2.0,
               "cpuSeconds": 0.02, "rssBytes": 1000, "readBytes": 10, "writeBytes": 20}
        rows = [row, {**row, "kind": "thread", "cpuPercent": 999.0}]
        if with_vite:
            rows.append({**row, "group": "vite", "rssBytes": 2000})
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder)
            samples = output / "samples.json"
            samples.write_text(json.dumps(rows))
            result = summarize_samples(samples, output, owner)
            self.assertEqual(result, json.loads((output / "summary.json").read_text()))
            return result

    def test_embedded_release_needs_only_app_samples(self):
        result = self.summarize("not-used")
        self.assertIsNone(result["viteServerProcessTree"])
        self.assertEqual(result["applicationProcessTree"]["intervalSamples"], 1)
        self.assertEqual(result["applicationProcessTree"]["cpuPercentOneLogicalCore"]["cpuSeconds"], 0.02)

    def test_development_keeps_separate_verified_vite_group(self):
        result = self.summarize("shared", with_vite=True)
        self.assertEqual(result["viteServerProcessTree"]["ownership"], "shared")
        self.assertEqual(result["viteServerProcessTree"]["processGroupRssBytes"]["peak"], 2000)
        self.assertEqual(result["applicationProcessTree"]["processGroupRssBytes"]["peak"], 1000)

    def test_missing_development_vite_samples_fail(self):
        with self.assertRaises(RuntimeError):
            self.summarize("shared")

    def test_release_does_not_silently_accept_vite_samples(self):
        with self.assertRaises(RuntimeError):
            self.summarize("not-used", with_vite=True)


if __name__ == "__main__":
    unittest.main()
