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
            while let Some(id) = receiver.recv().await {
                if let Ok(Some(asset)) = worker_store
                    .list_assets()
                    .map(|assets| assets.into_iter().find(|asset| asset.id == id))
                {
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
                        let updated = media::analyze_and_retag_asset(&asset, &settings).await?;
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
        let assets = self.store.list_assets()?;
        let records = self.store.list_analysis_records()?;
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "Analysis queue lock unavailable")?;
        let mut count = 0;
        for asset in assets {
            if asset.processing == "external-readonly" {
                continue;
            }
            if asset_ids
                .as_ref()
                .is_some_and(|ids| !ids.contains(&asset.id))
                || pending.contains(&asset.id)
            {
                continue;
            }
            if !std::path::Path::new(&asset.path).is_file() {
                continue;
            }
            if !force
                && records.iter().any(|record| {
                    record["assetId"] == asset.id
                        && record["inputHash"] == asset.hash
                        && record["version"] == VERSION
                        && record["status"] == "completed"
                })
            {
                continue;
            }
            if !force && asset.track.bpm.is_some() && asset.track.key.is_some() {
                continue;
            }
            self.store.save_analysis_record(
                &asset.id,
                &asset.hash,
                VERSION,
                "queued",
                &json!({}),
            )?;
            pending.insert(asset.id.clone());
            if self.sender.send(asset.id.clone()).is_err() {
                pending.remove(&asset.id);
                return Err("Analysis worker is not running".into());
            }
            count += 1;
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
}

fn analysis_payload(asset: &LocalAsset) -> Value {
    json!({"bpm":asset.track.bpm,"key":asset.track.key,"bpmAnalysis":asset.track.provenance.get("bpmAnalysis"),"keyAnalysis":asset.track.provenance.get("keyAnalysis")})
}
