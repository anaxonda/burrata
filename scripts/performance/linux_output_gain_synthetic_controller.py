"""Synthetic-only controller for validating the output capture harness."""

import math
import os
from pathlib import Path
import subprocess
import tempfile
import wave


RATE = 48_000


def main() -> None:
    sink = os.environ["BURRATA_QUAL_SINK"]
    case = os.environ["BURRATA_QUAL_CASE"]
    if case not in {"off", "normalized"}:
        raise ValueError(f"unexpected case {case!r}")
    expected_gain_db = float(os.environ["BURRATA_QUAL_EXPECTED_GAIN_DB"])
    scale = 1.0 if case == "off" else 10 ** (expected_gain_db / 20)
    with tempfile.TemporaryDirectory(prefix="burrata-synthetic-controller-") as temporary:
        fixture = Path(temporary) / "synthetic-stereo.wav"
        with wave.open(str(fixture), "wb") as output:
            output.setnchannels(2)
            output.setsampwidth(2)
            output.setframerate(RATE)
            pcm = bytearray()
            for index in range(RATE * 2):
                if index < RATE // 10:
                    left = right = 0
                else:
                    t = index / RATE
                    left = round(0.32 * scale * math.sin(2 * math.pi * 440 * t) * 32767)
                    right = round(0.17 * scale * math.sin(2 * math.pi * 880 * t) * 32767)
                pcm.extend(left.to_bytes(2, "little", signed=True))
                pcm.extend(right.to_bytes(2, "little", signed=True))
            output.writeframes(pcm)
        env = os.environ.copy()
        env["PULSE_SINK"] = sink
        subprocess.run(
            [
                "paplay",
                f"--device={sink}",
                "--latency-msec=20",
                "--process-time-msec=10",
                str(fixture),
            ],
            env=env,
            check=True,
        )


if __name__ == "__main__":
    main()
