# Open-login memory baseline

Three read-only snapshots one second apart select only the verified Burrata PID and its descendants. All five processes have readable PSS; median combined proportional memory is approximately 588.19 MiB. The GUI is the development executable 472b58f8 and retains an unconfirmed owned Google login window at its initial email step. No input, focus change, credentials or restart was performed.

This is a nonisolated open-login memory baseline, not idle-after-close, steady-playback, release-build or resource qualification. A separate compiler was active outside the selected process tree. CPU timing from this snapshot is not used. Compare with a matching post-confirmation/closure snapshot before claiming what resources native session handling releases. Raw local sampling is not published; the aggregate contains no credentials or account identity.
