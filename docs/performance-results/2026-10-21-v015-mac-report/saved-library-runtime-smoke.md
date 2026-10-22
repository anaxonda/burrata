# Saved library runtime smoke

The corrected one-attempt run observed at least five seconds of playback-position progress for ten saved tracks, with the effective matching mode read as Balanced before each playback request. Each success was tied to the accepted queue entry and requested library track. Two exact Afro House list items also passed: Mike Dunn and DJ Tai’s vocal remix, and Masaki Morii’s “Untitled Dub 1.” These results qualify short runtime progress observations; they do not establish physical audibility or acoustic recording identity.

| Saved track | Result |
| --- | --- |
| “Bacc Of The Bach” — Nipsey Hussle, Bino Rideaux | Progress observed |
| “Vibe With Me” — Nipsey Hussle, Bino Rideaux, Leon Thomas | Progress observed |
| “Sacrifices” — Nipsey Hussle, Bino Rideaux, James Fauntleroy II | Progress observed |
| “Nighttime in the Backyard” — Lucas Santtana & Arto Lindsay | Progress observed |
| “velib'bleu” — ฌон | Progress observed |
| “Diaraby (w/ Ry Cooder)” — Ali Farka Touré | Progress observed |
| “Matthew” — John Denver | Progress observed |
| “Baba” — Ralmm, Emil lassaria | Progress observed |
| “(I'm Sick) I've Been Infected Wit House Musick (Remix)” — Mike Dunn, DJ Tai | Progress observed |
| “Untitled Dub 1” — Masaki Morii | Progress observed |
| “To Miroloi Tis Manas” — Lakis Chalkias | Earlier run remained idle; follow-up progress observed (details below) |
| “Level 2 - Miami (Insane)” — Pengus | Skipped: the app reported Balanced while this case requires Flexible |

The initial harness report remains preserved in the private cache. It is an invalid harness result, not a track failure: the harness incorrectly required a playback-state generation counter that WebView snapshots do not provide. The corrected run used queue-entry and track identity for ownership and completed without retries. Its WebView source fields were null, so that run supports no source-ID comparison claim. Earlier source observations in the input manifest are historical context only.

## Miroloi follow-up

Frozen app `ed056432…` completed a later Balanced one-attempt smoke for “To Miroloi Tis Manas.” The accepted Monochrome source ID was `264536741330165760`; playback-position progress reached **5.243 seconds** within **22.655 seconds**. Before and after snapshots retain the same requested title, artist (`Lakis Chalkias`), album (`36 Tragoudia Tis Ipirou Me Ton Laki Chalkia`), version, ISRC, and 127.626-second duration. The saved metadata was unchanged. The source ID agrees with the prior persisted source observation, but position progress is not acoustic identity verification. Because this followed earlier attempts, the elapsed time is not a cold-start latency measurement. Earlier idle results remain historical, not a contradiction to this successful follow-up. See the [sanitized Miroloi record](miroloi-alias-cache-recheck.json); raw evidence remains private.

The third item from “Afro House — Chart Top 10 + Hidden Finds” remains unknown because it was not visible in the supplied screenshot.

## Queue failure and natural advancement

On the same frozen app, one Balanced queue submission contained “Level 2 - Miami (Insane)” by Pengus followed by “Bacc Of The Bach” by Nipsey Hussle and Bino Rideaux. The first occurrence was skipped with `recording_identity_unverified`; playback advanced to the exact second queue entry, which was playing at 27.496 seconds of a 198-second track with no media failure. A later app-owned capture showed the explicit “Skipped Level 2” notice and Bacc paused at its natural end, about 197/198 seconds. This verifies skip-then-advance for that queued pair, not the single-click library context or Mac behavior. [Sanitized runtime proof](failure-advance-runtime-proof.json).
