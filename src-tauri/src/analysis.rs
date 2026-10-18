//! Serial local analysis and recoverable tag refreshes for registered assets.
use crate::{media, models::LocalAsset, Store};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};
use tauri::Emitter;
use tokio::sync::mpsc;

pub const VERSION: &str = media::ANALYSIS_VERSION;

pub struct AnalysisManager {
    store: Arc<Store>,
    pending: Arc<Mutex<HashSet<String>>>,
    sender: mpsc::UnboundedSender<String>,
}

impl AnalysisManager {
    pub fn new(app: tauri::AppHandle, store: Arc<Store>) -> Self {
        let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let worker_store = store.clone();
        let worker_pending = pending.clone();
        tauri::async_runtime::spawn(async move {
            let perf_trace = std::env::var_os("BURRATA_PERF_TRACE").is_some();
            let mut item_index = 0usize;
            while let Some(id) = receiver.recv().await {
                item_index += 1;
                let lookup_started = std::time::Instant::now();
                let asset = worker_store.get_asset_by_id(&id).ok().flatten();
                if perf_trace {
                    eprintln!(
                        "[perf] analysis.worker lookup item={} lookup=asset_id found={} elapsed_ms={}",
                        item_index,
                        asset.is_some(),
                        lookup_started.elapsed().as_millis()
                    );
                }
                if let Some(asset) = asset {
                    let input_hash = asset.hash.clone();
                    let _ = worker_store.save_analysis_record(
                        &id,
                        &input_hash,
                        VERSION,
                        "running",
                        &json!({}),
                    );
                    let _ = app.emit(
                        "analysis-progress",
                        json!({"assetId":id,"status":"running"}),
                    );
                    let result = async {
                        let settings = worker_store.settings()?;
                        let analysis_started = std::time::Instant::now();
                        let updated_result =
                            media::analyze_and_retag_asset(&asset, &settings).await;
                        if perf_trace {
                            eprintln!(
                                "[perf] analysis.worker media item={} outcome={} elapsed_ms={}",
                                item_index,
                                if updated_result.is_ok() {
                                    "ok"
                                } else {
                                    "error"
                                },
                                analysis_started.elapsed().as_millis()
                            );
                        }
                        let updated = updated_result?;
                        worker_store.save_analyzed_asset(
                            &updated,
                            &input_hash,
                            VERSION,
                            &analysis_payload(&updated),
                        )?;
                        if let Some(id) = updated.track.provenance["retagJournalId"].as_str() {
                            let _ = media::acknowledge_retag_journal(&settings, id);
                        }
                        Ok::<LocalAsset, String>(updated)
                    }
                    .await;
                    match result {
                        Ok(updated) => {
                            let _ = app.emit("library-updated", updated.track.clone());
                            let _ = app.emit("analysis-progress", json!({"assetId":id,"status":"completed","bpm":updated.track.bpm,"key":updated.track.key}));
                        }
                        Err(error) => {
                            let _ = worker_store.save_analysis_record(
                                &id,
                                &input_hash,
                                VERSION,
                                "failed",
                                &json!({"error":error}),
                            );
                            let _ = app.emit(
                                "analysis-progress",
                                json!({"assetId":id,"status":"failed","error":error}),
                            );
                        }
                    }
                }
                if let Ok(mut pending) = worker_pending.lock() {
                    pending.remove(&id);
                }
            }
        });
        Self {
            store,
            pending,
            sender,
        }
    }

    pub fn queue(&self, asset_ids: Option<Vec<String>>, force: bool) -> Result<usize, String> {
        let perf_trace = std::env::var_os("BURRATA_PERF_TRACE").is_some();
        let queue_started = std::time::Instant::now();
        let assets_started = std::time::Instant::now();
        let assets = if let Some(ids) = asset_ids.as_ref().filter(|ids| ids.len() > 200) {
            let unique_ids = ids.iter().cloned().collect::<HashSet<_>>();
            let mut candidates = Vec::new();
            for chunk in unique_ids.into_iter().collect::<Vec<_>>().chunks(200) {
                candidates.extend(self.store.list_analysis_queue_assets(
                    Some(chunk),
                    force,
                    VERSION,
                )?);
            }
            candidates.sort_by(|left, right| right.rowid.cmp(&left.rowid));
            candidates
        } else {
            self.store
                .list_analysis_queue_assets(asset_ids.as_deref(), force, VERSION)?
        };
        let assets_elapsed = assets_started.elapsed();
        let asset_count = assets.len();
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "Analysis queue lock unavailable")?;
        let mut count = 0;
        for asset in assets {
            if pending.contains(&asset.id) {
                continue;
            }
            let Some(hydrated) = self.store.get_asset_by_id(&asset.id)? else {
                continue;
            };
            if !force && hydrated.track.bpm.is_some() && hydrated.track.key.is_some() {
                continue;
            }
            if !std::path::Path::new(&hydrated.path).is_file() {
                continue;
            }
            self.store.save_analysis_record(
                &hydrated.id,
                &hydrated.hash,
                VERSION,
                "queued",
                &json!({}),
            )?;
            pending.insert(hydrated.id.clone());
            if self.sender.send(hydrated.id.clone()).is_err() {
                pending.remove(&hydrated.id);
                return Err("Analysis worker is not running".into());
            }
            count += 1;
        }
        if perf_trace {
            eprintln!(
                "[perf] analysis.queue candidate_assets={} candidate_query_ms={} queued={} total_ms={}",
                asset_count,
                assets_elapsed.as_millis(),
                count,
                queue_started.elapsed().as_millis()
            );
        }
        Ok(count)
    }

    pub fn status(&self) -> Result<Vec<Value>, String> {
        let mut seen = HashSet::new();
        Ok(self
            .store
            .list_analysis_records()?
            .into_iter()
            .filter(|record| seen.insert(record["assetId"].as_str().unwrap_or_default().to_owned()))
            .collect())
    }

    pub fn status_for_assets(&self, asset_ids: &[String]) -> Result<Vec<Value>, String> {
        let mut seen = HashSet::new();
        Ok(self
            .store
            .list_analysis_records_for_assets(asset_ids)?
            .into_iter()
            .filter(|record| seen.insert(record["assetId"].as_str().unwrap_or_default().to_owned()))
            .collect())
    }
}

fn analysis_payload(asset: &LocalAsset) -> Value {
    json!({"bpm":asset.track.bpm,"key":asset.track.key,"bpmAnalysis":asset.track.provenance.get("bpmAnalysis"),"keyAnalysis":asset.track.provenance.get("keyAnalysis")})
}
