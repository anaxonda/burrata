# Sort availability across collected remote rows

Remote playlist and search availability now examines all collected rows, before display filters. Hiding the only row with a BPM or other optional value therefore no longer disables that sort choice. Displayed playback and queue snapshots still use the filtered visible rows.

Source capability declarations are deliberately operation-specific: the Last.fm top-tracks parser can explicitly label unsupported fields. Generic provider names do not establish absence. Incomplete datasets retain unknown fields while more pages may supply values; complete datasets use observed values. Existing sort defaults and source order are preserved.

After integration, 96 tests across the shared availability helper, RemotePlaylistDetail, SearchPanel and SpotifyArtist pass with one worker. A further 24 ArtistPage tests pass, for 120 focused tests across five files. Svelte checks also pass with zero errors and zero warnings. The production frontend build passes. Broader desktop screenshots and live pagination acceptance remain pending.

The eight-file integration has verified before/after hashes and source backups under `.cache/qualification-sort-capabilities/`. No playlist membership, source order or normal application data was changed by this verification.
