//! Destination-local review gate for automatic work restored from a backup.
//!
//! A full restore can contain durable tag-writing or provider work queues and
//! restored opt-in streamed-media work. Queued items remain intact; automatic
//! playback-triggered stream analysis is also paused until explicit review.

use crate::{analysis, models::LocalAsset, storage::Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const PREFERENCE_KEY: &str = "restoreAutomaticWork.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticWorkKind {
    MetadataRetagAnalysis,
    AssetAutotag,
    BackgroundSourcePreparation,
    BackgroundStreamMediaAnalysis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomaticWorkCategoryState {
    pub kind: AutomaticWorkKind,
    pub paused: bool,
    pub pending_count: usize,
    pub writes_audio_tags: bool,
    pub queries_providers: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreWorkPolicyState {
    pub review_required: bool,
    pub snapshot_id: Option<String>,
    pub reviewed_at: Option<String>,
    pub warning: Option<String>,
    pub categories: Vec<AutomaticWorkCategoryState>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeAutomaticWork {
    #[serde(default)]
    pub metadata_retag_analysis: bool,
    #[serde(default)]
    pub asset_autotag: bool,
    #[serde(default)]
    pub background_source_preparation: bool,
    #[serde(default)]
    pub background_stream_media_analysis: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedPolicy {
    schema_version: u8,
    review_required: bool,
    snapshot_id: Option<String>,
    reviewed_at: Option<String>,
    metadata_retag_analysis_paused: bool,
    asset_autotag_paused: bool,
    background_source_preparation_paused: bool,
    /// `None` is retained for old persisted markers. Treat a missing value as
    /// paused for any restored profile until this category is reviewed, while
    /// ordinary profiles without a restore marker remain unpaused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    background_stream_media_analysis_paused: Option<bool>,
}

impl PersistedPolicy {
    fn for_restore(snapshot_id: &str) -> Result<Self, String> {
        let snapshot_id = snapshot_id.trim();
        if snapshot_id.is_empty() || snapshot_id.len() > 256 {
            return Err("Restore snapshot identity is invalid".into());
        }
        Ok(Self {
            schema_version: 1,
            review_required: true,
            snapshot_id: Some(snapshot_id.to_owned()),
            reviewed_at: None,
            metadata_retag_analysis_paused: true,
            asset_autotag_paused: true,
            background_source_preparation_paused: true,
            background_stream_media_analysis_paused: Some(true),
        })
    }

    fn fail_closed() -> Self {
        Self {
            schema_version: 1,
            review_required: true,
            metadata_retag_analysis_paused: true,
            asset_autotag_paused: true,
            background_source_preparation_paused: true,
            background_stream_media_analysis_paused: Some(true),
            ..Self::default()
        }
    }

    fn paused(&self, kind: AutomaticWorkKind) -> bool {
        match kind {
            AutomaticWorkKind::MetadataRetagAnalysis => self.metadata_retag_analysis_paused,
            AutomaticWorkKind::AssetAutotag => self.asset_autotag_paused,
            AutomaticWorkKind::BackgroundSourcePreparation => {
                self.background_source_preparation_paused
            }
            AutomaticWorkKind::BackgroundStreamMediaAnalysis => self
                .background_stream_media_analysis_paused
                .unwrap_or(self.review_required || self.snapshot_id.is_some()),
        }
    }
}

fn read_policy(store: &Store) -> Result<PersistedPolicy, String> {
    let Some(value) = store
        .get_json_preference(PREFERENCE_KEY)
        .map_err(|error| error.to_string())?
    else {
        // No restore marker means ordinary startup behavior is unchanged.
        return Ok(PersistedPolicy {
            schema_version: 1,
            ..PersistedPolicy::default()
        });
    };
    let policy: PersistedPolicy =
        serde_json::from_value(value).map_err(|_| "Restore work policy is malformed".to_owned())?;
    if policy.schema_version != 1 {
        return Err("Restore work policy version is not supported".into());
    }
    Ok(policy)
}

fn save_policy(store: &Store, policy: &PersistedPolicy) -> Result<(), String> {
    let value = serde_json::to_value(policy).map_err(|error| error.to_string())?;
    store
        .set_json_preference(PREFERENCE_KEY, &value)
        .map_err(|error| error.to_string())
}

/// Build a persisted policy value that pauses only automatic-work categories
/// enabled by an explicit settings import. The caller writes this value in
/// the same SQLite transaction as the imported settings.
pub fn pause_selected_for_settings_import(
    current: Option<&Value>,
    categories: &[AutomaticWorkKind],
) -> Result<Option<Value>, String> {
    if categories.is_empty() {
        return Ok(None);
    }
    let mut policy = match current {
        Some(value) => serde_json::from_value::<PersistedPolicy>(value.clone())
            .ok()
            .filter(|policy| policy.schema_version == 1)
            .unwrap_or_else(PersistedPolicy::fail_closed),
        None => PersistedPolicy {
            schema_version: 1,
            ..PersistedPolicy::default()
        },
    };
    // Preserve the effective old value of this newer optional field before
    // review_required changes its legacy fallback semantics.
    let stream_was_paused = policy.paused(AutomaticWorkKind::BackgroundStreamMediaAnalysis);
    policy.background_stream_media_analysis_paused = Some(stream_was_paused);
    policy.review_required = true;
    policy.reviewed_at = None;
    for category in categories {
        match category {
            AutomaticWorkKind::MetadataRetagAnalysis => {
                policy.metadata_retag_analysis_paused = true;
            }
            AutomaticWorkKind::AssetAutotag => policy.asset_autotag_paused = true,
            AutomaticWorkKind::BackgroundSourcePreparation => {
                policy.background_source_preparation_paused = true;
            }
            AutomaticWorkKind::BackgroundStreamMediaAnalysis => {
                policy.background_stream_media_analysis_paused = Some(true);
            }
        }
    }
    serde_json::to_value(policy)
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Returns true on storage or schema errors so automatic work fails closed.
pub fn is_paused(store: &Store, kind: AutomaticWorkKind) -> bool {
    read_policy(store).map_or(true, |policy| policy.paused(kind))
}

/// Gate only work started automatically by a playback preference. Explicit
/// user requests remain available while restore review is pending.
pub fn ensure_stream_media_analysis_allowed(store: &Store, automatic: bool) -> Result<(), String> {
    if automatic && is_paused(store, AutomaticWorkKind::BackgroundStreamMediaAnalysis) {
        return Err("Automatic streamed-track analysis is paused until restore review.".into());
    }
    Ok(())
}

/// Record that a restored profile requires explicit review before automatic
/// work can resume. Backup code should call this on the staged destination
/// Store before publishing the restored database.
pub fn pause_all_after_restore(
    store: &Store,
    snapshot_id: &str,
) -> Result<RestoreWorkPolicyState, String> {
    let policy = PersistedPolicy::for_restore(snapshot_id)?;
    save_policy(store, &policy)?;
    state_with_policy(store, policy, None)
}

/// Persist the user's reviewed choices. Any unselected category stays paused.
pub fn review(
    store: &Store,
    resume: ResumeAutomaticWork,
) -> Result<RestoreWorkPolicyState, String> {
    // A malformed marker already pauses workers. Let an explicit review
    // replace it with a fresh, validated choice instead of leaving the user
    // unable to recover through the UI.
    let mut policy = read_policy(store).unwrap_or_else(|_| PersistedPolicy::fail_closed());
    if !policy.review_required && policy.snapshot_id.is_none() {
        return Err("There is no restored automatic work to review".into());
    }
    policy.review_required = false;
    policy.reviewed_at = Some(chrono::Utc::now().to_rfc3339());
    policy.metadata_retag_analysis_paused = !resume.metadata_retag_analysis;
    policy.asset_autotag_paused = !resume.asset_autotag;
    policy.background_source_preparation_paused = !resume.background_source_preparation;
    policy.background_stream_media_analysis_paused = Some(!resume.background_stream_media_analysis);
    save_policy(store, &policy)?;
    state_with_policy(store, policy, None)
}

pub fn state(store: &Store) -> Result<RestoreWorkPolicyState, String> {
    match read_policy(store) {
        Ok(policy) => state_with_policy(store, policy, None),
        Err(error) => state_with_policy(store, PersistedPolicy::fail_closed(), Some(error)),
    }
}

fn state_with_policy(
    store: &Store,
    policy: PersistedPolicy,
    warning: Option<String>,
) -> Result<RestoreWorkPolicyState, String> {
    let counts = pending_counts(store)?;
    let metadata_retag_analysis_paused = policy.paused(AutomaticWorkKind::MetadataRetagAnalysis);
    let asset_autotag_paused = policy.paused(AutomaticWorkKind::AssetAutotag);
    let background_source_preparation_paused =
        policy.paused(AutomaticWorkKind::BackgroundSourcePreparation);
    let background_stream_media_analysis_paused =
        policy.paused(AutomaticWorkKind::BackgroundStreamMediaAnalysis);
    Ok(RestoreWorkPolicyState {
        review_required: policy.review_required,
        snapshot_id: policy.snapshot_id,
        reviewed_at: policy.reviewed_at,
        warning,
        categories: vec![
            AutomaticWorkCategoryState {
                kind: AutomaticWorkKind::MetadataRetagAnalysis,
                paused: metadata_retag_analysis_paused,
                pending_count: counts.metadata_retag_analysis,
                writes_audio_tags: true,
                queries_providers: false,
            },
            AutomaticWorkCategoryState {
                kind: AutomaticWorkKind::AssetAutotag,
                paused: asset_autotag_paused,
                pending_count: counts.asset_autotag,
                writes_audio_tags: true,
                queries_providers: true,
            },
            AutomaticWorkCategoryState {
                kind: AutomaticWorkKind::BackgroundSourcePreparation,
                paused: background_source_preparation_paused,
                pending_count: counts.background_source_preparation,
                writes_audio_tags: false,
                queries_providers: true,
            },
            AutomaticWorkCategoryState {
                kind: AutomaticWorkKind::BackgroundStreamMediaAnalysis,
                paused: background_stream_media_analysis_paused,
                // This is an opt-in behavior on playback, not a durable queue.
                pending_count: 0,
                writes_audio_tags: false,
                queries_providers: true,
            },
        ],
    })
}

#[derive(Debug, Default)]
struct PendingCounts {
    metadata_retag_analysis: usize,
    asset_autotag: usize,
    background_source_preparation: usize,
}

fn pending_counts(store: &Store) -> Result<PendingCounts, String> {
    Ok(PendingCounts {
        metadata_retag_analysis: pending_metadata_retag_count(store)?,
        asset_autotag: pending_autotag_count(store)?,
        background_source_preparation: pending_source_preparation_count(store)?,
    })
}

fn pending_metadata_retag_count(store: &Store) -> Result<usize, String> {
    let candidates = store
        .list_analysis_queue_assets(None, false, analysis::VERSION)
        .map_err(|error| error.to_string())?;
    let candidate_ids = candidates
        .into_iter()
        .map(|asset| asset.id)
        .collect::<std::collections::HashSet<_>>();
    let assets = store.list_assets().map_err(|error| error.to_string())?;
    Ok(assets
        .iter()
        .filter(|asset| candidate_ids.contains(&asset.id) && needs_metadata_retag(asset))
        .count())
}

fn needs_metadata_retag(asset: &LocalAsset) -> bool {
    if asset.processing == "external-readonly" || !std::path::Path::new(&asset.path).is_file() {
        return false;
    }
    let manual = &asset.track.provenance["manualFields"];
    let bpm_present = asset.track.bpm.is_some() || manual["bpm"] == true;
    let key_present = asset
        .track
        .key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty())
        || manual["key"] == true;
    !(bpm_present && key_present)
}

fn pending_autotag_count(store: &Store) -> Result<usize, String> {
    let assets = store
        .list_assets()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|asset| (asset.id.clone(), asset.hash))
        .collect::<std::collections::HashMap<_, _>>();
    let records = store
        .list_autotag_records()
        .map_err(|error| error.to_string())?;
    Ok(records
        .iter()
        .filter(|record| {
            matches!(
                record["status"].as_str(),
                Some("queued" | "retry" | "matching" | "paused" | "restore_review")
            ) && record["assetId"]
                .as_str()
                .zip(record["inputHash"].as_str())
                .is_some_and(|(id, hash)| assets.get(id).is_some_and(|current| current == hash))
        })
        .count())
}

fn pending_source_preparation_count(store: &Store) -> Result<usize, String> {
    crate::source_preparation::pending_count(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn empty_store() -> Store {
        let dir = tempdir().unwrap();
        // Keep the temporary directory alive for the duration of the test.
        let path = dir.keep().join("burrata.sqlite");
        Store::open(&path).unwrap()
    }

    #[test]
    fn ordinary_profile_has_no_review_pause_and_restore_requires_explicit_category_choices() {
        let store = empty_store();
        let ordinary = read_policy(&store).unwrap();
        assert!(!ordinary.review_required);
        assert!(!ordinary.paused(AutomaticWorkKind::MetadataRetagAnalysis));
        assert!(!ordinary.paused(AutomaticWorkKind::AssetAutotag));
        assert!(!ordinary.paused(AutomaticWorkKind::BackgroundSourcePreparation));
        assert!(!ordinary.paused(AutomaticWorkKind::BackgroundStreamMediaAnalysis));
        assert!(review(&store, ResumeAutomaticWork::default()).is_err());

        pause_all_after_restore(&store, "snapshot-123").unwrap();
        assert!(is_paused(&store, AutomaticWorkKind::MetadataRetagAnalysis));
        assert!(is_paused(&store, AutomaticWorkKind::AssetAutotag));
        assert!(is_paused(
            &store,
            AutomaticWorkKind::BackgroundSourcePreparation
        ));
        assert!(is_paused(
            &store,
            AutomaticWorkKind::BackgroundStreamMediaAnalysis
        ));
        assert!(ensure_stream_media_analysis_allowed(&store, true).is_err());
        assert!(ensure_stream_media_analysis_allowed(&store, false).is_ok());
        let state = state(&store).unwrap();
        assert!(state.review_required);
        assert_eq!(state.snapshot_id.as_deref(), Some("snapshot-123"));

        let resumed = review(
            &store,
            ResumeAutomaticWork {
                metadata_retag_analysis: true,
                asset_autotag: false,
                background_source_preparation: true,
                background_stream_media_analysis: true,
            },
        )
        .unwrap();
        assert!(!resumed.review_required);
        assert!(!is_paused(&store, AutomaticWorkKind::MetadataRetagAnalysis));
        assert!(is_paused(&store, AutomaticWorkKind::AssetAutotag));
        assert!(!is_paused(
            &store,
            AutomaticWorkKind::BackgroundSourcePreparation
        ));
        assert!(!is_paused(
            &store,
            AutomaticWorkKind::BackgroundStreamMediaAnalysis
        ));
        assert!(ensure_stream_media_analysis_allowed(&store, true).is_ok());
    }

    #[test]
    fn settings_import_pause_preserves_prior_choices_and_targets_selected_categories() {
        let prior = json!({
            "schemaVersion": 1,
            "reviewRequired": false,
            "snapshotId": "previously-reviewed",
            "reviewedAt": "2026-10-25T00:00:00Z",
            "metadataRetagAnalysisPaused": true,
            "assetAutotagPaused": false,
            "backgroundSourcePreparationPaused": false,
            "backgroundStreamMediaAnalysisPaused": false
        });
        let updated =
            pause_selected_for_settings_import(Some(&prior), &[AutomaticWorkKind::AssetAutotag])
                .unwrap()
                .unwrap();
        let policy: PersistedPolicy = serde_json::from_value(updated).unwrap();
        assert!(policy.review_required);
        assert_eq!(policy.snapshot_id.as_deref(), Some("previously-reviewed"));
        assert!(policy.reviewed_at.is_none());
        assert!(policy.paused(AutomaticWorkKind::MetadataRetagAnalysis));
        assert!(policy.paused(AutomaticWorkKind::AssetAutotag));
        assert!(!policy.paused(AutomaticWorkKind::BackgroundSourcePreparation));
        assert!(!policy.paused(AutomaticWorkKind::BackgroundStreamMediaAnalysis));

        let theme_only = pause_selected_for_settings_import(None, &[]).unwrap();
        assert!(theme_only.is_none());
        let ordinary = PersistedPolicy {
            schema_version: 1,
            ..PersistedPolicy::default()
        };
        assert!(!ordinary.review_required);
        assert!(!ordinary.paused(AutomaticWorkKind::BackgroundStreamMediaAnalysis));
    }

    #[test]
    fn malformed_policy_fails_closed() {
        let store = empty_store();
        store
            .set_json_preference(PREFERENCE_KEY, &json!({"schemaVersion":99}))
            .unwrap();
        for kind in [
            AutomaticWorkKind::MetadataRetagAnalysis,
            AutomaticWorkKind::AssetAutotag,
            AutomaticWorkKind::BackgroundSourcePreparation,
            AutomaticWorkKind::BackgroundStreamMediaAnalysis,
        ] {
            assert!(is_paused(&store, kind));
        }
        let state = state(&store).unwrap();
        assert!(state.review_required);
        assert!(state.warning.is_some());

        let repaired = review(&store, ResumeAutomaticWork::default()).unwrap();
        assert!(repaired.warning.is_none());
        assert!(!repaired.review_required);
        assert!(is_paused(&store, AutomaticWorkKind::MetadataRetagAnalysis));
    }

    #[test]
    fn legacy_restore_marker_without_stream_analysis_bit_stays_paused_until_explicit_review() {
        let store = empty_store();
        assert!(!is_paused(
            &store,
            AutomaticWorkKind::BackgroundStreamMediaAnalysis
        ));

        store
            .set_json_preference(
                PREFERENCE_KEY,
                &json!({
                    "schemaVersion": 1,
                    "reviewRequired": true,
                    "snapshotId": "old-snapshot",
                    "metadataRetagAnalysisPaused": true,
                    "assetAutotagPaused": true,
                    "backgroundSourcePreparationPaused": true
                }),
            )
            .unwrap();
        let legacy_review_marker = read_policy(&store).unwrap();
        assert!(legacy_review_marker
            .background_stream_media_analysis_paused
            .is_none());
        assert!(legacy_review_marker.paused(AutomaticWorkKind::BackgroundStreamMediaAnalysis));
        assert!(ensure_stream_media_analysis_allowed(&store, true).is_err());
        assert!(ensure_stream_media_analysis_allowed(&store, false).is_ok());

        // A prior review of the old categories does not implicitly authorize
        // the newly introduced provider fetch.
        store
            .set_json_preference(
                PREFERENCE_KEY,
                &json!({
                    "schemaVersion": 1,
                    "reviewRequired": false,
                    "snapshotId": "old-snapshot",
                    "metadataRetagAnalysisPaused": true,
                    "assetAutotagPaused": true,
                    "backgroundSourcePreparationPaused": true
                }),
            )
            .unwrap();
        assert!(is_paused(
            &store,
            AutomaticWorkKind::BackgroundStreamMediaAnalysis
        ));

        review(
            &store,
            ResumeAutomaticWork {
                background_stream_media_analysis: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!is_paused(
            &store,
            AutomaticWorkKind::BackgroundStreamMediaAnalysis
        ));
    }
}
