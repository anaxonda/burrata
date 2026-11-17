# Progress — November 17, 2026

Implemented fixes for all six reports below:

- [x] Artist overview: biography first, formatted provider references, top tracks next; genres, aliases, links, and provider coverage in collapsed Artist details. Duplicate genre/tag labels are combined with attribution preserved.
- [x] Radio Paradise duplicate prevention: advancing program countdowns suppress redundant fingerprint validation; stalled/missing metadata still permits recognition. Existing history is preserved.
- [x] Radio artist information: song context disambiguates same-name artists for biography lookup; changing songs refreshes the lookup. Live MusicBrainz confirmation was unavailable due to HTTP 503 responses.
- [x] Queue selection: Delete removes selected occurrences; Clear queue becomes Clear selection while rows are selected. The currently playing occurrence is retained.
- [x] Public Bandcamp fan playlists: direct import follows pagination without login; companion capture labels the initial page partial. The supplied URL returned 267 tracks across six public pages during verification.
- [x] Ask AI: hide internal citation handles while keeping returned public source links.

Top tracks on catalog artist pages come from Last.fm's artist.getTopTracks, queried by MusicBrainz artist ID where available. The default order is Last.fm popularity, not a MusicBrainz popularity ranking.

Verification: 119 focused frontend tests pass; type checking and the production frontend build pass. All extension test files pass (the installer fixture required an unsandboxed rerun because the sandbox blocked Node spawning Python). Visual checks pass at 1280px and 720px with no horizontal overflow. Native verification passes: six radio cases, four Bandcamp cases, the manual current-song identity case, and the live unauthenticated paginated Bandcamp import.

Original reports follow.

---

# Original reports
- musicbrainz bio formatting issues:
MusicBrainz identity
Beck
Save artist
alt rock, multi‐instrumentalist Beck Hansen
Open source profile ↗ Find on Spotify
Overview
Discography
Tracks
Collaborations
Local
Genres and tags by source
alternative dance
genre · MusicBrainz
alternative rock
genre · MusicBrainz
anti-folk
genre · MusicBrainz
chamber folk
genre · MusicBrainz
electronic
genre · MusicBrainz
experimental hip hop
genre · MusicBrainz
folk rock
genre · MusicBrainz
indie rock
genre · MusicBrainz
noise rock
genre · MusicBrainz
pop
genre · MusicBrainz
psychedelic pop
genre · MusicBrainz
rock
genre · MusicBrainz
singer-songwriter
genre · MusicBrainz
slacker rock
genre · MusicBrainz
trip hop
genre · MusicBrainz
alternative dance
tag · MusicBrainz
alternative rock
tag · MusicBrainz
anti-folk
tag · MusicBrainz
chamber folk
tag · MusicBrainz
electronic
tag · MusicBrainz
experimental hip hop
tag · MusicBrainz
folk rock
tag · MusicBrainz
indie rock
tag · MusicBrainz
lo-fi indie
tag · MusicBrainz
noise rock
tag · MusicBrainz
pity pop
tag · MusicBrainz
pop
tag · MusicBrainz
psychedelic pop
tag · MusicBrainz
rock
tag · MusicBrainz
singer-songwriter
tag · MusicBrainz
singer/songwriter
tag · MusicBrainz
slacker rock
tag · MusicBrainz
trip hop
tag · MusicBrainz
Aliases
Beck David Hansen · Beck Hansen · Bek David Campbell · Bjeck · Mr. Hansen
Provider coverage
MusicBrainz
available
Selected identity and source profile loaded.
Source ↗
Discogs
available
Explicitly linked Discogs artist data was loaded.
Source ↗
Last.fm
available
Last.fm profile details loaded.
Source ↗
Biography · Discogs
American musician, singer-songwriter and multi-instrumentalist. born July 8, 1970, in Los Angeles, California, USA. Son of [a=David Campbell] and [a=Bibbe Hansen], grandson of [a=Al Hansen]. He was married to [a=Marissa Ribisi] from 2004–2019.

When his parents split, he and his younger brother stayed with their mother. Beck dropped out of school at the age of 14 and drifted around for about a decade, ending up back in Los Angeles. While playing some open-mike events, he got noticed by the little local label Bong Load Custom Records. In 1993, "Loser" was released to the world in a limited pressing of 500 copies and sent out to radio DJs across the U.S. The song became quite popular, and subsequently DGC/Geffen/MCA signed Beck in November of 1993, while giving him the freedom to work with independent labels and to make uncommercial music. The DGC release of Loser in January 1994 became an international hit.

