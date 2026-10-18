# To Miroloi Tis Manas investigation — 2026-10-18

Saved recording: Lakis Chalkias, “To Miroloi Tis Manas”, album “36 Tragoudia Tis Ipirou Me Ton Laki Chalkia”, Spotify `2Q9DpDilWFbnGzEbX5AIBS`, 127.626 seconds, no saved ISRC. No queue, tags, source pin or playback settings were changed during this read-only investigation.

The original [resolver trace](resolver-summary.json) proves automatic fallback was active and the source was not pinned. Monochrome, YouTube and SoundCloud were tried; the operation ended after 7.918 seconds without exhausting its deadline. Thus the generic manual-source instruction does not mean automatic fallback was disabled.

- Monochrome found `264536741330165760`, same title and 127.626-second duration, credited to “Lakis Halkias, Dimotiko Paradosiako Sygrotima Elatos”, catalog ISRC `USA371646567`. The artist gate rejected it.
- YouTube found [mYJTM8wN0Qc](https://www.youtube.com/watch?v=mYJTM8wN0Qc), same title, 128 seconds, uploaded by “Lakis Halkias - Topic”. Initial candidate gating allowed extraction; extraction and head/tail stream probes succeeded. The final fresh identity check failed despite matching provider ID and close duration. A separate metadata inspection confirms the same title/channel/duration and five audio formats. No full audio decode or in-app playback of this recording was performed in this investigation.
- SoundCloud found a 30-second preview under “Lakis Halkias”; it was correctly excluded as a duration mismatch.

[Wikidata](https://www.wikidata.org/wiki/Q12879973) explicitly lists Lakis Halkias as an alias of Lakis Chalkias. [Amazon’s album listing](https://music.amazon.in/albums/B00KWTUO8C) lists the requested song at 2:07; [the corresponding alternate-credit edition](https://music.amazon.co.uk/albums/B007D2WV6Y) lists it at 2:08. This provides a strong same-recording candidate, but an exact-master/ISRC cross-check is not established: two bounded Spotify metadata reads returned HTTP 429 with retry hints, and no further Spotify request was issued.

The finding is a spelling/credit identity false rejection across services, not absent automatic fallback and not proof the recording is unavailable. A reusable correction should bind artist aliases through verified identities or an explicitly reviewed source; do not globally rewrite “Chalkias” to “Halkias”, weaken version/duration checks, or hardcode this recording. Existing source-crosswalk support does not yet establish that binding for this imported row.

The banner’s “Choose another source” means manual review after automatic matching failed. It is misleading here because a plausible automatic candidate exists but was rejected. Prefer copy that states that no matching source could be verified from the automatic results, with manual source review as an option. Neither the alias integration nor that copy change is claimed implemented by this investigation.
