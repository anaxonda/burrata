# Kaval Sviri alternate-source discovery

The user notes that obscure recordings may exist on YouTube under unusual names. Failure to qualify a source is not evidence of global unavailability. This bounded public-source investigation separates finding candidates from verifying the recording.

The target Spotify artist row is `64rvuYgKLuIHqacdZZiwPr`, “Kaval Sviri (The Flute Plays)” by Bulgarian State Television Female Choir, album Le Mystère des Voix Bulgares, Vol. 2, integer duration131 seconds. Artist-row position2 is not an album track number.

[Nonesuch’s official Vol. II page](https://www.nonesuch.com/albums/le-myst-re-des-voix-bulgares-vol-ii) lists Kaval Sviri as track1 at2:10 and credits the Bulgarian State Radio and Television Female Vocal Choir (selection79201). [MusicBrainz’s release](https://musicbrainz.org/release/b8254485-3b63-4e55-8776-7bd713fe3dd7) links the full subtitle, choir credit and2:11 duration to [recording5a1d6c4a-bd16-4d09-87da-6af8d4daf36c](https://musicbrainz.org/recording/5a1d6c4a-bd16-4d09-87da-6af8d4daf36c). These are target anchors, not yet a verified YouTube mapping. The1-second catalog display difference is not a recording-identity test.

Public searches found these candidates:

- [mor4vmCRIsE](https://www.youtube.com/watch?v=mor4vmCRIsE): full subtitle, Bulgarian State Television Female Choir Topic credit.
- [1fPn2lQktx8](https://www.youtube.com/watch?v=1fPn2lQktx8): short title, same choir Topic credit.
- [ujG02lWRAQg](https://www.youtube.com/watch?v=ujG02lWRAQg): short title, Le Mystère Des Voix Bulgares Topic credit.

Search snippets did not expose album, catalog, ISRC or recording identifiers for these videos. None has been seeded as a verified mapping or automatically accepted. MusicBrainz also lists a distinct2:14 Kaval sviri recording under the Le Mystère credit, so expanding an artist alias alone could select the wrong version. The useful next check is public art-track/distributor metadata connecting a candidate to this exact Vol. II recording. Query variants can improve discovery; a verified recording-to-source mapping must retain the evidence separately from transient stream URLs.

## Direct public metadata follow-up

A bounded direct metadata read for `mor4vmCRIsE` succeeds using the existing bundled helper (SHA25660606cef1a65d70f6639a85b45ebb7acf08dbb81831e134dd93939e23ca2b792) with workspace temporary storage. The initial helper attempt with default `/tmp` exits255; it is retained separately and does not establish provider unavailability. No audio is downloaded or played.

The direct result reports full track title, the volume-two album,1988 Nonesuch release information, Ensemble Trakia as choir, Marcel Cellier as producer and Peter Liomdev as arranger. Those recording-level credits corroborate the Nonesuch/Qobuz target, whereas its artist/channel are Le Mystère des Voix Bulgares rather than the Spotify display credit. Reported media duration132 seconds is a sanity check only. This is much stronger evidence than the public search snippets. The earlier “no candidate verified” result is superseded for this candidate's metadata investigation; actual application acceptance, decoding and correct playback remain unverified.

Sanitized metadata is retained in `.cache/qualification-kaval-public-metadata/mor4vmCRIsE-workspace-temp-sanitized.json`. The next task is to trace how this recording-scoped evidence can be represented in matching without accepting every recording under an artist alias or weakening unrelated identity/version checks.
