# Live reserve pilot — exploratory check, 2026-10-17

On built candidate `51f9347cd3e6`, the strict control with reserve disabled passed its startup gate (first mixer at 2,320ms) and five-second sustained playback checks after 95% and 1% seeks. One reserve-enabled isolated pilot started its mixer at 2,026ms and released 180,224 samples against a 176,400-sample target, but failed the post-95%-seek sustained-playback gate with `continued_playback_interrupted`. The retained diagnostics reported a maximum observed seek of 6,422ms with 36,864 PCM samples produced and consumed.

This is n=1 per arm and does not establish causality, repeatability, audible correctness or a fix for live Fruit playback. The pilot remains default-off; the normal profile flag is disabled. No release or broad playback qualification is claimed. [Sanitized finite result](result.json).
