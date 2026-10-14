# Linux desktop media controls

Burrata registers `org.mpris.MediaPlayer2.burrata` on the user's session D-Bus
while the desktop app is running, at `/org/mpris/MediaPlayer2`. No separate daemon,
API key, or setting is required. It exposes the MPRIS root and Player interfaces;
optional TrackList and Playlists interfaces are not advertised.

The same player handles GUI, MCP, and MPRIS requests. MPRIS reports playback state,
title, artist, album, cover URL, source URL, duration, position, volume, shuffle,
and repeat mode. Media controls support play/pause, stop, next/previous, seek,
absolute position, volume, shuffle, repeat, raise, and quit. State changes emit
PropertiesChanged and explicit seeks emit Seeked. Variable playback speed is not
supported (minimum and maximum rate are 1).

OpenUri accepts saved library file/source URLs and supported exact YouTube or
SoundCloud URLs. Unregistered arbitrary filesystem paths are rejected; import
local files through Burrata first. MPRIS controls are available to applications
in the same desktop session independently of the optional AI-control permissions.

Use `busctl --user introspect org.mpris.MediaPlayer2.burrata /org/mpris/MediaPlayer2`
to inspect the service, or an MPRIS-compatible desktop widget or playerctl to
control it. A missing session bus leaves the music app usable and records a
startup diagnostic rather than preventing launch. MPRIS is Linux-specific.

Validation (2026-10-12): the real desktop service was discovered and introspected
on the session bus. Its metadata matched Burrata's current track. Setting Volume,
Shuffle, and LoopStatus to their existing values completed through frontend
acknowledgements; a stale SetPosition was ignored. The 616-track queue, index 8,
and paused state were unchanged. Six focused MPRIS tests pass. Frontend tests
also verify paused Next/Previous preserve status without resolving or playing
additional audio. The frontend suite has 98 passing tests.

Protocol behavior follows the official [MPRIS Player specification](https://specifications.freedesktop.org/mpris/latest/Player_Interface.html).
