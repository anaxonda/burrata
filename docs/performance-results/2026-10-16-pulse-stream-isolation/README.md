# Linux per-stream PCM isolation experiment

This is a functional PipeWire-Pulse experiment, not a playback performance or identification qualification.

Two disposable clients generated 440 Hz and 880 Hz tones into a temporary, explicitly selected null sink. The recorder used both the selected sink's monitor reference (`--device`) and the selected input index (`--monitor-stream`). It collected eight seconds of mono 48 kHz PCM16. The selected 440 Hz amplitude was 0.24997; the excluded 880 Hz amplitude was approximately 4.4e-16. This demonstrates stream isolation on this host. No physical output, microphone, Burrata playback or other application's audio was used. PCM stayed in memory and was discarded. Both clients and the recorder exited, and removal of the owned null sink was verified.

A preceding exact-input experiment at stream volume zero returned eight seconds of silence. The capture is therefore sensitive to stream volume on this host; silence must produce an actionable result rather than an identification request.

The initial cleanup check expected an `index` field in `pactl`'s JSON module list; that field was absent. Cleanup was completed separately using the test sink's verified `owner_module` and its unique generated module argument, then absence of the test sink was checked. The aggregate includes that completed cleanup verification.

The application still needs fail-closed stream ownership, fresh main/other-WebView audio-state checks, playback identity and cancellation guards, dependency detection, integration, and actual Burrata playback/recognition testing. This experiment does not qualify those paths, other Pulse implementations, macOS or Windows.
