# Restored WebView playback functional check

One existing paused test recording was resumed with app volume zero. The player reported a ready stream, its expected 181.4-second duration, and more than two seconds of playhead advancement. Pause state, position, volume and queue were restored. A subsequent numeric library read still reports 4,948 saved tracks, 15 assets and 36 playlists. The owned provider sign-in window was untouched.

This used the existing 472b58f8 native executable with the current Vite-served frontend. It does not qualify the newly compiled native changes, first mixer-consumed audio, physical audibility, startup p95, or other recordings/providers. No audio sample was captured. Scoped UI screenshots separately verified that a restored session says Ready instead of the false Unavailable label; they remain private temporary artifacts.
