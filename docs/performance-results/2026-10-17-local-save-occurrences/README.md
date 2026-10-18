# Local destination save wiring

Two App integration regressions use mocked provider responses and the actual local destination picker. Selecting the third occurrence before the first still appends them in their source order, preserving distinct remote-entry IDs and positions even when the canonical recording ID is identical. The cases cover a YouTube playlist and a Spotify album; no Spotify playlist workflow is exercised.

The local `append_playlist_entries` arguments preserve the two selected occurrence objects. Tests also assert no playback-start or remote-write command is issued. This proves UI-to-command wiring with a fake backend, not real destination persistence, provider sessions, partial/cancelled import behavior or writeback.

The integrated focused run passes all four new continuation/navigation/save tests (4 passed, 339 skipped), using one worker. The full 917-test result predates these four additions; no newer full-suite count is claimed.
