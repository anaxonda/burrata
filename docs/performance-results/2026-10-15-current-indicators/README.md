# Current playback / search indicator check

Development executable SHA-256:
`dc800d5d411b595ff7e35dcefcfdfce5495ce9a85c71bd4157efd182b77e3870`.

The main app was closed before launching the retained synthetic fixture profile.
The current executable passed the mounted-frontend handshake (79 tools). Through
MCP, muted native playback of the local WAV reached `playing`. A visible library
search for the fixture returned one row. The actual DOM inspector reported one
current row while playing, then one current and paused row after pausing. In the
inspector schema, `playing` counts the current recording indicator, including
paused recordings; `paused` separately identifies its paused state.

Both reports had zero player geometry findings at the existing window size.
See [actual app reports](actual-app.json). This verifies the search indicator for
one local recording, not buffering, every list, remote playback, or responsive
layouts. Earlier responsive evidence is in the expanded-player UI report.

The isolated fixture was closed before restoring the main profile. No main queue
operation, account confirmation, or remote playlist write was performed.
