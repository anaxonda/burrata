# Cached seek range fixture — exploratory, 2026-10-17

An ignored native cached-reader/decoder/Rodio fixture passed three focused repetitions. With input available, each run decoded 442,274 samples in about 5.03s, with first PCM at 20ms. Holding input for 1.5s before the independent range origin induced the fixture’s range-starvation condition; each run then decoded 441,448 samples in about 6.18s and recovered decoded consumption for five seconds after input resumed. The reported injected-zero count was zero; the fixture WAV itself contains zero PCM.

This demonstrates only controlled reader/decoder behavior. It is not audible output, FLAC behavior, live network or Fruit playback, nor proof of a production fix. The ignored test was directly invoked three times under a 40-second child deadline (11.45–11.49s observed). [Sanitized results](result.json).
