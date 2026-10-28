"""Offline analyzer tests; these do not exercise PulseAudio or an app build."""

import math
from pathlib import Path
import sys
import tempfile
import unittest
import wave

sys.path.insert(0, str(Path(__file__).resolve().parent))
import linux_output_gain_qualification as qualification


class QualificationAnalyzerTests(unittest.TestCase):
    def make_wav(self, path: Path, scale: float) -> None:
        with wave.open(str(path), "wb") as output:
            output.setnchannels(2)
            output.setsampwidth(2)
            output.setframerate(qualification.RATE)
            frames = bytearray()
            for index in range(qualification.RATE):
                left = round(0.3 * scale * math.sin(2 * math.pi * 440 * index / qualification.RATE) * 32767)
                right = round(0.12 * scale * math.sin(2 * math.pi * 880 * index / qualification.RATE) * 32767)
                frames.extend(left.to_bytes(2, "little", signed=True))
                frames.extend(right.to_bytes(2, "little", signed=True))
            output.writeframes(frames)

    def test_stereo_attenuation_metrics_capture_first_frame_and_peak(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            off_path = folder / "off.wav"
            down_path = folder / "minus6.wav"
            scale = 10 ** (-6 / 20)
            self.make_wav(off_path, 1.0)
            self.make_wav(down_path, scale)

            off = qualification.analyze_wav(off_path)
            down = qualification.analyze_wav(down_path)
            result = qualification.compare(off, down, expected_gain_db=-6.0)

            self.assertAlmostEqual(result["allMeanDb"]["left"], -6.0, delta=0.02)
            self.assertAlmostEqual(result["allMeanDb"]["right"], -6.0, delta=0.02)
            self.assertAlmostEqual(result["first100msMeanDb"]["left"], -6.0, delta=0.1)
            self.assertAlmostEqual(result["first100msMeanDb"]["right"], -6.0, delta=0.1)
            self.assertAlmostEqual(result["firstAudibleSampleDbDelta"]["left"], -6.0, delta=0.05)
            self.assertAlmostEqual(result["firstAudibleSampleDbDelta"]["right"], -6.0, delta=0.05)
            self.assertTrue(result["channelsRemainIndependent"])
            self.assertAlmostEqual(
                down["peakPerChannel"]["left"] / off["peakPerChannel"]["left"],
                scale,
                delta=0.0001,
            )
            self.assertAlmostEqual(
                down["firstAudibleFrame"]["left"] / off["firstAudibleFrame"]["left"],
                scale,
                delta=0.002,
            )

    def test_raw_capture_is_wrapped_without_losing_stereo_frames(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            raw = folder / "capture.s16le"
            output = folder / "capture.wav"
            raw.write_bytes(b"\x00\x20\x00\x10" * 100)

            qualification.raw_s16le_to_wav(raw, output)
            metrics = qualification.analyze_wav(output)

            self.assertEqual(metrics["frames"], 100)
            self.assertGreater(metrics["peakPerChannel"]["left"], metrics["peakPerChannel"]["right"])

    def test_periodic_multitone_alignment_refines_past_a_coarse_phase_alias(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            folder = Path(temporary)
            off_path = folder / "off.wav"
            normalized_path = folder / "normalized.wav"
            pre_roll = 1_925
            leading_silence = 300
            frames = qualification.RATE * 2
            gain = 10 ** (-7.14 / 20)

            def source_sample(index: int) -> int:
                time_seconds = index / qualification.RATE
                value = sum(
                    amplitude * math.sin(2 * math.pi * frequency * time_seconds)
                    for amplitude, frequency in ((0.14, 261.63), (0.11, 329.63), (0.08, 392.0))
                )
                return round(value * 32767)

            def write_capture(path: Path, sample_indices: range, scale: float, silence: int = 0) -> None:
                with wave.open(str(path), "wb") as output:
                    output.setnchannels(2)
                    output.setsampwidth(2)
                    output.setframerate(qualification.RATE)
                    pcm = bytearray(silence * 4)
                    for index in sample_indices:
                        sample = round(source_sample(index) * scale)
                        pcm.extend(sample.to_bytes(2, "little", signed=True) * 2)
                    output.writeframes(pcm)

            write_capture(off_path, range(frames), 1.0, leading_silence)
            write_capture(normalized_path, range(-pre_roll, frames), gain)

            off = qualification.analyze_wav(off_path)
            normalized = qualification.analyze_wav(normalized_path)
            result = qualification.compare(off, normalized, expected_gain_db=-7.14)
            expected_shift = pre_roll + off["firstAudibleFrameIndex"] - leading_silence - normalized["firstAudibleFrameIndex"]

            # This fixture's known pre-roll is 1,925 frames; the analyzer's
            # first-audible thresholds add a one-frame offset to the aligned
            # comparison. The old single-coarse-peak refinement chose a
            # periodic phase alias instead.
            self.assertEqual(expected_shift, 1_926)
            self.assertEqual(result["alignedSampleOffsetFrames"], expected_shift)
            self.assertAlmostEqual(result["allMeanDb"]["left"], -7.14, delta=0.02)
            self.assertAlmostEqual(result["allMeanDb"]["right"], -7.14, delta=0.02)
            self.assertAlmostEqual(result["first100msMeanDb"]["left"], -7.14, delta=0.1)
            self.assertAlmostEqual(result["first100msMeanDb"]["right"], -7.14, delta=0.1)


if __name__ == "__main__":
    unittest.main()