- duplication of radio paradise songs: "Select
1


Morning
Beck · Morning Phase · Via Radio Paradise · Mellow Mix
Nov 17, 2026, 9:35 AM MST

↓
⋯
2


Wait for Me
Moby · Wait for Me · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 6:05 PM MST

↓
⋯
3


Kothbiro
Ayub Ogada · En Mana Kuoyo · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 6:00 PM MST

↓
⋯
4


Right in Time
Lucinda Williams · Car Wheels on a Gravel Road · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:55 PM MST

↓
⋯
5


All the Kings Horses
Robert Plant · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:53 PM MST

↓
⋯
6


All The King's Horses
Robert Plant · Mighty Rearranger · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:51 PM MST

↓
⋯
7


Helpless
Neil Young · Harvest Moon · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:46 PM MST

↓
⋯
8
♫

Don't Look Back In Anger (Live)
Noel Gallagher · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:43 PM MST

↓
⋯
9


Don't Look Back in Anger
Noel Gallagher · The Very Best of MTV Unplugged · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:41 PM MST

↓
⋯
10


How To Be Invisible (2018 Remaster)
Kate Bush · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:37 PM MST

↓
⋯
11


How to Be Invisible
Kate Bush · Aerial · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:35 PM MST

↓
⋯
12


Sweet Surrender
Sarah McLachlan · Surfacing · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:32 PM MST

↓
⋯
13


Sky Blue Sky
Wilco · 2005-11-01: Auditorium Theatre, Chicago, IL, USA · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:28 PM MST

↓
⋯
14


(I've Got) Dreams to Remember
Toots Hibbert · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:26 PM MST

↓
⋯
15


(I've Got) Dreams To Remember
Toots & The Maytals · Toots In Memphis · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:25 PM MST

↓
⋯
16


The Greatest
Lana Del Rey · Norman Fucking Rockwell! · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:20 PM MST

↓
⋯
17


Close to My Fire (feat. Anna Leyne)
Slackwax · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:18 PM MST

↓
⋯
18


Close to my Fire
Slackwax · Hôtel Costes, Volume 15 · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:16 PM MST

↓
⋯
19


White Bird
KT Tunstall · Drastic Fantastic · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:13 PM MST

↓
⋯
20


Thin Air
Pearl Jam · Binaural · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:10 PM MST

↓
⋯
21


Angel From Montgomery
Bonnie Raitt · Streetlights · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:06 PM MST

↓
⋯
22


Rikki Don't Lose That Number
Steely Dan · Pretzel Logic · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 5:02 PM MST

↓
⋯
23


Isabelle Fly
The Lumineers · Isabelle Fly · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 4:58 PM MST

↓
⋯
24


Home
Xavier Rudd · Dark Shades Of Blue · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 4:53 PM MST

↓
⋯
25


Underwaterboys
Shriekback · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 4:50 PM MST

↓
⋯
26


The Underwaterboys
Shriekback · Big Night Music · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 4:48 PM MST

↓
⋯
27


The Girl From Ipanema
Stan Getz · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 4:47 PM MST

↓
⋯
28


The Girl From Ipanema
Stan Getz & Joao Gilberto · Getz/Gilberto · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 4:45 PM MST

↓
⋯
29


Good Vibrations
The Beach Boys · Via Radio Paradise · Mellow Mix
Nov 12, 2026, 4:43 PM MST

↓
⋯
30


Furnace Room Lullaby
Neko Case · Live From Austin TX · Via Radio Paradise · Mellow Mix"

- no song info for radio artist?

"About the artist

Beck
Artist information is not available for this recording.

Browse artist"

- multiple select in queue, del button should clear them and if a selection is present the 'clear queue' button should say clear selection
- https://bandcamp.com/funkentechno/playlist/ambient-reading we don't support importing bandcamp playlist of this type.
- also some formatting issue for 'ask ai': "
Song information

Ask AI
AI summary · web sources
Context for Snåcko — Jeremiah Chiu, Marta Sofia Honer
“Snåcko” is a 4:47 instrumental track—not a concert or DJ set—by Jeremiah Chiu and Marta Sofia Honer. It appears third on their album Recordings from the Åland Islands, released March 11, 2022. The album’s official label page credits both artists with performing and producing it. citeturn1search1

The track takes its name from an island near Kumlinge, and the artists describe it as shaped by field recordings made while exploring the Åland Islands. Pitchfork notes its birdsong and forest ambience alongside Honer’s layered viola and Chiu’s synthesizer. citeturn1search1turn1search0"
