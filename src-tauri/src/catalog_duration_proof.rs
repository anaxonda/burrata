//! Recording-scoped independent catalog evidence; never library tag updates.
use super::*;
const CACHE: &str = "playback.catalog-duration.v1";
fn trace_stage(stage: &str, started: std::time::Instant, details: Value) {
    if std::env::var("BURRATA_PLAYBACK_TRACE").is_ok_and(|value| value == "1") {
        eprintln!(
            "burrata_catalog_duration {}",
            json!({"stage":stage,"elapsedMs":started.elapsed().as_millis(),"details":details})
        );
    }
}
#[derive(Debug, Clone)]
pub(crate) struct PreparedCatalogDuration {
    pub track: Track,
    pub original_fingerprint: Value,
    pub verified_artist_variants: Vec<String>,
}
fn identity(track: &Track) -> Value {
    alias_hydration_identity(track)
}
fn key(track: &Track) -> String {
    format!(
        "{:x}",
        Sha256::digest(identity(track).to_string().as_bytes())
    )
}
fn eligible(track: &Track) -> bool {
    track.provider == "radio_song"
        && track.duration_seconds.is_none()
        && track.playback_range.is_none()
        && track.local_path.is_none()
        && track.asset_id.is_none()
        && track.provenance["chapters"]
            .as_array()
            .is_none_or(|chapters| chapters.is_empty())
        && !track.title.trim().is_empty()
        && !track.artist.trim().is_empty()
        && track.provenance["manualFields"]["durationSeconds"] != true
}
fn isrc(value: &str) -> Option<String> {
    crate::recording_identity::normalize_isrc(value)
}
fn full_credits(source: &str, refs: &[Value]) -> bool {
    if refs.is_empty() || refs.len() > 8 {
        return false;
    }
    let names = refs
        .iter()
        .filter_map(|r| {
            (r["provider"] == "spotify" && r["id"].as_str().is_some_and(valid_spotify_id))
                .then(|| r["name"].as_str().map(crate::matching::normalize))
                .flatten()
        })
        .collect::<Vec<_>>();
    if refs
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect::<HashSet<_>>()
        .len()
        != refs.len()
    {
        return false;
    }
    if names.len() != refs.len() || names.iter().any(String::is_empty) {
        return false;
    }
    fn consume(rest: &str, names: &[String], used: u16) -> bool {
        if used.count_ones() as usize == names.len() {
            return rest.trim().is_empty();
        }
        for (i, name) in names.iter().enumerate() {
            if used & (1 << i) != 0 {
                continue;
            }
            let Some(tail) = rest.strip_prefix(name) else {
                continue;
            };
            if !tail.is_empty() && !tail.starts_with(' ') {
                continue;
            }
            let tail = tail.trim_start();
            if consume(tail, names, used | (1 << i)) {
                return true;
            }
            for join in ["and ", "feat ", "featuring ", "with ", "x "] {
                if tail
                    .strip_prefix(join)
                    .is_some_and(|next| consume(next, names, used | (1 << i)))
                {
                    return true;
                }
            }
        }
        false
    }
    consume(&crate::matching::normalize(source), &names, 0)
}
fn recognized_isrc(track: &Track) -> Option<String> {
    if track.provenance["manualFields"]["artist"] == true
        || track.provenance["manualFields"]["title"] == true
    {
        return None;
    }
    let known = crate::recording_identity::RecordingIdentity::from_track(track);
    if known.identifier_conflict {
        return None;
    }
    let code = known.isrc?;
    (track.provider == "radio_song"
        && track.provenance["recognitionIdentifiers"]["isrc"]
            .as_str()
            .and_then(isrc)
            .as_ref()
            == Some(&code))
    .then_some(code)
}
fn filtered_query_code(track: &Track, proof: &Value) -> Option<String> {
    let code = recognized_isrc(track)?;
    let control = &proof["isrcFilterControl"];
    (proof["isrcQuery"] == format!("isrc:{code}")
        && terminal_page(control)
        && control["items"].as_array()?.is_empty())
    .then_some(code)
}
fn matches_identity_with_code(track: &Track, row: &Value, filtered_code: Option<&str>) -> bool {
    let data = &row["data"];
    let Some(refs) = data["artistRefs"].as_array() else {
        return false;
    };
    let identifier_credit = recognized_isrc(track).is_some_and(|known| {
        (data["isrc"].as_str().and_then(isrc).as_deref() == Some(known.as_str())
            || (data["isrc"].as_str().is_none_or(|s| s.is_empty())
                && filtered_code == Some(known.as_str())))
            && data["artist"]
                .as_str()
                .is_some_and(|artist| full_credits(artist, refs))
    });
    matches_identity(track, row)
        || (identifier_credit
            && row["kind"] == "track"
            && row["id"].as_str().is_some_and(valid_spotify_id)
            && data["artistCount"].as_u64() == Some(refs.len() as u64)
            && crate::matching::normalize(&track.title)
                == crate::matching::normalize(data["title"].as_str().unwrap_or_default())
            && !crate::recording_identity::placeholder_title(&track.title)
            && crate::matching::normalize_version(track.version.as_deref().unwrap_or_default())
                == crate::matching::normalize_version(data["version"].as_str().unwrap_or_default())
            && track.album.as_deref().is_none_or(|album| {
                crate::matching::normalize(album)
                    == crate::matching::normalize(data["album"].as_str().unwrap_or_default())
            }))
}
fn matches_identity(track: &Track, row: &Value) -> bool {
    let data = &row["data"];
    let Some(refs) = data["artistRefs"].as_array() else {
        return false;
    };
    row["kind"] == "track"
        && row["id"].as_str().is_some_and(valid_spotify_id)
        && crate::matching::normalize(&track.title)
            == crate::matching::normalize(data["title"].as_str().unwrap_or_default())
        && crate::matching::normalize_version(track.version.as_deref().unwrap_or_default())
            == crate::matching::normalize_version(data["version"].as_str().unwrap_or_default())
        && data["artistCount"].as_u64() == Some(refs.len() as u64)
        && full_credits(&track.artist, refs)
        && track.album.as_deref().is_none_or(|album| {
            crate::matching::normalize(album)
                == crate::matching::normalize(data["album"].as_str().unwrap_or_default())
        })
}
fn choose(track: &Track, rows: &[Value]) -> Option<(f64, Option<String>)> {
    choose_with_code(track, rows, None)
}
fn choose_with_code(
    track: &Track,
    rows: &[Value],
    filtered_code: Option<&str>,
) -> Option<(f64, Option<String>)> {
    // An incomplete same-title row could be another supported recording;
    // filtering it out would manufacture uniqueness from missing evidence.
    if rows.iter().any(|r| {
        crate::matching::normalize(r["data"]["title"].as_str().unwrap_or_default())
            == crate::matching::normalize(&track.title)
            && r["data"]["artistRefs"].as_array().is_none_or(|refs| {
                refs.is_empty() || r["data"]["artistCount"].as_u64() != Some(refs.len() as u64)
            })
    }) {
        return None;
    }
    let rows = rows
        .iter()
        .filter(|r| matches_identity_with_code(track, r, filtered_code))
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return None;
    }
    let mut ids = HashSet::new();
    let mut codes = HashSet::new();
    let mut durations = Vec::new();
    let mut absent = false;
    for row in rows {
        ids.insert(row["id"].as_str()?);
        let d = row["data"]["durationSeconds"].as_f64()?;
        if !d.is_finite() || !(1.0..=86400.0).contains(&d) {
            return None;
        }
        durations.push(d);
        if row["data"]["isrc"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty() && isrc(value).is_none())
        {
            return None;
        }
        match row["data"]["isrc"].as_str().and_then(isrc) {
            Some(code) => {
                codes.insert(code);
            }
            None => {
                if let Some(code) = filtered_code {
                    codes.insert(code.to_owned());
                } else {
                    absent = true;
                }
            }
        }
    }
    if codes.len() > 1 || (absent && ids.len() > 1) || (codes.is_empty() && ids.len() != 1) {
        return None;
    }
    let min = durations.iter().copied().fold(f64::INFINITY, f64::min);
    let max = durations.iter().copied().fold(0.0, f64::max);
    if max - min > 2.0 {
        return None;
    }
    let code = codes.into_iter().next();
    if track
        .isrc
        .as_deref()
        .is_some_and(|known| isrc(known).is_none() || isrc(known) != code)
    {
        return None;
    }
    Some((min, code))
}
fn terminal_page(page: &Value) -> bool {
    let Some(rows) = page.get("items").and_then(Value::as_array) else {
        return false;
    };
    page.get("nextCursor") == Some(&Value::Null)
        && page.get("rawCount").and_then(Value::as_u64) == Some(rows.len() as u64)
        && page.get("unrecognizedCount").and_then(Value::as_u64) == Some(0)
        && page.get("complete").is_none_or(|value| value == true)
        && page.get("partial").is_none_or(|value| value == false)
        && page.get("truncated").is_none_or(|value| value == false)
        && page
            .get("total")
            .is_none_or(|value| value.as_u64() == Some(rows.len() as u64))
}
fn materialize(original: &Track, proof: &Value, account: &str) -> Option<PreparedCatalogDuration> {
    if !eligible(original)
        || proof["version"] != 1
        || proof["original"] != identity(original)
        || proof["accountId"] != account
    {
        return None;
    }
    let age = chrono::Utc::now().timestamp() - proof["checkedAt"].as_i64()?;
    if !(0..=30 * 86400).contains(&age)
        || !terminal_page(&proof["page"])
        || proof["page"]["provider"] != "spotify"
    {
        return None;
    }
    let filtered_code = filtered_query_code(original, proof);
    let rows = proof["page"]["items"].as_array()?;
    if filtered_code.is_some() && rows.len() != 1 {
        return None;
    }
    let (duration, code) = choose_with_code(original, rows, filtered_code.as_deref())?;
    let verified_artist_variants = if recognized_isrc(original).is_some() {
        rows.iter()
            .filter(|r| matches_identity_with_code(original, r, filtered_code.as_deref()))
            .filter_map(|r| r["data"]["artist"].as_str())
            .filter(|artist| *artist != original.artist)
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    let mut track = original.clone();
    track.duration_seconds = Some(duration);
    if track.isrc.is_none() {
        track.isrc = code;
    }
    track.provenance["playbackCatalogDurationProof"] = json!({"version":1,"originalFingerprint":identity(original),"provider":"spotify","checkedAt":proof["checkedAt"]});
    Some(PreparedCatalogDuration {
        track,
        original_fingerprint: identity(original),
        verified_artist_variants,
    })
}
impl RemotePlaylistService {
    async fn catalog_duration_account(&self) -> Result<CachedAccount> {
        if self.load_state()?.forgotten_providers.contains("spotify")
            || !self.bridge.provider_operations_supported()
        {
            return Err("Spotify is not connected".into());
        }
        let status = self
            .bridge
            .provider_operation("spotify", "status", json!({}))
            .await?;
        self.optional_result_account("spotify", &status)?
            .ok_or("Spotify is not connected".into())
    }
    pub(crate) async fn prepare_catalog_duration_track(
        &self,
        original: &Track,
        cancel: tokio::sync::watch::Receiver<bool>,
        deadline: tokio::time::Instant,
    ) -> Result<Option<PreparedCatalogDuration>> {
        if !eligible(original) {
            return Ok(None);
        }
        // Recognized ISRC lookup also checks the provider's filter behavior.
        // Keep both account fences and that independent control inside one
        // bounded opportunity; the caller still reserves eight seconds for audio.
        let budget = std::time::Duration::from_secs(if recognized_isrc(original).is_some() {
            6
        } else {
            4
        });
        let deadline = deadline.min(tokio::time::Instant::now() + budget);
        let started = std::time::Instant::now();
        trace_stage(
            "lookup_start",
            started,
            json!({"recognizedIsrc":recognized_isrc(original).is_some(),"budgetMs":deadline.saturating_duration_since(tokio::time::Instant::now()).as_millis()}),
        );
        await_crosswalk_step(deadline, cancel.clone(), async {
            let before = match self.catalog_duration_account().await {
                Ok(value) => value,
                Err(_) => { trace_stage("account_unavailable", started, json!({})); return Ok(None); },
            };
            trace_stage("account_ready", started, json!({}));
            let saved_at_start = self
                .store
                .get_track(&original.id)?
                .map(|saved| identity(&saved));
            if saved_at_start
                .as_ref()
                .is_some_and(|saved| *saved != identity(original))
            {
                return Ok(None);
            }
            let source_unchanged = || -> Result<bool> {
                Ok(self
                    .store
                    .get_track(&original.id)?
                    .map(|saved| identity(&saved))
                    == saved_at_start)
            };
            if !source_unchanged()? {
                return Ok(None);
            }
            if let Some(proof) = self.store.get_provider_cache(CACHE, &key(original))? {
                if let Some(prepared) = materialize(original, &proof, &before.id) {
                    let after = self.catalog_duration_account().await?;
                    return Ok((after.id == before.id
                        && !*cancel.borrow()
                        && tokio::time::Instant::now() < deadline
                        && source_unchanged()?)
                    .then_some(prepared));
                }
            }
            // Provider field syntax bounds the result set to title hits. A free-text
            // query fills pages with unrelated recommendations and can never
            // supply the complete evidence required by terminal_page.
            let title = original.title.replace(['"', '\\', '\n', '\r'], " ");
            let recognition_code = recognized_isrc(original);
            let query = recognition_code.as_ref().map(|code| format!("isrc:{code}"))
                .unwrap_or_else(|| format!("track:\"{}\"", title.trim()));
            if query.len() > 256 {
                return Ok(None);
            }
            let raw = self
                .bridge
                .provider_operation(
                    "spotify",
                    "search",
                    json!({"query":query,"kind":"track","limit":30}),
                )
                .await?;
            self.ensure_response_account("spotify", &raw, &before.id)?;
            trace_stage("search_complete", started, json!({"rows":raw["items"].as_array().map(Vec::len),"rawCount":raw["rawCount"],"unrecognizedCount":raw["unrecognizedCount"],"hasCursor":raw["nextCursor"].as_str().is_some(),"terminal":terminal_page(&raw)}));
            if serde_json::to_vec(&raw)
                .map_err(|_| "Invalid catalog page")?
                .len()
                > 512 * 1024
                || !terminal_page(&raw)
            {
                return Ok(None);
            }
            let rows = raw["items"].as_array().ok_or("Invalid catalog page")?;
            if rows.len() > 30 {
                return Ok(None);
            }
            let normalized = rows
                .iter()
                .map(|row| normalize_catalog_item(row, "spotify", "track"))
                .collect::<Result<Vec<_>>>()?;
            let mut page = raw;
            page["provider"] = json!("spotify");
            page["items"] = json!(normalized);
            trace_stage("normalized", started, json!({"rows":page["items"].as_array().map(Vec::len),"soleIdentity":page["items"].as_array().is_some_and(|rows| rows.len() == 1 && matches_identity_with_code(original, &rows[0], recognition_code.as_deref()))}));
            let mut filter_control = Value::Null;
            if recognition_code.is_some() && page["items"].as_array().is_some_and(|items| items.len() == 1)
                && matches_identity_with_code(original, &page["items"][0], recognition_code.as_deref()) {
                trace_stage("filter_control_start", started, json!({}));
                let control = self.bridge.provider_operation("spotify", "search", json!({"query":"isrc:ZZZZZ0000000","kind":"track","limit":30})).await?;
                self.ensure_response_account("spotify", &control, &before.id)?;
                trace_stage("filter_control_complete", started, json!({"terminal":terminal_page(&control),"empty":control["items"].as_array().is_some_and(Vec::is_empty)}));
                if terminal_page(&control) && control["items"].as_array().is_some_and(Vec::is_empty) { filter_control = control; }
            }
            // Only a sole title hit can be hydrated without hiding incomplete
            // competing credits. Exact provider data remains independent of YouTube.
            let same_title = page["items"]
                .as_array()
                .ok_or("Invalid catalog page")?
                .iter()
                .enumerate()
                .filter(|(_, row)| {
                    crate::matching::normalize(row["title"].as_str().unwrap_or_default())
                        == crate::matching::normalize(&original.title)
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if same_title.len() == 1 {
                let index = same_title[0];
                let row = &page["items"][index];
                if !matches_identity_with_code(original, row, if filter_control.is_null() { None } else { recognition_code.as_deref() })
                    || row["data"]["durationSeconds"].as_f64().is_none_or(|d| !d.is_finite() || !(1.0..=86400.0).contains(&d))
                    || (original.isrc.is_some() && filter_control.is_null() && row["data"]["isrc"].as_str().and_then(isrc).is_none())
                {
                    trace_stage("exact_hydration_start", started, json!({}));
                    let id = row["id"].as_str().ok_or("Missing catalog ID")?;
                    let (exact, account) = self
                        .get_remote_track_for_account("spotify", id, before.clone())
                        .await?;
                    if account != before.id {
                        return Ok(None);
                    }
                    let mut data = exact.provenance["remoteCatalog"].clone();
                    data["title"] = json!(exact.title);
                    data["durationSeconds"] = json!(exact.duration_seconds);
                    if data["isrc"].is_null() {
                        data["isrc"] = exact.provenance["recordingContext"]["isrc"].clone();
                    }
                    page["items"][index]["data"] = data;
                }
            }
            trace_stage("account_recheck_start", started, json!({}));
            let after = self.catalog_duration_account().await?;
            if before.id != after.id
                || *cancel.borrow()
                || tokio::time::Instant::now() >= deadline
                || !source_unchanged()?
            {
                return Ok(None);
            }
            let proof = json!({"version":1,"original":identity(original),"accountId":before.id,
                "page":page,"isrcQuery":if recognition_code.is_some(){Some(query)}else{None},"isrcFilterControl":filter_control,"checkedAt":chrono::Utc::now().timestamp()});
            let prepared = materialize(original, &proof, &before.id);
            trace_stage("identity_complete", started, json!({"accepted":prepared.is_some()}));
            if prepared.is_some() {
                self.store
                    .set_provider_cache(CACHE, &key(original), &proof, 30 * 86400)?;
            }
            Ok(prepared)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> Track {
        Track {
            id: "radio_song:fixture".into(),
            provider: "radio_song".into(),
            provider_id: "fixture".into(),
            artist: "First Artist & Guest Artist".into(),
            title: "Exact Recording".into(),
            ..Track::default()
        }
    }
    fn row(id: &str) -> Value {
        json!({"id":id,"kind":"track","title":"Exact Recording","data":{"title":"Exact Recording","durationSeconds":240.0,"isrc":"USABC2600001","artistCount":2,"artistRefs":[{"provider":"spotify","id":"aaaaaaaaaaaaaaaaaaaaaa","name":"Guest Artist"},{"provider":"spotify","id":"bbbbbbbbbbbbbbbbbbbbbb","name":"First Artist"}]}})
    }
    fn proof(track: &Track, rows: Vec<Value>) -> Value {
        json!({"version":1,"original":identity(track),"accountId":"account","checkedAt":chrono::Utc::now().timestamp(),"page":{"provider":"spotify","nextCursor":null,"rawCount":rows.len(),"unrecognizedCount":0,"items":rows}})
    }
    #[test]
    fn real_nighttime_catalog_page_distinguishes_full_credits_and_remixes() {
        let original = Track {
            id: "radio_song:nighttime".into(),
            provider: "radio_song".into(),
            provider_id: "nighttime".into(),
            title: "Nighttime in the Backyard".into(),
            artist: "Lucas Santtana & Arto Lindsay".into(),
            provenance: json!({}),
            ..Track::default()
        };
        // Public Spotify title-filter result, including competing credits and remixes.
        let raw: Vec<Value> = serde_json::from_str(r###"[{"id":"0tC7qWz5qL85SFFHKnaerg","kind":"track","title":"Nighttime in the Backyard","url":"https://open.spotify.com/track/0tC7qWz5qL85SFFHKnaerg","data":{"album":"Sem Nostalgia","artist":"Lucas Santtana, Arto Lindsay","artistCount":2,"artistRefs":[{"id":"67muZL7DIwZwLnH9BFfI5Z","name":"Lucas Santtana","provider":"spotify"},{"id":"2914JagOufmP5BY9qLDxtF","name":"Arto Lindsay","provider":"spotify"}],"artworkUrl":"https://i.scdn.co/image/ab67616d00001e02c3aaba4450c13794256c7d1d","available":true,"durationSeconds":197.76,"title":"Nighttime in the Backyard","trackId":"0tC7qWz5qL85SFFHKnaerg","url":"https://open.spotify.com/track/0tC7qWz5qL85SFFHKnaerg"}},{"id":"7wtrUiPoMD7apbdVNm42Aa","kind":"track","title":"Nighttime In The Backyard","url":"https://open.spotify.com/track/7wtrUiPoMD7apbdVNm42Aa","data":{"album":"Nighttime In The Backyard","artist":"Tostex, Lenis Rino, Kika, Beto Villares, Allen Alencar, Cuca Ferreira, Lucas Santtana","artistCount":7,"artistRefs":[{"id":"4vV3Eiaq9WXXiZlUGXIi3i","name":"Tostex","provider":"spotify"},{"id":"3a59TWoEOct1WSYDTWczY0","name":"Lenis Rino","provider":"spotify"},{"id":"5IUYkBHjcEmqy0tPYxgyKD","name":"Kika","provider":"spotify"},{"id":"5buchbVIeExunKNBkTCxlt","name":"Beto Villares","provider":"spotify"},{"id":"7z0HfEXe5IsnX3Svpbf1l8","name":"Allen Alencar","provider":"spotify"},{"id":"2OhBgyzfcQarYe2RrCgDX2","name":"Cuca Ferreira","provider":"spotify"},{"id":"67muZL7DIwZwLnH9BFfI5Z","name":"Lucas Santtana","provider":"spotify"}],"artworkUrl":"https://i.scdn.co/image/ab67616d00001e026299e4e04cd8f41ffb9fe32f","available":true,"durationSeconds":198.001,"title":"Nighttime In The Backyard","trackId":"7wtrUiPoMD7apbdVNm42Aa","url":"https://open.spotify.com/track/7wtrUiPoMD7apbdVNm42Aa"}},{"id":"3QoEQ8ZbWdqBu2O6fKZ7Bc","kind":"track","title":"Nighttime In The Backyard - Adubada","url":"https://open.spotify.com/track/3QoEQ8ZbWdqBu2O6fKZ7Bc","data":{"album":"Nighttime In The Backyard","artist":"Tostex, Lenis Rino, Kika, Beto Villares, Allen Alencar, Lucas Santtana, Cuca Ferreira, Buguinha Dub","artistCount":8,"artistRefs":[{"id":"4vV3Eiaq9WXXiZlUGXIi3i","name":"Tostex","provider":"spotify"},{"id":"3a59TWoEOct1WSYDTWczY0","name":"Lenis Rino","provider":"spotify"},{"id":"5IUYkBHjcEmqy0tPYxgyKD","name":"Kika","provider":"spotify"},{"id":"5buchbVIeExunKNBkTCxlt","name":"Beto Villares","provider":"spotify"},{"id":"7z0HfEXe5IsnX3Svpbf1l8","name":"Allen Alencar","provider":"spotify"},{"id":"67muZL7DIwZwLnH9BFfI5Z","name":"Lucas Santtana","provider":"spotify"},{"id":"2OhBgyzfcQarYe2RrCgDX2","name":"Cuca Ferreira","provider":"spotify"},{"id":"2XEe5tiTUJbMnBQbaFWguL","name":"Buguinha Dub","provider":"spotify"}],"artworkUrl":"https://i.scdn.co/image/ab67616d00001e026299e4e04cd8f41ffb9fe32f","available":true,"durationSeconds":209.574,"title":"Nighttime In The Backyard - Adubada","trackId":"3QoEQ8ZbWdqBu2O6fKZ7Bc","url":"https://open.spotify.com/track/3QoEQ8ZbWdqBu2O6fKZ7Bc"}},{"id":"67NdNq5A4NUbNm9acvDRgi","kind":"track","title":"Nighttime in the backyard - Deerhoof John Dieterich Remix","url":"https://open.spotify.com/track/67NdNq5A4NUbNm9acvDRgi","data":{"album":"Remix Nostalgia","artist":"Lucas Santtana, John Dieterich, Deerhoof","artistCount":3,"artistRefs":[{"id":"67muZL7DIwZwLnH9BFfI5Z","name":"Lucas Santtana","provider":"spotify"},{"id":"5JrG5IVyvhOBaoBKBSnOtf","name":"John Dieterich","provider":"spotify"},{"id":"7AZwAitWq1KcFoIJhRWb6V","name":"Deerhoof","provider":"spotify"}],"artworkUrl":"https://i.scdn.co/image/ab67616d00001e0270ff430a7dec82201cbb249c","available":true,"durationSeconds":190.666,"title":"Nighttime in the backyard - Deerhoof John Dieterich Remix","trackId":"67NdNq5A4NUbNm9acvDRgi","url":"https://open.spotify.com/track/67NdNq5A4NUbNm9acvDRgi"}},{"id":"23JUQdGtT4UVFuAxJGZUYn","kind":"track","title":"Nighttime In The Backyard","url":"https://open.spotify.com/track/23JUQdGtT4UVFuAxJGZUYn","data":{"album":"Tostex, Vol. 1","artist":"Tostex, Lenis Rino, Kika, Beto Villares, Allen Alencar, Lucas Santtana, Cuca Ferreira","artistCount":7,"artistRefs":[{"id":"4vV3Eiaq9WXXiZlUGXIi3i","name":"Tostex","provider":"spotify"},{"id":"3a59TWoEOct1WSYDTWczY0","name":"Lenis Rino","provider":"spotify"},{"id":"5IUYkBHjcEmqy0tPYxgyKD","name":"Kika","provider":"spotify"},{"id":"5buchbVIeExunKNBkTCxlt","name":"Beto Villares","provider":"spotify"},{"id":"7z0HfEXe5IsnX3Svpbf1l8","name":"Allen Alencar","provider":"spotify"},{"id":"67muZL7DIwZwLnH9BFfI5Z","name":"Lucas Santtana","provider":"spotify"},{"id":"2OhBgyzfcQarYe2RrCgDX2","name":"Cuca Ferreira","provider":"spotify"}],"artworkUrl":"https://i.scdn.co/image/ab67616d00001e0214cfc93b6e409f8030e3d5ca","available":true,"durationSeconds":198.001,"title":"Nighttime In The Backyard","trackId":"23JUQdGtT4UVFuAxJGZUYn","url":"https://open.spotify.com/track/23JUQdGtT4UVFuAxJGZUYn"}}]"###).unwrap();
        let rows = raw
            .iter()
            .map(|row| normalize_catalog_item(row, "spotify", "track").unwrap())
            .collect::<Vec<_>>();
        let prepared = materialize(&original, &proof(&original, rows), "account").unwrap();
        assert_eq!(prepared.track.duration_seconds, Some(197.76));
        assert_eq!(prepared.track.artist, original.artist);
        assert_eq!(original.duration_seconds, None);
    }

    #[test]
    fn asset_and_chapter_durations_are_not_catalog_enrichment_targets() {
        let original = track();
        assert!(eligible(&original));
        for field in ["path", "asset", "chapters", "manual"] {
            let mut changed = original.clone();
            match field {
                "path" => changed.local_path = Some("/registered/track.wav".into()),
                "asset" => changed.asset_id = Some("asset:excerpt".into()),
                "chapters" => changed.provenance["chapters"] = json!([{"startTime":0}]),
                _ => changed.provenance["manualFields"]["durationSeconds"] = json!(true),
            }
            assert!(!eligible(&changed), "{field}");
        }
    }

    #[test]
    fn recognized_isrc_binds_locale_credit_variants_only_with_complete_filter_evidence() {
        let mut source = track();
        source.artist = "ฌอน".into();
        source.title = "velib'bleu".into();
        source.isrc = Some("FR9W12639430".into());
        source.provenance = json!({"recognitionIdentifiers":{"isrc":"FR9W12639430"}});
        let mut r = row("cccccccccccccccccccccc");
        r["data"]["title"] = json!(source.title);
        r["data"]["artist"] = json!("sean");
        r["data"]["artistRefs"] =
            json!([{"provider":"spotify","id":"aaaaaaaaaaaaaaaaaaaaaa","name":"sean"}]);
        r["data"]["artistCount"] = json!(1);
        r["data"]["isrc"] = Value::Null;
        let mut p = proof(&source, vec![r.clone()]);
        p["isrcQuery"] = json!("isrc:FR9W12639430");
        p["isrcFilterControl"] =
            json!({"items":[],"nextCursor":null,"rawCount":0,"unrecognizedCount":0});
        let prepared = materialize(&source, &p, "account").unwrap();
        assert_eq!(prepared.verified_artist_variants, vec!["sean"]);
        assert_eq!(prepared.track.artist, "ฌอน");
        assert_eq!(source.duration_seconds, None);
        for field in [
            "ignored-filter",
            "wrong-code",
            "partial-control",
            "manual-artist",
            "wrong-title",
        ] {
            let mut bad = p.clone();
            let mut original = source.clone();
            match field {
                "ignored-filter" => bad["isrcFilterControl"] = bad["page"].clone(),
                "wrong-code" => bad["page"]["items"][0]["data"]["isrc"] = json!("FR9W12639431"),
                "partial-control" => bad["isrcFilterControl"]["nextCursor"] = json!("30"),
                "manual-artist" => original.provenance["manualFields"] = json!({"artist":true}),
                _ => bad["page"]["items"][0]["data"]["title"] = json!("Unrelated song"),
            }
            assert!(materialize(&original, &bad, "account").is_none(), "{field}");
        }
    }

    #[test]
    fn complete_credit_bijection_and_one_isrc_family_enrich_only_clone() {
        let original = track();
        let snapshot = serde_json::to_value(&original).unwrap();
        let prepared = materialize(
            &original,
            &proof(
                &original,
                vec![row("cccccccccccccccccccccc"), row("dddddddddddddddddddddd")],
            ),
            "account",
        )
        .unwrap();
        assert_eq!(prepared.track.duration_seconds, Some(240.0));
        assert_eq!(prepared.track.isrc.as_deref(), Some("USABC2600001"));
        assert_eq!(prepared.original_fingerprint, identity(&original));
        assert_eq!(serde_json::to_value(&original).unwrap(), snapshot);
        assert_eq!(prepared.track.artist, original.artist);
        assert_eq!(prepared.track.title, original.title);
        assert_eq!(prepared.track.provider, original.provider);
    }
    #[test]
    fn rejects_wrong_guest_title_version_and_known_isrc() {
        for field in ["guest", "title", "version", "isrc", "album"] {
            let mut source = track();
            let mut r = row("cccccccccccccccccccccc");
            match field {
                "guest" => r["data"]["artistRefs"][0]["name"] = json!("Different Guest"),
                "title" => r["data"]["title"] = json!("Different Recording"),
                "version" => r["data"]["version"] = json!("Live"),
                "isrc" => source.isrc = Some("USABC2600002".into()),
                _ => source.album = Some("Known Release".into()),
            }
            assert!(choose(&source, &[r]).is_none(), "{field}");
        }
    }
    #[test]
    fn ambiguous_recordings_and_incomplete_competitor_fail_closed() {
        let source = track();
        let left = row("cccccccccccccccccccccc");
        for field in ["duration", "isrc", "credits", "missing-isrc"] {
            let mut right = row("dddddddddddddddddddddd");
            match field {
                "duration" => right["data"]["durationSeconds"] = json!(250.0),
                "isrc" => right["data"]["isrc"] = json!("USABC2600002"),
                "credits" => right["data"]["artistCount"] = json!(3),
                _ => right["data"]["isrc"] = Value::Null,
            }
            assert!(choose(&source, &[left.clone(), right]).is_none(), "{field}");
        }
        let mut no_isrc = left.clone();
        no_isrc["data"]["isrc"] = Value::Null;
        assert_eq!(choose(&source, &[no_isrc]), Some((240.0, None)));
    }
    #[test]
    fn proof_rejects_stale_account_identity_page_and_existing_duration() {
        let source = track();
        let good = proof(&source, vec![row("cccccccccccccccccccccc")]);
        for field in ["account", "identity", "page", "age", "future", "duration"] {
            let mut p = good.clone();
            let mut incoming = source.clone();
            match field {
                "account" => p["accountId"] = json!("different"),
                "identity" => incoming.title = "Changed".into(),
                "page" => p["page"]["nextCursor"] = json!("truncated"),
                "age" => p["checkedAt"] = json!(chrono::Utc::now().timestamp() - 31 * 86400),
                "future" => p["checkedAt"] = json!(chrono::Utc::now().timestamp() + 60),
                _ => incoming.duration_seconds = Some(99.0),
            }
            assert!(materialize(&incoming, &p, "account").is_none(), "{field}");
        }
    }
    #[test]
    fn credit_matching_does_not_split_names_or_omit_duplicate_guests() {
        let r = row("cccccccccccccccccccccc");
        let refs = r["data"]["artistRefs"].as_array().unwrap();
        assert!(full_credits("Guest Artist, First Artist", refs));
        assert!(!full_credits("First Artist", refs));
        assert!(!full_credits("First Artist & Guest Artist & Extra", refs));
    }
    struct Fixture {
        searches: std::sync::atomic::AtomicUsize,
        search_args: std::sync::Mutex<Vec<Value>>,
        mode: &'static str,
    }
    #[async_trait::async_trait]
    impl ProviderTransport for Fixture {
        fn provider_operations_supported(&self) -> bool {
            true
        }
        async fn provider_operation(
            &self,
            _provider: &str,
            op: &str,
            args: Value,
        ) -> Result<Value> {
            match op {
                "status" => Ok(
                    json!({"accountId":if self.mode=="switch" && self.searches.load(Ordering::SeqCst)>0 {"changed"} else {"account"}}),
                ),
                "search" => {
                    self.searches.fetch_add(1, Ordering::SeqCst);
                    self.search_args.lock().unwrap().push(args.clone());
                    if self.mode == "wait" {
                        return std::future::pending().await;
                    }
                    if self.mode == "isrc-locale" || self.mode == "wrong-control-account" {
                        let query = args["query"].as_str().unwrap_or_default();
                        if query == "isrc:ZZZZZ0000000" {
                            return Ok(
                                json!({"accountId":if self.mode=="wrong-control-account" {"changed"} else {"account"},"items":[],"nextCursor":null,"rawCount":0,"unrecognizedCount":0}),
                            );
                        }
                        if query != "isrc:FR9W12639430" {
                            return Err("Unexpected recognized ISRC query".into());
                        }
                        let mut r = json!({"id":"cccccccccccccccccccccc","kind":"track","title":"velib'bleu","url":"https://open.spotify.com/track/cccccccccccccccccccccc","data":{"title":"velib'bleu","artist":"sean","durationSeconds":203.491,"isrc":null,"artistCount":1,"artistRefs":[{"provider":"spotify","id":"aaaaaaaaaaaaaaaaaaaaaa","name":"sean"}]}});
                        return Ok(
                            json!({"accountId":"account","items":[r],"nextCursor":null,"rawCount":1,"unrecognizedCount":0}),
                        );
                    }
                    let mut r = row("cccccccccccccccccccccc");
                    r["url"] = json!("https://open.spotify.com/track/cccccccccccccccccccccc");
                    if self.mode == "without-isrc" {
                        r["data"]["isrc"] = Value::Null;
                    }
                    Ok(
                        json!({"accountId":"account","items":[r],"nextCursor":null,"rawCount":1,"unrecognizedCount":0}),
                    )
                }
                _ => Err("Unexpected fixture operation".into()),
            }
        }
        async fn provider_operation_cancellable(
            &self,
            p: &str,
            o: &str,
            a: Value,
            _c: Option<Arc<AtomicBool>>,
        ) -> Result<Value> {
            self.provider_operation(p, o, a).await
        }
    }
    #[tokio::test]
    async fn complete_sole_recording_without_optional_isrc_needs_no_rest_hydration() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("test.sqlite")).unwrap());
        let original = track();
        store.save_track(&original).unwrap();
        let bridge = Arc::new(Fixture {
            searches: std::sync::atomic::AtomicUsize::new(0),
            search_args: std::sync::Mutex::new(Vec::new()),
            mode: "without-isrc",
        });
        let service = RemotePlaylistService::new(store, bridge.clone());
        let (_stop, cancel) = tokio::sync::watch::channel(false);
        // The fixture rejects every getTrack operation, so an unnecessary REST
        // hydration would fail despite sufficient search evidence.
        assert!(service
            .prepare_catalog_duration_track(
                &original,
                cancel,
                tokio::time::Instant::now() + std::time::Duration::from_secs(1)
            )
            .await
            .unwrap()
            .is_some());
        assert_eq!(bridge.searches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn recognized_isrc_locale_bridge_uses_exact_filter_and_empty_control() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("test.sqlite")).unwrap());
        let mut original = track();
        original.artist = "ฌอน".into();
        original.title = "velib'bleu".into();
        original.isrc = Some("FR9W12639430".into());
        original.provenance = json!({"recognitionIdentifiers":{"isrc":"FR9W12639430"}});
        store.save_track(&original).unwrap();
        let saved_before = store.get_track(&original.id).unwrap().unwrap();
        let bridge = Arc::new(Fixture {
            searches: std::sync::atomic::AtomicUsize::new(0),
            search_args: std::sync::Mutex::new(Vec::new()),
            mode: "isrc-locale",
        });
        let service = RemotePlaylistService::new(store.clone(), bridge.clone());
        let (_stop, cancel) = tokio::sync::watch::channel(false);
        let prepared = service
            .prepare_catalog_duration_track(
                &original,
                cancel.clone(),
                tokio::time::Instant::now() + std::time::Duration::from_secs(2),
            )
            .await
            .unwrap()
            .expect("verified catalog duration should prepare");
        assert_eq!(prepared.track.duration_seconds, Some(203.491));
        assert_eq!(prepared.track.artist, "ฌอน");
        assert_eq!(prepared.verified_artist_variants, vec!["sean".to_owned()]);
        assert_eq!(bridge.searches.load(Ordering::SeqCst), 2);
        let args = bridge.search_args.lock().unwrap();
        assert_eq!(
            args[0],
            json!({"query":"isrc:FR9W12639430","kind":"track","limit":30})
        );
        assert_eq!(
            args[1],
            json!({"query":"isrc:ZZZZZ0000000","kind":"track","limit":30})
        );
        drop(args);

        let cached = store
            .get_provider_cache(CACHE, &key(&original))
            .unwrap()
            .expect("proof should be cached");
        assert_eq!(cached["isrcQuery"], "isrc:FR9W12639430");
        assert_eq!(cached["isrcFilterControl"]["items"], json!([]));
        assert!(materialize(&original, &cached, "account").is_some());
        assert_eq!(
            store.get_track(&original.id).unwrap().unwrap(),
            saved_before
        );

        // A cache read still makes no provider operation beyond the account check;
        // crucially, the exact and control searches are not repeated.
        assert!(service
            .prepare_catalog_duration_track(
                &original,
                cancel,
                tokio::time::Instant::now() + std::time::Duration::from_secs(2),
            )
            .await
            .unwrap()
            .is_some());
        assert_eq!(bridge.searches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn recognized_isrc_bridge_rejects_control_query_account_switch() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("test.sqlite")).unwrap());
        let mut original = track();
        original.artist = "ฌอน".into();
        original.title = "velib'bleu".into();
        original.isrc = Some("FR9W12639430".into());
        original.provenance = json!({"recognitionIdentifiers":{"isrc":"FR9W12639430"}});
        store.save_track(&original).unwrap();
        let saved_before = store.get_track(&original.id).unwrap().unwrap();
        let bridge = Arc::new(Fixture {
            searches: std::sync::atomic::AtomicUsize::new(0),
            search_args: std::sync::Mutex::new(Vec::new()),
            mode: "wrong-control-account",
        });
        let service = RemotePlaylistService::new(store.clone(), bridge.clone());
        let (_stop, cancel) = tokio::sync::watch::channel(false);
        let result = service
            .prepare_catalog_duration_track(
                &original,
                cancel,
                tokio::time::Instant::now() + std::time::Duration::from_secs(2),
            )
            .await;
        assert!(matches!(result, Err(_) | Ok(None)));
        assert_eq!(bridge.searches.load(Ordering::SeqCst), 2);
        assert!(store
            .get_provider_cache(CACHE, &key(&original))
            .unwrap()
            .is_none());
        assert_eq!(
            store.get_track(&original.id).unwrap().unwrap(),
            saved_before
        );
    }

    #[tokio::test]
    async fn service_retains_proof_and_never_updates_saved_tags() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(&dir.path().join("test.sqlite")).unwrap());
        let original = track();
        store.save_track(&original).unwrap();
        let bridge = Arc::new(Fixture {
            searches: std::sync::atomic::AtomicUsize::new(0),
            search_args: std::sync::Mutex::new(Vec::new()),
            mode: "normal",
        });
        let service = RemotePlaylistService::new(store.clone(), bridge.clone());
        let (_tx, cancel) = tokio::sync::watch::channel(false);
        for _ in 0..2 {
            assert!(service
                .prepare_catalog_duration_track(
                    &original,
                    cancel.clone(),
                    tokio::time::Instant::now() + std::time::Duration::from_secs(1)
                )
                .await
                .unwrap()
                .is_some());
        }
        assert_eq!(bridge.searches.load(Ordering::SeqCst), 1);
        let cached = store
            .get_provider_cache(CACHE, &key(&original))
            .unwrap()
            .expect("normal TTL cache is writable");
        assert!(materialize(&original, &cached, "account").is_some());
        assert_eq!(
            store
                .get_track(&original.id)
                .unwrap()
                .unwrap()
                .duration_seconds,
            None
        );
        let mut changed = original.clone();
        changed.title = "Changed".into();
        store.save_track(&changed).unwrap();
        assert!(service
            .prepare_catalog_duration_track(
                &original,
                cancel,
                tokio::time::Instant::now() + std::time::Duration::from_secs(1)
            )
            .await
            .unwrap()
            .is_none());
    }
    #[tokio::test]
    async fn account_switch_cancel_and_original_deadline_never_publish_proof() {
        for mode in ["switch", "wait", "cancel"] {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(Store::open(&dir.path().join("test.sqlite")).unwrap());
            let original = track();
            let bridge = Arc::new(Fixture {
                searches: std::sync::atomic::AtomicUsize::new(0),
                search_args: std::sync::Mutex::new(Vec::new()),
                mode: if mode == "cancel" { "wait" } else { mode },
            });
            let service = RemotePlaylistService::new(store.clone(), bridge);
            let (tx, cancel) = tokio::sync::watch::channel(false);
            let trigger = tokio::spawn(async move {
                if mode == "cancel" {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    tx.send_replace(true);
                }
            });
            let result = service
                .prepare_catalog_duration_track(
                    &original,
                    cancel,
                    tokio::time::Instant::now() + std::time::Duration::from_millis(40),
                )
                .await;
            assert!(result.is_err() || result.unwrap().is_none(), "{mode}");
            assert!(store
                .get_provider_cache(CACHE, &key(&original))
                .unwrap()
                .is_none());
            trigger.await.unwrap();
        }
    }
    #[test]
    fn terminal_page_requires_explicit_cursor_and_no_truncation_signals() {
        let source = track();
        let good = proof(&source, vec![row("cccccccccccccccccccccc")]);
        for field in [
            "missing",
            "malformed",
            "partial",
            "truncated",
            "complete",
            "total",
            "unrecognized",
            "missing-count",
            "raw-count",
        ] {
            let mut p = good.clone();
            match field {
                "missing" => {
                    p["page"].as_object_mut().unwrap().remove("nextCursor");
                }
                "malformed" => p["page"]["nextCursor"] = json!({"bad":true}),
                "partial" => p["page"]["partial"] = json!(true),
                "truncated" => p["page"]["truncated"] = json!(true),
                "complete" => p["page"]["complete"] = json!(false),
                "unrecognized" => {
                    p["page"]["rawCount"] = json!(2);
                    p["page"]["unrecognizedCount"] = json!(1);
                }
                "missing-count" => {
                    p["page"].as_object_mut().unwrap().remove("rawCount");
                }
                "raw-count" => p["page"]["rawCount"] = json!(2),
                _ => p["page"]["total"] = json!(2),
            }
            assert!(materialize(&source, &p, "account").is_none(), "{field}");
        }
    }
}
