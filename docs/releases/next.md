# Burrata 0.1.52

- Native playback builds a compressed audio buffer for supported finite streams. It can retry interrupted reads and refresh the same source while the decoder keeps consuming buffered audio.
- A song that has already started stays selected if recovery runs out. Retry and restored-queue playback use the normal foreground resolver, including the direct route for an exact YouTube upload.
- Transient failures preserve remembered sources. A stale mapping can be rediscovered within the startup deadline.
- Artist Tracks can search for a song in the artist's catalog, even with an empty initial page. Ordinary artist browsing uses available catalog identities. Monochrome artist pages remain available through explicit source selection.
- Artist and album links use their text height in track rows.

Finite HLS recovery supports complete segments with verified continuity. Live streams and unsupported representations use their existing playback paths.
