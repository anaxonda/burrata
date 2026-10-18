# Playing indicators across track-bearing views

The source call-site audit finds playing/buffering/paused state wired through every shared TrackList use: SearchPanel, ArtistPage sections, ReleaseView, DirectoryBrowser, RadioView history, App history, RemotePlaylistDetail and SpotifyArtist album/top tracks. The custom Library table uses the same recording identity helper. Queue parent rows use the current occurrence index, and chapter rows match exact playback ranges. No title-fuzzy comparison is used.

The existing TrackList tests cover current, paused and buffering announcements plus full recording/chapter range identity; the current full frontend suite passes858 tests across74 files. A read-only actual local-library search for the current Magalenha row returned one complete result on `dd9474d3fdf9`; root reviewed the app-owned capture showing the paused marker and highlighted matching row, with the paused footer state consistent. No playback was started. This runtime capture is one paused-search case, not a live matrix of every source or playing/buffering transition.

Evidence: `.cache/qualification-safe-format-cache-trace/normal-launch/playing-indicator-search-private.json` and `playing-indicator-search.png`. Full normal playback state was preserved across the preceding app handoff. Broader desktop/source acceptance remains tracked separately.
