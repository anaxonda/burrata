//! Explicit directory roots, bounded file browsing and cancellable metadata indexing.
//! This registry is independent of both Music and playlist discovery settings.
use crate::{
    media,
    models::{LocalAsset, Track},
    storage::Store,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

const ROOTS_KEY: &str = "directoryBrowserRoots.v1";
const MAX_ENTRIES: usize = 100_000;
const MAX_ROOTS: usize = 32;
const MAX_DEPTH: usize = 64;
type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryRoot {
    pub id: String,
    pub name: String,
    pub path: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryBreadcrumb {
    pub name: String,
    pub relative_path: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildDirectory {
    pub name: String,
    pub relative_path: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryChildren {
    pub root_id: String,
    pub relative_path: String,
    pub directories: Vec<ChildDirectory>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryFile {
    pub path: String,
    pub name: String,
    pub asset_id: Option<String>,
    pub track: Option<Track>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryPage {
    pub root_id: String,
    pub path: String,
    pub relative_path: String,
    pub breadcrumbs: Vec<DirectoryBreadcrumb>,
    pub directories: Vec<ChildDirectory>,
    pub files: Vec<DirectoryFile>,
    pub total: usize,
    /// Present when the request already materialized the whole active filtered relation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_availability: Option<[bool; 10]>,
    pub offset: usize,
    pub limit: usize,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryBrowseOptions {
    pub query: Option<String>,
    pub sort_field: Option<String>,
    pub sort_direction: Option<String>,
    pub duration_min: Option<f64>,
    pub duration_max: Option<f64>,
}
impl DirectoryBrowseOptions {
    fn validate(&self) -> Result<(&str, bool)> {
        if self.query.as_ref().is_some_and(|query| query.len() > 512) {
            return Err("Directory query exceeds 512 bytes".into());
        }
        let field = self.sort_field.as_deref().unwrap_or("filename");
        if !matches!(field, "filename" | "sourceOrder")
            && crate::track_sort_sql::expression(field, "metadata").is_none()
        {
            return Err("Unsupported directory sort field".into());
        }
        let descending = match self.sort_direction.as_deref().unwrap_or("asc") {
            "asc" => false,
            "desc" => true,
            _ => return Err("Unsupported directory sort direction".into()),
        };
        if [self.duration_min, self.duration_max]
            .into_iter()
            .flatten()
            .any(|v| !v.is_finite() || v < 0.0)
            || matches!((self.duration_min,self.duration_max),(Some(min),Some(max)) if min.floor()>max.floor())
        {
            return Err("Directory duration bounds are invalid".into());
        }
        Ok((field, descending))
    }
}
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ScanState {
    Running,
    Completed,
    Cancelled,
    Failed,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryScan {
    pub job_id: String,
    pub root_id: String,
    pub state: ScanState,
    pub completed: usize,
    pub indexed: usize,
    pub reused: usize,
    pub inspected: usize,
    pub failed: usize,
    pub total: Option<usize>,
    pub error: Option<String>,
}
/// A folder-wide availability computation. `fields` uses track_sort_sql's order:
/// title, artist, album, duration, releaseDate, dateAdded, bpm, key, plays, views.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorySortAvailabilityJob {
    pub job_id: String,
    pub root_id: String,
    pub relative_path: String,
    pub state: ScanState,
    pub completed: usize,
    pub total: Option<usize>,
    pub fields: Option<[bool; 10]>,
    pub error: Option<String>,
}
struct SortAvailabilityJob {
    status: DirectorySortAvailabilityJob,
    cancel: watch::Sender<bool>,
}
struct ScanJob {
    status: DirectoryScan,
    cancel: watch::Sender<bool>,
}
struct Inner {
    store: Arc<Store>,
    roots_gate: Mutex<()>,
    jobs: Mutex<HashMap<String, ScanJob>>,
    availability_jobs: Mutex<HashMap<String, SortAvailabilityJob>>,
}
#[derive(Clone)]
pub struct DirectoryBrowser {
    inner: Arc<Inner>,
}
impl DirectoryBrowser {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                roots_gate: Mutex::new(()),
                jobs: Mutex::new(HashMap::new()),
                availability_jobs: Mutex::new(HashMap::new()),
            }),
        }
    }
    pub fn roots(&self) -> Result<Vec<DirectoryRoot>> {
        let roots = self
            .inner
            .store
            .get_json_preference(ROOTS_KEY)?
            .map(serde_json::from_value::<Vec<DirectoryRoot>>)
            .transpose()
            .map_err(|_| "Directory root registry is invalid")?
            .unwrap_or_default();
        if roots.len() > MAX_ROOTS {
            return Err("Directory root registry exceeded its limit".into());
        }
        Ok(roots)
    }
    pub fn add_root(&self, path: &str) -> Result<DirectoryRoot> {
        let _gate = self
            .inner
            .roots_gate
            .lock()
            .map_err(|_| "Directory registry is unavailable")?;
        let path = Path::new(path);
        if !path.is_absolute() {
            return Err("Choose an absolute directory path".into());
        }
        reject_symlinks(path)?;
        let canonical = fs::canonicalize(path).map_err(|_| "Directory is unavailable")?;
        if !canonical.is_dir() {
            return Err("Choose a directory".into());
        }
        let value = path_text(&canonical)?;
        let mut roots = self.roots()?;
        if let Some(root) = roots.iter().find(|root| root.path == value) {
            return Ok(root.clone());
        }
        if roots.len() >= MAX_ROOTS {
            return Err("At most 32 directory roots can be registered".into());
        }
        let root = DirectoryRoot {
            id: uuid::Uuid::new_v4().to_string(),
            name: canonical
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("Directory")
                .to_owned(),
            path: value,
        };
        roots.push(root.clone());
        self.save_roots(&roots)?;
        Ok(root)
    }
    pub fn remove_root(&self, root_id: &str) -> Result<()> {
        let _gate = self
            .inner
            .roots_gate
            .lock()
            .map_err(|_| "Directory registry is unavailable")?;
        let mut roots = self.roots()?;
        let before = roots.len();
        roots.retain(|root| root.id != root_id);
        if roots.len() == before {
            return Err("Directory root is not registered".into());
        }
        self.save_roots(&roots)?;
        for job in self
            .inner
            .jobs
            .lock()
            .map_err(|_| "Directory index registry is unavailable")?
            .values()
        {
            if job.status.root_id == root_id {
                job.cancel.send_replace(true);
            }
        }
        for job in self
            .inner
            .availability_jobs
            .lock()
            .map_err(|_| "Directory availability registry is unavailable")?
            .values()
        {
            if job.status.root_id == root_id {
                job.cancel.send_replace(true);
            }
        }
        Ok(())
    }
    fn save_roots(&self, roots: &[DirectoryRoot]) -> Result<()> {
        self.inner.store.set_json_preference(
            ROOTS_KEY,
            &serde_json::to_value(roots).map_err(|_| "Cannot encode directory roots")?,
        )
    }
    fn root(&self, id: &str) -> Result<DirectoryRoot> {
        self.roots()?
            .into_iter()
            .find(|root| root.id == id)
            .ok_or_else(|| "Directory root is not registered".into())
    }
    /// Directory-only tree expansion. No asset lookup or media inspection is performed.
    pub fn children(&self, root_id: &str, relative_path: &str) -> Result<DirectoryChildren> {
        if relative_path.len() > 4096 {
            return Err("Directory path exceeds 4096 bytes".into());
        }
        let root = self.root(root_id)?;
        let path = contained_path(&root, relative_path)?;
        let directories = immediate_directories(&root, &path, MAX_ENTRIES)?;
        Ok(DirectoryChildren {
            root_id: root.id.clone(),
            relative_path: relative_text(Path::new(&root.path), &path)?,
            directories,
        })
    }
    pub fn browse(
        &self,
        root_id: &str,
        relative_path: &str,
        offset: usize,
        limit: usize,
    ) -> Result<DirectoryPage> {
        self.browse_with_options(
            root_id,
            relative_path,
            offset,
            limit,
            &DirectoryBrowseOptions::default(),
        )
    }
    pub fn browse_with_options(
        &self,
        root_id: &str,
        relative_path: &str,
        offset: usize,
        limit: usize,
        options: &DirectoryBrowseOptions,
    ) -> Result<DirectoryPage> {
        let (sort_field, descending) = options.validate()?;
        if limit == 0 || limit > 200 || offset > MAX_ENTRIES {
            return Err("Directory page bounds are invalid".into());
        }
        let root = self.root(root_id)?;
        let path = contained_path(&root, relative_path)?;
        if !path.is_dir() {
            return Err("Choose a directory".into());
        }
        let mut directories = Vec::new();
        let mut files = Vec::new();
        let mut count = 0;
        for entry in fs::read_dir(&path).map_err(|_| "Directory cannot be read")? {
            count += 1;
            if count > MAX_ENTRIES {
                return Err(
                    "Directory has more than 100000 immediate entries; choose a subdirectory"
                        .into(),
                );
            }
            let entry = entry.map_err(|_| "Directory entry cannot be read")?;
            let kind = entry
                .file_type()
                .map_err(|_| "Directory entry cannot be read")?;
            if kind.is_symlink() {
                continue;
            }
            let child = entry.path();
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "Directory contains a non-UTF8 filename")?;
            if burrata_working_artifact(&child) {
                continue;
            }
            if kind.is_dir() {
                directories.push(ChildDirectory {
                    name,
                    relative_path: relative_text(Path::new(&root.path), &child)?,
                });
            } else if kind.is_file() && supported_audio(&child) {
                files.push((name, child));
            }
        }
        directories.sort_by(|a, b| a.name.cmp(&b.name));
        files.sort_by(|a, b| a.0.cmp(&b.0));
        // Filename-only browsing preserves the existing page-only hydration cost.
        let global = options
            .query
            .as_ref()
            .is_some_and(|query| !query.trim().is_empty())
            || !matches!(sort_field, "filename" | "sourceOrder")
            || options.duration_min.is_some()
            || options.duration_max.is_some();
        if !global && descending {
            files.reverse();
        }
        let total = files.len();
        let chosen = if global {
            files
        } else {
            files.into_iter().skip(offset).take(limit).collect()
        };
        let paths = chosen
            .iter()
            .map(|(_, path)| path_text(path))
            .collect::<Result<Vec<_>>>()?;
        let mut assets = HashMap::new();
        for asset in self.inner.store.list_directory_assets_for_paths(&paths)? {
            assets.entry(asset.path.clone()).or_insert(asset);
        }
        let mut page_files = chosen
            .into_iter()
            .map(|(name, path)| {
                let relative = relative_text(Path::new(&root.path), &path)?;
                let path = contained_path(&root, &relative)?;
                let text = path_text(&path)?;
                let asset = assets.remove(&text);
                let asset = asset.filter(|asset| {
                    fingerprint(&path).ok().as_ref()
                        == asset.track.provenance.get("directoryBrowserFingerprint")
                });
                let (asset_id, track) = match asset {
                    Some(asset) => {
                        let mut track = asset.track;
                        track.asset_id = Some(asset.id.clone());
                        track.local_path = Some(text.clone());
                        track.local_availability = "available".into();
                        (Some(asset.id), Some(track))
                    }
                    None => (None, None),
                };
                Ok(DirectoryFile {
                    path: text,
                    name,
                    asset_id,
                    track,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let (total, sort_availability) = if global {
            let (selected, filtered_total, availability) =
                sort_directory_files(page_files, options, offset, limit)?;
            page_files = selected;
            (filtered_total, Some(availability))
        } else {
            (total, None)
        };
        Ok(DirectoryPage {
            root_id: root.id.clone(),
            path: path_text(&path)?,
            relative_path: relative_text(Path::new(&root.path), &path)?,
            breadcrumbs: breadcrumbs(&root, &path)?,
            directories,
            files: page_files,
            total,
            sort_availability,
            offset,
            limit,
        })
    }
    pub fn start_sort_availability(
        &self,
        root_id: &str,
        relative_path: &str,
        options: DirectoryBrowseOptions,
    ) -> Result<DirectorySortAvailabilityJob> {
        options.validate()?;
        let root = self.root(root_id)?;
        let folder = contained_path(&root, relative_path)?;
        if !folder.is_dir() {
            return Err("Choose a directory".into());
        }
        let mut jobs = self
            .inner
            .availability_jobs
            .lock()
            .map_err(|_| "Directory availability registry is unavailable")?;
        if jobs.len() >= 32 {
            let terminal = jobs
                .iter()
                .filter_map(|(id, job)| {
                    (job.status.state != ScanState::Running).then_some(id.clone())
                })
                .collect::<Vec<_>>();
            for id in terminal {
                if jobs.len() <= 31 {
                    break;
                }
                jobs.remove(&id);
            }
        }
        if jobs.len() >= 32 {
            return Err("Too many directory availability requests are still running".into());
        }
        let job_id = uuid::Uuid::new_v4().to_string();
        let (cancel, receiver) = watch::channel(false);
        let status = DirectorySortAvailabilityJob {
            job_id: job_id.clone(),
            root_id: root.id.clone(),
            relative_path: relative_text(Path::new(&root.path), &folder)?,
            state: ScanState::Running,
            completed: 0,
            total: None,
            fields: None,
            error: None,
        };
        jobs.insert(
            job_id.clone(),
            SortAvailabilityJob {
                status: status.clone(),
                cancel,
            },
        );
        drop(jobs);
        let manager = self.clone();
        tokio::spawn(async move {
            manager
                .run_sort_availability(root, folder, options, job_id, receiver)
                .await;
        });
        Ok(status)
    }
    pub fn get_sort_availability(&self, job_id: &str) -> Result<DirectorySortAvailabilityJob> {
        self.inner
            .availability_jobs
            .lock()
            .map_err(|_| "Directory availability registry is unavailable")?
            .get(job_id)
            .map(|job| job.status.clone())
            .ok_or_else(|| "Directory availability job was not found".into())
    }
    pub fn cancel_sort_availability(&self, job_id: &str) -> Result<DirectorySortAvailabilityJob> {
        let jobs = self
            .inner
            .availability_jobs
            .lock()
            .map_err(|_| "Directory availability registry is unavailable")?;
        let job = jobs
            .get(job_id)
            .ok_or("Directory availability job was not found")?;
        job.cancel.send_replace(true);
        Ok(job.status.clone())
    }
    fn update_sort_availability(
        &self,
        job_id: &str,
        change: impl FnOnce(&mut DirectorySortAvailabilityJob),
    ) {
        if let Ok(mut jobs) = self.inner.availability_jobs.lock() {
            if let Some(job) = jobs.get_mut(job_id) {
                change(&mut job.status);
            }
        }
    }
    async fn run_sort_availability(
        &self,
        root: DirectoryRoot,
        folder: PathBuf,
        options: DirectoryBrowseOptions,
        job_id: String,
        mut cancel: watch::Receiver<bool>,
    ) {
        let walk_root = root.clone();
        let walk_folder = folder.clone();
        let walked =
            tokio::task::spawn_blocking(move || immediate_audio_files(&walk_root, &walk_folder))
                .await;
        let paths = match walked {
            Ok(Ok(paths)) => paths,
            Ok(Err(error)) => {
                self.finish_sort_availability(&job_id, &cancel, Err(error));
                return;
            }
            Err(_) => {
                self.finish_sort_availability(
                    &job_id,
                    &cancel,
                    Err("Directory summary worker failed".into()),
                );
                return;
            }
        };
        self.update_sort_availability(&job_id, |job| job.total = Some(paths.len()));
        let mut found = [false; 10];
        'batches: for chunk in paths.chunks(200) {
            if *cancel.borrow() {
                break;
            }
            let batch = chunk
                .iter()
                .filter_map(|path| path_text(path).ok())
                .collect::<Vec<_>>();
            let store = self.inner.store.clone();
            let loaded =
                tokio::task::spawn_blocking(move || store.list_directory_assets_for_paths(&batch));
            let assets = tokio::select! { result = loaded => match result { Ok(Ok(assets)) => assets, _ => { self.finish_sort_availability(&job_id, &cancel, Err("Directory metadata summary failed".into())); return; } }, _ = cancel.changed() => break 'batches };
            let by_path = assets
                .into_iter()
                .map(|asset| (asset.path.clone(), asset))
                .collect::<HashMap<_, _>>();
            let mut completed = 0;
            for path in chunk {
                completed += 1;
                let Ok(text) = path_text(path) else {
                    continue;
                };
                let Some(asset) = by_path.get(&text) else {
                    continue;
                };
                let Ok(current) = fingerprint(path) else {
                    continue;
                };
                if asset.track.provenance.get("directoryBrowserFingerprint") != Some(&current) {
                    continue;
                }
                let track = &asset.track;
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("");
                if !directory_file_matches_availability_filters(name, track, &options) {
                    continue;
                }
                let Ok(metadata) = serde_json::to_value(track) else {
                    continue;
                };
                crate::track_sort_sql::extend_available_fields(&metadata, &mut found);
                if found.iter().all(|present| *present) {
                    break;
                }
            }
            self.update_sort_availability(&job_id, |job| {
                job.completed += completed;
            });
            if found.iter().all(|present| *present) {
                break;
            }
        }
        if *cancel.borrow() {
            self.finish_sort_availability(&job_id, &cancel, Err("cancelled".into()));
        } else {
            self.finish_sort_availability(&job_id, &cancel, Ok(found));
        }
    }
    fn finish_sort_availability(
        &self,
        job_id: &str,
        cancel: &watch::Receiver<bool>,
        result: Result<[bool; 10]>,
    ) {
        self.update_sort_availability(job_id, |job| match result {
            Ok(fields) => {
                job.state = ScanState::Completed;
                job.fields = Some(fields);
            }
            Err(error) if *cancel.borrow() || error == "cancelled" => {
                job.state = ScanState::Cancelled;
            }
            Err(error) => {
                job.state = ScanState::Failed;
                job.error = Some(error);
            }
        });
    }
    pub fn start_scan(&self, root_id: &str) -> Result<DirectoryScan> {
        let root = self.root(root_id)?;
        contained_path(&root, "")?;
        let mut jobs = self
            .inner
            .jobs
            .lock()
            .map_err(|_| "Directory index registry is unavailable")?;
        if jobs
            .values()
            .any(|job| job.status.state == ScanState::Running)
        {
            return Err("A directory index is already running; cancel or wait for it".into());
        }
        if jobs.len() >= 32 {
            jobs.retain(|_, job| job.status.state == ScanState::Running);
        }
        let (cancel, receiver) = watch::channel(false);
        let status = DirectoryScan {
            job_id: uuid::Uuid::new_v4().to_string(),
            root_id: root.id.clone(),
            state: ScanState::Running,
            completed: 0,
            indexed: 0,
            reused: 0,
            inspected: 0,
            failed: 0,
            total: None,
            error: None,
        };
        jobs.insert(
            status.job_id.clone(),
            ScanJob {
                status: status.clone(),
                cancel,
            },
        );
        drop(jobs);
        let manager = self.clone();
        let job_id = status.job_id.clone();
        tokio::spawn(async move {
            manager.run_scan(root, job_id, receiver).await;
        });
        Ok(status)
    }
    pub fn scan_status(&self, job_id: &str) -> Result<DirectoryScan> {
        self.inner
            .jobs
            .lock()
            .map_err(|_| "Directory index registry is unavailable")?
            .get(job_id)
            .map(|job| job.status.clone())
            .ok_or_else(|| "Directory index job was not found".into())
    }
    pub fn cancel_scan(&self, job_id: &str) -> Result<DirectoryScan> {
        let jobs = self
            .inner
            .jobs
            .lock()
            .map_err(|_| "Directory index registry is unavailable")?;
        let job = jobs
            .get(job_id)
            .ok_or("Directory index job was not found")?;
        job.cancel.send_replace(true);
        Ok(job.status.clone())
    }
    fn update(&self, job_id: &str, change: impl FnOnce(&mut DirectoryScan)) {
        if let Ok(mut jobs) = self.inner.jobs.lock() {
            if let Some(job) = jobs.get_mut(job_id) {
                change(&mut job.status);
            }
        }
    }
    async fn run_scan(
        &self,
        root: DirectoryRoot,
        job_id: String,
        mut cancel: watch::Receiver<bool>,
    ) {
        let walk_root = root.clone();
        let walk_cancel = cancel.clone();
        let found =
            tokio::task::spawn_blocking(move || collect_audio(&walk_root, &walk_cancel)).await;
        let files = match found {
            Ok(Ok(files)) => files,
            Ok(Err(error)) => {
                let cancelled = *cancel.borrow();
                self.update(&job_id, |job| {
                    job.state = if cancelled {
                        ScanState::Cancelled
                    } else {
                        ScanState::Failed
                    };
                    job.error = (!cancelled).then_some(error);
                });
                return;
            }
            Err(_) => {
                self.update(&job_id, |job| {
                    job.state = ScanState::Failed;
                    job.error = Some("Directory discovery worker failed".into());
                });
                return;
            }
        };
        self.update(&job_id, |job| job.total = Some(files.len()));
        let settings = match self.inner.store.settings() {
            Ok(settings) => settings,
            Err(_) => {
                self.update(&job_id, |job| {
                    job.state = ScanState::Failed;
                    job.error = Some("Audio inspection settings are unavailable".into());
                });
                return;
            }
        };
        'chunks: for chunk in files.chunks(200) {
            if *cancel.borrow() {
                break;
            }
            let paths = match chunk
                .iter()
                .map(|path| path_text(path))
                .collect::<Result<Vec<_>>>()
            {
                Ok(paths) => paths,
                Err(_) => {
                    self.update(&job_id, |job| {
                        job.state = ScanState::Failed;
                        job.error = Some("Directory path is not UTF8".into());
                    });
                    return;
                }
            };
            let store = self.inner.store.clone();
            let loading = tokio::task::spawn_blocking(move || store.list_assets_for_paths(&paths));
            let loaded = tokio::select! {result=loading=>result,_=cancel.changed()=>break 'chunks};
            let mut existing = match loaded {
                Ok(Ok(assets)) => {
                    let mut by_path = HashMap::new();
                    for asset in assets {
                        by_path.entry(asset.path.clone()).or_insert(asset);
                    }
                    by_path
                }
                _ => {
                    self.update(&job_id, |job| {
                        job.state = ScanState::Failed;
                        job.error = Some("Existing audio metadata is unavailable".into());
                    });
                    return;
                }
            };
            for file in chunk {
                if *cancel.borrow() {
                    break 'chunks;
                }
                let relative = match relative_text(Path::new(&root.path), file) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let before = match contained_path(&root, &relative).and_then(|_| fingerprint(file))
                {
                    Ok(value) => value,
                    Err(_) => {
                        self.update(&job_id, |job| {
                            job.completed += 1;
                            job.failed += 1;
                        });
                        continue;
                    }
                };
                let known = path_text(file).ok().and_then(|path| existing.remove(&path));
                let reused = known.is_some();
                let result = if let Some(asset) = known {
                    let manager = self.clone();
                    let root = root.clone();
                    let relative = relative.clone();
                    let before = before.clone();
                    let cancel_check = cancel.clone();
                    let pending = tokio::task::spawn_blocking(move || {
                        manager.reuse_existing_asset(
                            &root,
                            &relative,
                            &before,
                            asset,
                            &cancel_check,
                            media::hash_file,
                        )
                    });
                    let result =
                        tokio::select! {result=pending=>Some(result),_=cancel.changed()=>None};
                    let Some(result) = result else {
                        break 'chunks;
                    };
                    result
                        .unwrap_or_else(|_| Err("Existing audio verification worker failed".into()))
                } else {
                    let inspection = media::inspect_local_file(file, &settings);
                    let inspected =
                        tokio::select! {result=inspection=>Some(result),_=cancel.changed()=>None};
                    let Some(inspected) = inspected else {
                        break 'chunks;
                    };
                    if *cancel.borrow() {
                        break 'chunks;
                    }
                    match inspected {
                        Ok(track) => {
                            self.commit_inspection(&root, &relative, &before, track, &cancel)
                        }
                        Err(_) => Err("Audio metadata could not be inspected".into()),
                    }
                };
                if *cancel.borrow() {
                    break 'chunks;
                }
                self.update(&job_id,|job|{job.completed+=1;if result.is_ok(){job.indexed+=1;if reused{job.reused+=1;}else{job.inspected+=1;}}else{job.failed+=1;job.error=Some("Some files could not be indexed; changed existing audio requires review".into());}});
            }
        }
        self.update(&job_id, |job| {
            job.state = if *cancel.borrow() {
                ScanState::Cancelled
            } else {
                ScanState::Completed
            }
        });
    }
    /// Existing registered assets keep their recording, tags, chapter and codec facts.
    /// A legacy fingerprint requires one full hash verification, never another probe.
    fn reuse_existing_asset(
        &self,
        root: &DirectoryRoot,
        relative: &str,
        before: &serde_json::Value,
        mut asset: LocalAsset,
        cancel: &watch::Receiver<bool>,
        hash_file: impl FnOnce(&Path) -> Result<String>,
    ) -> Result<()> {
        if *cancel.borrow() {
            return Err("Directory index was cancelled".into());
        }
        let path = contained_path(root, relative)?;
        if asset.path != path_text(&path)? || fingerprint(&path)? != *before {
            return Err("Audio changed during verification".into());
        }
        let current = asset.track.provenance.get("directoryBrowserFingerprint") == Some(before);
        if !current && hash_file(&path)? != asset.hash {
            return Err("Changed existing audio requires review before replacement".into());
        }
        let _gate = self
            .inner
            .roots_gate
            .lock()
            .map_err(|_| "Directory registry is unavailable")?;
        if *cancel.borrow() || self.root(&root.id)?.path != root.path {
            return Err("Directory index was cancelled".into());
        }
        if contained_path(root, relative)? != path || fingerprint(&path)? != *before {
            return Err("Audio changed during verification".into());
        }
        if current {
            return Ok(());
        }
        if !asset.track.provenance.is_object() {
            asset.track.provenance = serde_json::json!({});
        }
        asset.track.provenance["directoryBrowserFingerprint"] = before.clone();
        self.inner.store.save_asset(&asset)
    }
    fn commit_inspection(
        &self,
        root: &DirectoryRoot,
        relative: &str,
        before: &serde_json::Value,
        mut track: Track,
        cancel: &watch::Receiver<bool>,
    ) -> Result<()> {
        let _gate = self
            .inner
            .roots_gate
            .lock()
            .map_err(|_| "Directory registry is unavailable")?;
        if *cancel.borrow() || self.root(&root.id)?.path != root.path {
            return Err("Directory index was cancelled".into());
        }
        let path = contained_path(root, relative)?;
        if fingerprint(&path)? != *before {
            return Err("Audio changed during inspection".into());
        }
        let hash = track
            .id
            .strip_prefix("local:")
            .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or("Audio identity was not recognized")?
            .to_owned();
        let text = path_text(&path)?;
        let existing = self.inner.store.get_asset_by_path(&text)?;
        if existing.as_ref().is_some_and(|asset| asset.hash != hash) {
            return Err("Changed existing audio requires review before replacement".into());
        }
        track.id = format!("local:{hash}");
        track.provider = "local".into();
        track.provider_id = hash.clone();
        track.source_url = track.id.clone();
        track.local_path = Some(text.clone());
        if !track.provenance.is_object() {
            track.provenance = serde_json::json!({});
        }
        track.provenance["directoryBrowserFingerprint"] = before.clone();
        let codec = track.provenance["codec"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned();
        let bitrate = track.provenance["bitrateKbps"].as_f64();
        let asset = LocalAsset {
            id: existing
                .map(|asset| asset.id)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            recording_id: track.id.clone(),
            path: text,
            hash,
            source_codec: codec.clone(),
            final_codec: codec,
            source_bitrate: bitrate,
            final_bitrate: bitrate,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_seconds: track.duration_seconds,
            size_bytes: fs::metadata(&path).ok().map(|value| value.len()),
            bitrate_origin: None,
            processing: "existing-local-file".into(),
            track,
        };
        self.inner.store.save_asset(&asset)
    }
}
// These names are created by media.rs; ordinary hidden audio remains visible.
fn burrata_working_artifact(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name == ".burrata-staging"
                || name.starts_with(".burrata-retag-")
                || name.starts_with(".burrata-autotag-")
        })
}
/// A request-owned relation: no cached metadata can outlive filesystem freshness checks.
/// Shared SQL functions keep dates, musical keys, counts and Unicode ordering consistent.
fn directory_file_matches_availability_filters(
    name: &str,
    track: &Track,
    options: &DirectoryBrowseOptions,
) -> bool {
    let query = options.query.as_deref().unwrap_or("").trim().to_lowercase();
    if !query.is_empty() {
        let mut search = name.to_lowercase();
        for text in [
            track.title.as_str(),
            track.artist.as_str(),
            track.album.as_deref().unwrap_or(""),
        ] {
            search.push('\n');
            search.push_str(&text.to_lowercase());
        }
        if !search.contains(&query) {
            return false;
        }
    }
    let duration = track
        .playback_range
        .as_ref()
        .map(|range| range.end_time - range.start_time)
        .or(track.duration_seconds)
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(f64::floor);
    let minimum = options.duration_min.map(f64::floor);
    let maximum = options.duration_max.map(f64::floor);
    !minimum.is_some_and(|bound| duration.is_none_or(|value| value < bound))
        && !maximum.is_some_and(|bound| duration.is_none_or(|value| value > bound))
}
fn directory_availability(files: &[DirectoryFile], options: &DirectoryBrowseOptions) -> [bool; 10] {
    let mut found = [false; 10];
    for file in files {
        if let Some(track) = file
            .track
            .as_ref()
            .filter(|track| directory_file_matches_availability_filters(&file.name, track, options))
        {
            if let Ok(metadata) = serde_json::to_value(track) {
                crate::track_sort_sql::extend_available_fields(&metadata, &mut found);
            }
            if found.iter().all(|present| *present) {
                break;
            }
        }
    }
    found
}

fn sort_directory_files(
    files: Vec<DirectoryFile>,
    options: &DirectoryBrowseOptions,
    offset: usize,
    limit: usize,
) -> Result<(Vec<DirectoryFile>, usize, [bool; 10])> {
    let (field, descending) = options.validate()?;
    let conn = rusqlite::Connection::open_in_memory().map_err(|e| e.to_string())?;
    crate::track_sort_text::register_sqlite(&conn)?;
    conn.execute_batch("CREATE TABLE files (ordinal INTEGER PRIMARY KEY,path TEXT NOT NULL,name TEXT NOT NULL,metadata TEXT,search_text TEXT NOT NULL)").map_err(|e| e.to_string())?;
    for (chunk_index, chunk) in files.chunks(100).enumerate() {
        let mut values = Vec::with_capacity(chunk.len() * 5);
        for (within, file) in chunk.iter().enumerate() {
            let metadata = file
                .track
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| e.to_string())?;
            let mut search = file.name.clone();
            if let Some(track) = &file.track {
                for text in [
                    track.title.as_str(),
                    track.artist.as_str(),
                    track.album.as_deref().unwrap_or(""),
                ] {
                    search.push('\n');
                    search.push_str(text);
                }
            }
            values.extend([
                rusqlite::types::Value::Integer((chunk_index * 100 + within) as i64),
                rusqlite::types::Value::Text(file.path.clone()),
                rusqlite::types::Value::Text(file.name.clone()),
                metadata
                    .map(rusqlite::types::Value::Text)
                    .unwrap_or(rusqlite::types::Value::Null),
                rusqlite::types::Value::Text(search.to_lowercase()),
            ]);
        }
        let sql = format!(
            "INSERT INTO files VALUES {}",
            vec!["(?,?,?,?,?)"; chunk.len()].join(",")
        );
        conn.execute(&sql, rusqlite::params_from_iter(values))
            .map_err(|e| e.to_string())?;
    }
    // Match shared parseDurationBound/wholeDuration: positive playable durations
    // and user bounds are compared in displayed whole-second buckets.
    let duration = crate::track_sort_sql::expression("duration", "metadata")
        .expect("shared duration expression");
    let predicate =
        format!("(?1 IS NULL OR ({duration})>=?1) AND (?2 IS NULL OR ({duration})<=?2) AND instr(search_text,?3)>0");
    let query = options.query.as_deref().unwrap_or("").trim().to_lowercase();
    let duration_min = options.duration_min.map(f64::floor);
    let duration_max = options.duration_max.map(f64::floor);
    let params = rusqlite::params![duration_min, duration_max, query];
    let availability = directory_availability(&files, options);
    let total: usize = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM files WHERE {predicate}"),
            params,
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let expression = if matches!(field, "filename" | "sourceOrder") {
        "name COLLATE BINARY".to_owned()
    } else {
        crate::track_sort_sql::expression(field, "metadata")
            .ok_or("Unsupported directory sort field")?
    };
    let direction = if descending { "DESC" } else { "ASC" };
    let sql = format!("SELECT ordinal FROM files WHERE {predicate} ORDER BY ({expression}) IS NULL ASC,({expression}) {direction},path COLLATE BINARY ASC LIMIT ?4 OFFSET ?5");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let indices = stmt
        .query_map(
            rusqlite::params![duration_min, duration_max, query, limit, offset],
            |row| row.get::<_, usize>(0),
        )
        .map_err(|e| e.to_string())?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut files = files.into_iter().map(Some).collect::<Vec<_>>();
    Ok((
        indices
            .into_iter()
            .map(|index| files[index].take().expect("unique relation ordinal"))
            .collect(),
        total,
        availability,
    ))
}

fn immediate_audio_files(root: &DirectoryRoot, folder: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for (index, entry) in fs::read_dir(folder)
        .map_err(|_| "Directory cannot be read")?
        .enumerate()
    {
        if index >= MAX_ENTRIES {
            return Err(
                "Directory has more than 100000 immediate entries; choose a subdirectory".into(),
            );
        }
        let entry = entry.map_err(|_| "Directory entry cannot be read")?;
        let kind = entry
            .file_type()
            .map_err(|_| "Directory entry cannot be read")?;
        if kind.is_file()
            && !kind.is_symlink()
            && !burrata_working_artifact(&entry.path())
            && supported_audio(&entry.path())
        {
            let relative = relative_text(Path::new(&root.path), &entry.path())?;
            files.push(contained_path(root, &relative)?);
        }
    }
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(files)
}
fn immediate_directories(
    root: &DirectoryRoot,
    path: &Path,
    entry_limit: usize,
) -> Result<Vec<ChildDirectory>> {
    let mut directories = Vec::new();
    for (index, entry) in fs::read_dir(path)
        .map_err(|_| "Directory cannot be read")?
        .enumerate()
    {
        if index >= entry_limit {
            return Err(
                "Directory has more than 100000 immediate entries; choose a subdirectory".into(),
            );
        }
        let entry = entry.map_err(|_| "Directory entry cannot be read")?;
        let kind = entry
            .file_type()
            .map_err(|_| "Directory entry cannot be read")?;
        if !kind.is_dir() || kind.is_symlink() || burrata_working_artifact(&entry.path()) {
            continue;
        }
        // Recheck containment and all path components before returning a traversable child.
        let relative = relative_text(Path::new(&root.path), &entry.path())?;
        contained_path(root, &relative)?;
        directories.push(ChildDirectory {
            name: entry
                .file_name()
                .into_string()
                .map_err(|_| "Directory contains a non-UTF8 filename")?,
            relative_path: relative,
        });
    }
    directories.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(directories)
}
fn supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            [
                "mp3", "flac", "wav", "wave", "aif", "aiff", "m4a", "mp4", "aac", "ogg", "opus",
                "wma", "alac",
            ]
            .contains(&value.to_ascii_lowercase().as_str())
        })
}
fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "Directory path is not UTF8".into())
}
fn reject_symlinks(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            // `fs::canonicalize` returns Windows paths with a verbatim prefix
            // (for example `\\?\C:`). That prefix is not an independently
            // addressable filesystem component, so retain it but do not pass
            // it alone to `symlink_metadata`; the following root/component
            // entries are still checked below.
            Component::Prefix(_) => {
                current.push(component.as_os_str());
                continue;
            }
            Component::ParentDir => return Err("Parent directory traversal is not allowed".into()),
            Component::CurDir => continue,
            _ => current.push(component.as_os_str()),
        };
        let metadata =
            fs::symlink_metadata(&current).map_err(|_| "Directory path is unavailable")?;
        if metadata.file_type().is_symlink() {
            return Err("Symbolic links are not traversed".into());
        }
    }
    Ok(())
}
fn contained_path(root: &DirectoryRoot, relative: &str) -> Result<PathBuf> {
    let relative = Path::new(relative);
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("Use a relative directory path without traversal".into());
    }
    if relative
        .components()
        .any(|part| burrata_working_artifact(Path::new(part.as_os_str())))
    {
        return Err("Burrata working artifacts are not browsable".into());
    }
    let root_path = Path::new(&root.path);
    reject_symlinks(root_path)?;
    let path = root_path.join(relative);
    reject_symlinks(&path)?;
    let canonical = fs::canonicalize(&path).map_err(|_| "Directory path is unavailable")?;
    if !canonical.starts_with(root_path) {
        return Err("Directory path is outside its registered root".into());
    }
    Ok(canonical)
}
fn relative_text(root: &Path, path: &Path) -> Result<String> {
    path.strip_prefix(root)
        .map_err(|_| "Directory path is outside its registered root".to_owned())
        .and_then(path_text)
}
fn breadcrumbs(root: &DirectoryRoot, path: &Path) -> Result<Vec<DirectoryBreadcrumb>> {
    let relative = path
        .strip_prefix(&root.path)
        .map_err(|_| "Directory path is outside its root")?;
    let mut result = vec![DirectoryBreadcrumb {
        name: root.name.clone(),
        relative_path: String::new(),
    }];
    let mut current = PathBuf::new();
    for part in relative.components() {
        current.push(part.as_os_str());
        result.push(DirectoryBreadcrumb {
            name: part.as_os_str().to_string_lossy().into_owned(),
            relative_path: path_text(&current)?,
        });
    }
    Ok(result)
}
fn fingerprint(path: &Path) -> Result<serde_json::Value> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "Audio file is unavailable")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Audio file is not a regular file".into());
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().to_string());
    Ok(serde_json::json!({"size":metadata.len(),"modified":modified}))
}
fn collect_audio(root: &DirectoryRoot, cancel: &watch::Receiver<bool>) -> Result<Vec<PathBuf>> {
    let mut stack = vec![(contained_path(root, "")?, 0usize)];
    let mut files = Vec::new();
    let mut visited = 0;
    while let Some((directory, depth)) = stack.pop() {
        if *cancel.borrow() {
            return Err("Directory index was cancelled".into());
        }
        if depth > MAX_DEPTH {
            return Err("Directory nesting exceeded 64 levels".into());
        }
        for entry in fs::read_dir(&directory).map_err(|_| "Directory cannot be read")? {
            if *cancel.borrow() {
                return Err("Directory index was cancelled".into());
            }
            visited += 1;
            if visited > MAX_ENTRIES {
                return Err("Directory index exceeds 100000 entries; choose a smaller root".into());
            }
            let entry = entry.map_err(|_| "Directory entry cannot be read")?;
            let kind = entry
                .file_type()
                .map_err(|_| "Directory entry cannot be read")?;
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if burrata_working_artifact(&path) {
                continue;
            }
            let relative = relative_text(Path::new(&root.path), &path)?;
            let path = contained_path(root, &relative)?;
            if kind.is_dir() {
                stack.push((path, depth + 1));
            } else if kind.is_file() && supported_audio(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, DirectoryBrowser, DirectoryRoot) {
        let temp = tempfile::tempdir().unwrap();
        // macOS exposes the temporary directory through `/var`, which is a
        // symlink to `/private/var`. Roots intentionally reject symlink path
        // components, so tests must register the canonical physical path.
        let temp_path = fs::canonicalize(temp.path()).unwrap();
        let root = temp_path.join("music");
        fs::create_dir(&root).unwrap();
        let manager = DirectoryBrowser::new(Arc::new(
            Store::open(&temp_path.join("library.sqlite")).unwrap(),
        ));
        let registered = manager.add_root(root.to_str().unwrap()).unwrap();
        (temp, manager, registered)
    }
    fn inspected(hash: &str, title: &str) -> Track {
        Track {
            id: format!("local:{hash}"),
            provider: "local".into(),
            provider_id: "raw-inspector-path".into(),
            title: title.into(),
            artist: "Artist".into(),
            duration_seconds: Some(30.0),
            provenance: serde_json::json!({"codec":"wav"}),
            ..Track::default()
        }
    }
    #[test]
    fn explicit_roots_are_persisted_independently_and_remove_does_not_delete_audio() {
        let (_temp, manager, root) = fixture();
        let before = serde_json::to_value(manager.inner.store.settings().unwrap()).unwrap();
        assert_eq!(manager.add_root(&root.path).unwrap().id, root.id);
        assert_eq!(
            DirectoryBrowser::new(manager.inner.store.clone())
                .roots()
                .unwrap(),
            vec![root.clone()]
        );
        let file = Path::new(&root.path).join("keep.wav");
        fs::write(&file, b"fixture").unwrap();
        manager.remove_root(&root.id).unwrap();
        assert!(file.exists());
        assert!(manager.roots().unwrap().is_empty());
        assert_eq!(
            serde_json::to_value(manager.inner.store.settings().unwrap()).unwrap(),
            before
        );
        assert!(manager.browse(&root.id, "", 0, 30).is_err());
    }
    #[test]
    fn browse_immediate_children_and_supported_audio_pages_without_indexing() {
        let (_temp, manager, root) = fixture();
        let base = Path::new(&root.path);
        fs::create_dir(base.join("child")).unwrap();
        fs::write(base.join("child/nested.wav"), b"nested").unwrap();
        for name in ["b.MP3", "a.flac", "ignore.txt"] {
            fs::write(base.join(name), b"fixture").unwrap();
        }
        let first = manager.browse(&root.id, "", 0, 1).unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(first.files[0].name, "a.flac");
        assert!(first.files[0].track.is_none());
        assert_eq!(first.directories[0].relative_path, "child");
        assert_eq!(
            manager.browse(&root.id, "", 1, 1).unwrap().files[0].name,
            "b.MP3"
        );
        let nested = manager.browse(&root.id, "child", 0, 30).unwrap();
        assert_eq!(nested.breadcrumbs.len(), 2);
        assert_eq!(nested.files[0].name, "nested.wav");
        for relative in ["..", "child/../child", "/tmp"] {
            assert!(manager.browse(&root.id, relative, 0, 30).is_err());
        }
        assert!(manager.browse(&root.id, "", 0, 201).is_err());
        assert!(manager.inner.store.list_asset_paths().unwrap().is_empty());
    }
    fn sortable_file(path: &str, title: Option<&str>, duration: Option<f64>) -> DirectoryFile {
        DirectoryFile {
            path: path.into(),
            name: path.into(),
            asset_id: None,
            track: title.map(|title| Track {
                title: title.into(),
                duration_seconds: duration,
                ..Track::default()
            }),
        }
    }
    #[test]
    fn global_sort_filters_displayed_duration_buckets_before_pagination_and_keeps_unknown_last() {
        let files = || {
            vec![
                sortable_file("z", Some("Alpha"), Some(10.9)),
                sortable_file("a", Some("Zulu"), Some(10.1)),
                sortable_file("missing", None, None),
                sortable_file("b", Some("Alpha"), Some(10.5)),
            ]
        };
        let opts = DirectoryBrowseOptions {
            sort_field: Some("title".into()),
            ..Default::default()
        };
        let (page, total, _) = sort_directory_files(files(), &opts, 1, 1).unwrap();
        assert_eq!(total, 4);
        assert_eq!(page[0].path, "z");
        let desc = DirectoryBrowseOptions {
            sort_direction: Some("desc".into()),
            ..opts.clone()
        };
        let (page, _, _) = sort_directory_files(files(), &desc, 0, 10).unwrap();
        assert_eq!(
            page.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "z", "missing"]
        );
        let bounded = DirectoryBrowseOptions {
            duration_min: Some(10.5),
            duration_max: Some(10.9),
            ..opts
        };
        let (page, total, _) = sort_directory_files(files(), &bounded, 1, 1).unwrap();
        assert_eq!(total, 3);
        assert_eq!(page[0].path, "z");
        let equal_second = DirectoryBrowseOptions {
            duration_min: Some(10.0),
            duration_max: Some(10.0),
            ..Default::default()
        };
        assert_eq!(
            sort_directory_files(files(), &equal_second, 0, 10)
                .unwrap()
                .1,
            3
        );
        assert_eq!(
            sort_directory_files(
                vec![
                    sortable_file("live", Some("Live"), Some(0.0)),
                    sortable_file("unknown", None, None)
                ],
                &equal_second,
                0,
                10
            )
            .unwrap()
            .1,
            0
        );
        let invalid = DirectoryBrowseOptions {
            duration_min: Some(f64::NAN),
            ..Default::default()
        };
        assert!(invalid.validate().is_err());
        assert!(DirectoryBrowseOptions {
            duration_min: Some(3.0),
            duration_max: Some(2.0),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(DirectoryBrowseOptions {
            sort_field: Some("title;DROP TABLE files".into()),
            ..Default::default()
        }
        .validate()
        .is_err());
        for field in [
            "filename",
            "sourceOrder",
            "artist",
            "album",
            "duration",
            "releaseDate",
            "dateAdded",
            "bpm",
            "key",
            "plays",
            "views",
        ] {
            assert!(
                sort_directory_files(
                    files(),
                    &DirectoryBrowseOptions {
                        sort_field: Some(field.into()),
                        ..Default::default()
                    },
                    0,
                    10
                )
                .is_ok(),
                "{field}"
            );
        }
    }
    #[test]
    fn directory_duration_filter_uses_chapter_range_not_parent_duration() {
        let mut file = sortable_file("chapter.wav", Some("Chapter"), Some(500.0));
        file.track.as_mut().unwrap().playback_range = Some(crate::models::PlaybackRange {
            parent_track_id: "parent".into(),
            chapter_id: "clip".into(),
            source_url: String::new(),
            start_time: 10.25,
            end_time: 13.75,
            parent_title: "Parent".into(),
            parent_duration_seconds: Some(500.0),
        });
        let opts = DirectoryBrowseOptions {
            duration_min: Some(3.0),
            duration_max: Some(3.0),
            ..Default::default()
        };
        assert_eq!(sort_directory_files(vec![file], &opts, 0, 10).unwrap().1, 1);
    }
    #[test]
    fn directory_query_is_global_literal_unicode_and_matches_metadata_or_filename() {
        let mut file = sortable_file("plain.wav", Some("ÉTÉ 100%_"), Some(20.0));
        file.track.as_mut().unwrap().artist = "Artist Needle".into();
        file.track.as_mut().unwrap().album = Some("Album Search".into());
        for query in ["été", "100%_", "needle", "album search", "plain.wav"] {
            let (page, total, _) = sort_directory_files(
                vec![sortable_file("a.wav", None, None), file.clone()],
                &DirectoryBrowseOptions {
                    query: Some(query.into()),
                    ..Default::default()
                },
                0,
                1,
            )
            .unwrap();
            assert_eq!(total, 1);
            assert_eq!(page[0].path, "plain.wav");
        }
        assert_eq!(
            sort_directory_files(
                vec![file],
                &DirectoryBrowseOptions {
                    query: Some("100__".into()),
                    ..Default::default()
                },
                0,
                10
            )
            .unwrap()
            .1,
            0
        );
        assert!(DirectoryBrowseOptions {
            query: Some("x".repeat(513)),
            ..Default::default()
        }
        .validate()
        .is_err());
        let joined = sortable_file("end", Some("start"), Some(5.0));
        assert!(directory_file_matches_availability_filters(
            "end",
            joined.track.as_ref().unwrap(),
            &DirectoryBrowseOptions {
                query: Some("end\nstart".into()),
                ..Default::default()
            }
        ));
    }
    #[test]
    fn directory_availability_uses_active_query_and_chapter_duration_scope() {
        let mut chapter = sortable_file("chapter.wav", Some("Other"), Some(500.0));
        chapter.track.as_mut().unwrap().playback_range = Some(crate::models::PlaybackRange {
            parent_track_id: "parent".into(),
            chapter_id: "clip".into(),
            source_url: String::new(),
            start_time: 10.25,
            end_time: 13.75,
            parent_title: "Parent".into(),
            parent_duration_seconds: Some(500.0),
        });
        let mut match_bpm = sortable_file("needle.wav", Some("Needle"), Some(20.0));
        match_bpm.track.as_mut().unwrap().bpm = Some(120.0);
        let opts = DirectoryBrowseOptions {
            query: Some("chapter".into()),
            duration_min: Some(3.0),
            duration_max: Some(3.0),
            ..Default::default()
        };
        let fields = directory_availability(&[chapter.clone(), match_bpm], &opts);
        assert!(fields[0]);
        assert!(fields[3]);
        assert!(!fields[6]);
        let opts = DirectoryBrowseOptions {
            query: Some("needle".into()),
            duration_min: Some(3.0),
            duration_max: Some(3.0),
            ..Default::default()
        };
        let fields = directory_availability(
            &[
                chapter,
                sortable_file("needle.wav", Some("Needle"), Some(3.9)),
            ],
            &opts,
        );
        assert!(fields[3]);
        assert!(!fields[6]);
    }
    #[test]
    fn global_sort_relation_keeps_fifty_thousand_file_occurrences_and_stable_pages() {
        let files = (0..50_000)
            .rev()
            .map(|i| sortable_file(&format!("file-{i:05}"), Some("Same title"), Some(15.5)))
            .collect();
        let (page, total, _) = sort_directory_files(
            files,
            &DirectoryBrowseOptions {
                sort_field: Some("title".into()),
                duration_min: Some(15.5),
                duration_max: Some(15.5),
                ..Default::default()
            },
            49_800,
            200,
        )
        .unwrap();
        assert_eq!(total, 50_000);
        assert_eq!(page.len(), 200);
        assert_eq!(page.first().unwrap().path, "file-49800");
        assert_eq!(page.last().unwrap().path, "file-49999");
    }
    #[test]
    fn directory_sort_hydrates_exact_asset_analysis_and_preserves_manual_edits() {
        let (_temp, manager, root) = fixture();
        let base = Path::new(&root.path);
        let (_, cancel) = watch::channel(false);
        let hash = "e".repeat(64);
        for name in ["a.wav", "z.wav"] {
            let path = base.join(name);
            fs::write(&path, b"fixture").unwrap();
            manager
                .commit_inspection(
                    &root,
                    name,
                    &fingerprint(&path).unwrap(),
                    inspected(&hash, "Same recording"),
                    &cancel,
                )
                .unwrap();
        }
        let mut assets = manager
            .inner
            .store
            .list_directory_assets_for_paths(&[
                base.join("a.wav").to_str().unwrap().into(),
                base.join("z.wav").to_str().unwrap().into(),
            ])
            .unwrap();
        assets.sort_by(|a, b| a.path.cmp(&b.path));
        let first = assets[0].clone();
        let mut second = assets[1].clone();
        second.track.duration_seconds = Some(31.75);
        second.duration_seconds = Some(31.75);
        second.track.provenance["autotag"] = serde_json::json!({"version":"test"});
        second.track.album = Some("Selected album".into());
        manager.inner.store.save_asset(&second).unwrap();
        manager
            .inner
            .store
            .set_preferred_track_file(&first.track.id, &first.id)
            .unwrap();
        manager
            .inner
            .store
            .save_analysis_record(
                &first.id,
                &first.hash,
                "test",
                "completed",
                &serde_json::json!({"bpm":90.0}),
            )
            .unwrap();
        manager
            .inner
            .store
            .save_analysis_record(
                &second.id,
                &second.hash,
                "test",
                "completed",
                &serde_json::json!({"bpm":130.0}),
            )
            .unwrap();
        // A historical file version can leave a newer analysis row whose hash
        // no longer matches the asset. Seed through the validated storage API.
        let mut previous_version = second.clone();
        previous_version.hash = "f".repeat(64);
        manager.inner.store.save_asset(&previous_version).unwrap();
        manager
            .inner
            .store
            .save_analysis_record(
                &second.id,
                &previous_version.hash,
                "newer-stale",
                "completed",
                &serde_json::json!({"bpm":999.0}),
            )
            .unwrap();
        manager.inner.store.save_asset(&second).unwrap();
        let opts = DirectoryBrowseOptions {
            sort_field: Some("bpm".into()),
            sort_direction: Some("desc".into()),
            ..Default::default()
        };
        let page = manager
            .browse_with_options(&root.id, "", 0, 1, &opts)
            .unwrap();
        assert_eq!(page.total, 2);
        let track = page.files[0].track.as_ref().unwrap();
        assert_eq!(page.files[0].asset_id.as_deref(), Some(second.id.as_str()));
        assert_eq!(track.bpm, Some(130.0));
        assert_eq!(track.duration_seconds, Some(31.75));
        assert_eq!(track.album.as_deref(), Some("Selected album"));
        let mut manual = manager.inner.store.get_track(&track.id).unwrap().unwrap();
        manual.title = "Manual across files".into();
        manager.inner.store.update_track(&manual).unwrap();
        let page = manager
            .browse_with_options(
                &root.id,
                "",
                0,
                10,
                &DirectoryBrowseOptions {
                    duration_min: Some(31.5),
                    duration_max: Some(31.9),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(
            page.files[0].track.as_ref().unwrap().title,
            "Manual across files"
        );
        fs::write(base.join("z.wav"), b"changed-content").unwrap();
        assert_eq!(
            manager
                .browse_with_options(
                    &root.id,
                    "",
                    0,
                    10,
                    &DirectoryBrowseOptions {
                        duration_min: Some(31.5),
                        ..Default::default()
                    }
                )
                .unwrap()
                .total,
            0
        );
    }
    #[test]
    fn tree_children_are_directory_only_sorted_and_bounded_without_indexing() {
        let (_temp, manager, root) = fixture();
        let base = Path::new(&root.path);
        for name in [
            "z",
            "a",
            ".personal",
            ".burrata-staging",
            ".burrata-retag-work",
        ] {
            fs::create_dir(base.join(name)).unwrap();
        }
        fs::create_dir(base.join("a/nested")).unwrap();
        fs::write(base.join("invalid.mp3"), b"not audio").unwrap();
        let page = manager.children(&root.id, "").unwrap();
        assert_eq!(page.root_id, root.id);
        assert_eq!(page.relative_path, "");
        assert_eq!(
            page.directories
                .iter()
                .map(|dir| dir.name.as_str())
                .collect::<Vec<_>>(),
            vec![".personal", "a", "z"]
        );
        let nested = manager.children(&root.id, "a").unwrap();
        assert_eq!(nested.relative_path, "a");
        // Relative paths use the host separator and remain opaque to the UI.
        assert_eq!(
            PathBuf::from(&nested.directories[0].relative_path),
            Path::new("a").join("nested")
        );
        assert!(manager
            .children(&root.id, &nested.directories[0].relative_path)
            .unwrap()
            .directories
            .is_empty());
        for relative in ["..", "a/../z", "/tmp", ".burrata-staging", "invalid.mp3"] {
            assert!(manager.children(&root.id, relative).is_err());
        }
        assert!(manager.children("unregistered", "").is_err());
        assert!(manager.children(&root.id, &"x".repeat(4097)).is_err());
        assert!(immediate_directories(&root, base, 2).is_err());
        assert!(manager.inner.store.list_asset_paths().unwrap().is_empty());
        let encoded = serde_json::to_value(page).unwrap();
        assert_eq!(encoded.as_object().unwrap().len(), 3);
        assert!(encoded.get("files").is_none());
    }
    #[cfg(unix)]
    #[test]
    fn tree_children_never_follow_symlink_directories() {
        use std::os::unix::fs::symlink;
        let (temp, manager, root) = fixture();
        let outside = fs::canonicalize(temp.path()).unwrap().join("outside-tree");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, Path::new(&root.path).join("linked")).unwrap();
        assert!(manager
            .children(&root.id, "")
            .unwrap()
            .directories
            .is_empty());
        assert!(manager.children(&root.id, "linked").is_err());
    }
    #[test]
    fn working_artifacts_are_excluded_without_hiding_ordinary_hidden_audio() {
        let (_temp, manager, root) = fixture();
        let base = Path::new(&root.path);
        fs::create_dir(base.join(".burrata-staging")).unwrap();
        fs::create_dir(base.join(".personal")).unwrap();
        for name in [
            ".burrata-staging/staged.mp3",
            ".burrata-retag-test.mp3",
            ".burrata-autotag-test.flac",
            ".personal/keep.wav",
            ".hidden.mp3",
            "ordinary.wav",
        ] {
            fs::write(base.join(name), b"fixture").unwrap();
        }
        let page = manager.browse(&root.id, "", 0, 30).unwrap();
        assert_eq!(
            page.files
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            vec![".hidden.mp3", "ordinary.wav"]
        );
        assert_eq!(page.directories.len(), 1);
        assert_eq!(page.directories[0].name, ".personal");
        assert!(manager.browse(&root.id, ".burrata-staging", 0, 30).is_err());
        let (_, cancel) = watch::channel(false);
        let files = collect_audio(&root, &cancel).unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|path| !path
            .components()
            .any(|part| burrata_working_artifact(Path::new(part.as_os_str())))));
        assert!(base.join(".burrata-staging/staged.mp3").exists());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_roots_directories_and_files_are_never_followed() {
        use std::os::unix::fs::symlink;
        let (temp, manager, root) = fixture();
        let outside = fs::canonicalize(temp.path()).unwrap().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.wav"), b"outside").unwrap();
        let base = Path::new(&root.path);
        symlink(&outside, base.join("linked")).unwrap();
        symlink(outside.join("secret.wav"), base.join("linked.wav")).unwrap();
        assert!(manager
            .add_root(base.join("linked").to_str().unwrap())
            .is_err());
        assert!(manager.browse(&root.id, "linked", 0, 30).is_err());
        let page = manager.browse(&root.id, "", 0, 30).unwrap();
        assert!(page.files.is_empty());
        assert!(page.directories.is_empty());
        let (_, cancel) = watch::channel(false);
        assert!(collect_audio(&root, &cancel).unwrap().is_empty());
    }
    #[test]
    fn indexed_same_content_paths_keep_distinct_assets_and_pinned_playback_tracks() {
        let (_temp, manager, root) = fixture();
        let hash = "a".repeat(64);
        let base = Path::new(&root.path);
        let (_, cancel) = watch::channel(false);
        for name in ["first.wav", "second.wav"] {
            let path = base.join(name);
            fs::write(&path, b"same-content").unwrap();
            manager
                .commit_inspection(
                    &root,
                    name,
                    &fingerprint(&path).unwrap(),
                    inspected(&hash, name),
                    &cancel,
                )
                .unwrap();
        }
        let page = manager.browse(&root.id, "", 0, 30).unwrap();
        assert_eq!(page.files.len(), 2);
        let first = page.files[0].track.as_ref().unwrap();
        let second = page.files[1].track.as_ref().unwrap();
        assert_eq!(first.id, second.id);
        assert_ne!(first.asset_id, second.asset_id);
        assert_ne!(first.local_path, second.local_path);
        assert_eq!(
            first.local_path.as_deref(),
            Some(page.files[0].path.as_str())
        );
        assert_eq!(
            second.local_path.as_deref(),
            Some(page.files[1].path.as_str())
        );
        assert_eq!(manager.inner.store.list_asset_paths().unwrap().len(), 2);
        let mut manual = first.clone();
        manual.title = "Manual title".into();
        manager.inner.store.update_track(&manual).unwrap();
        let path = base.join("second.wav");
        manager
            .commit_inspection(
                &root,
                "second.wav",
                &fingerprint(&path).unwrap(),
                inspected(&hash, "Retagged source"),
                &cancel,
            )
            .unwrap();
        assert!(manager
            .browse(&root.id, "", 0, 30)
            .unwrap()
            .files
            .iter()
            .all(|file| file.track.as_ref().unwrap().title == "Manual title"));
    }
    #[test]
    fn cancelled_or_changed_files_cannot_commit_stale_index_data() {
        let (_temp, manager, root) = fixture();
        let path = Path::new(&root.path).join("one.wav");
        fs::write(&path, b"original").unwrap();
        let before = fingerprint(&path).unwrap();
        let (sender, cancel) = watch::channel(false);
        sender.send_replace(true);
        assert!(collect_audio(&root, &cancel).is_err());
        assert!(manager
            .commit_inspection(
                &root,
                "one.wav",
                &before,
                inspected(&"a".repeat(64), "One"),
                &cancel
            )
            .is_err());
        assert!(manager.inner.store.list_asset_paths().unwrap().is_empty());
        sender.send_replace(false);
        manager
            .commit_inspection(
                &root,
                "one.wav",
                &before,
                inspected(&"a".repeat(64), "One"),
                &cancel,
            )
            .unwrap();
        fs::write(&path, b"changed-longer").unwrap();
        assert!(manager
            .commit_inspection(
                &root,
                "one.wav",
                &before,
                inspected(&"a".repeat(64), "Stale"),
                &cancel
            )
            .is_err());
        assert!(manager
            .commit_inspection(
                &root,
                "one.wav",
                &fingerprint(&path).unwrap(),
                inspected(&"b".repeat(64), "Changed"),
                &cancel
            )
            .is_err());
        assert!(manager.browse(&root.id, "", 0, 30).unwrap().files[0]
            .track
            .is_none());
        assert_eq!(manager.inner.store.list_asset_paths().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn sort_availability_job_completes_for_unindexed_files_without_hydrating_page() {
        let (_temp, manager, root) = fixture();
        fs::write(
            Path::new(&root.path).join("unindexed.wav"),
            b"audio placeholder",
        )
        .unwrap();
        let page = manager.browse(&root.id, "", 0, 1).unwrap();
        assert_eq!(page.files.len(), 1);
        assert!(page.sort_availability.is_none());
        let started = manager
            .start_sort_availability(&root.id, "", DirectoryBrowseOptions::default())
            .unwrap();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let status = manager.get_sort_availability(&started.job_id).unwrap();
                if status.state != ScanState::Running {
                    break status;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(completed.state, ScanState::Completed);
        assert_eq!(completed.total, Some(1));
        assert_eq!(completed.fields, Some([false; 10]));
        assert!(manager.inner.store.list_asset_paths().unwrap().is_empty());
    }
    #[tokio::test]
    async fn summary_finds_path_specific_metadata_after_first_page_for_same_recording() {
        let (_temp, manager, root) = fixture();
        let base = Path::new(&root.path);
        let hash = "a".repeat(64);
        let (_, cancel) = watch::channel(false);
        for index in 0..201 {
            let name = format!("file-{index:03}.wav");
            let path = base.join(&name);
            fs::write(&path, b"same recording bytes").unwrap();
            let mut track = inspected(&hash, "Same recording");
            if index == 200 {
                track.bpm = Some(123.0);
            }
            manager
                .commit_inspection(&root, &name, &fingerprint(&path).unwrap(), track, &cancel)
                .unwrap();
        }
        let first_page = manager.browse(&root.id, "", 0, 200).unwrap();
        assert_eq!(first_page.total, 201);
        assert_eq!(first_page.files.len(), 200);
        assert!(first_page.sort_availability.is_none());
        assert!(first_page
            .files
            .iter()
            .all(|file| file.track.as_ref().unwrap().bpm.is_none()));
        let first_asset = manager
            .inner
            .store
            .get_asset_by_path(base.join("file-000.wav").to_str().unwrap())
            .unwrap()
            .unwrap();
        let last_asset = manager
            .inner
            .store
            .get_asset_by_path(base.join("file-200.wav").to_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(first_asset.recording_id, last_asset.recording_id);
        assert_eq!(first_asset.track.bpm, None);
        assert_eq!(last_asset.track.bpm, Some(123.0));

        let started = manager
            .start_sort_availability(&root.id, "", DirectoryBrowseOptions::default())
            .unwrap();
        let completed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let status = manager.get_sort_availability(&started.job_id).unwrap();
                if status.state != ScanState::Running {
                    break status;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(completed.state, ScanState::Completed);
        assert_eq!(completed.completed, 201);
        assert_eq!(completed.fields.unwrap()[6], true);
    }
    #[tokio::test]
    async fn scan_jobs_reject_overlap_and_cancellation_finishes_without_indexing() {
        let (_temp, manager, root) = fixture();
        let first = manager.start_scan(&root.id).unwrap();
        assert!(manager.start_scan(&root.id).is_err());
        manager.cancel_scan(&first.job_id).unwrap();
        let done = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let status = manager.scan_status(&first.job_id).unwrap();
                if status.state != ScanState::Running {
                    break status;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(done.state, ScanState::Cancelled);
        assert_eq!(done.indexed, 0);
        assert!(manager.inner.store.list_asset_paths().unwrap().is_empty());
    }
    fn legacy_download(
        manager: &DirectoryBrowser,
        root: &DirectoryRoot,
        name: &str,
    ) -> (PathBuf, LocalAsset) {
        let path = Path::new(&root.path).join(name);
        fs::write(&path, b"not real audio: existing trusted metadata").unwrap();
        let hash = media::hash_file(&path).unwrap();
        let (_, cancel) = watch::channel(false);
        manager
            .commit_inspection(
                root,
                name,
                &fingerprint(&path).unwrap(),
                inspected(&hash, "Legacy download"),
                &cancel,
            )
            .unwrap();
        let mut asset = manager
            .inner
            .store
            .get_asset_by_path(path.to_str().unwrap())
            .unwrap()
            .unwrap();
        asset
            .track
            .provenance
            .as_object_mut()
            .unwrap()
            .remove("directoryBrowserFingerprint");
        asset.processing = "download-managed".into();
        asset.duration_seconds = Some(15.0);
        asset.track.duration_seconds = Some(15.0);
        asset.track.playback_range = Some(crate::models::PlaybackRange {
            parent_track_id: "parent".into(),
            chapter_id: "clip".into(),
            source_url: "local:parent".into(),
            start_time: 10.0,
            end_time: 25.0,
            parent_title: "Parent".into(),
            parent_duration_seconds: Some(120.0),
        });
        manager.inner.store.save_asset(&asset).unwrap();
        (path, asset)
    }
    #[test]
    fn legacy_asset_verifies_hash_once_then_reuses_metadata_and_exact_asset_without_probe() {
        let (_temp, manager, root) = fixture();
        let (path, legacy) = legacy_download(&manager, &root, "existing.wav");
        assert!(manager.browse(&root.id, "", 0, 30).unwrap().files[0]
            .track
            .is_none());
        let mut manual = legacy.track.clone();
        manual.title = "Manual retained".into();
        manager.inner.store.update_track(&manual).unwrap();
        let before = fingerprint(&path).unwrap();
        let (_, cancel) = watch::channel(false);
        let hash_calls = std::sync::atomic::AtomicUsize::new(0);
        let known = manager
            .inner
            .store
            .get_asset_by_path(path.to_str().unwrap())
            .unwrap()
            .unwrap();
        manager
            .reuse_existing_asset(&root, "existing.wav", &before, known, &cancel, |file| {
                hash_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                media::hash_file(file)
            })
            .unwrap();
        assert_eq!(hash_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        let page = manager.browse(&root.id, "", 0, 30).unwrap();
        let row = page.files[0].track.as_ref().unwrap();
        assert_eq!(page.files[0].asset_id.as_deref(), Some(legacy.id.as_str()));
        assert_eq!(row.title, "Manual retained");
        assert_eq!(row.duration_seconds, Some(15.0));
        assert_eq!(row.playback_range, legacy.track.playback_range);
        let known = manager
            .inner
            .store
            .get_asset_by_path(path.to_str().unwrap())
            .unwrap()
            .unwrap();
        manager
            .reuse_existing_asset(&root, "existing.wav", &before, known, &cancel, |_| {
                panic!("current fingerprint must not hash or probe")
            })
            .unwrap();
        assert_eq!(manager.inner.store.list_asset_paths().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn actual_scan_reuses_existing_invalid_audio_bytes_without_invoking_ffprobe() {
        let (_temp, manager, root) = fixture();
        let (_path, legacy) = legacy_download(&manager, &root, "existing.wav");
        for _ in 0..2 {
            let started = manager.start_scan(&root.id).unwrap();
            let done = tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    let status = manager.scan_status(&started.job_id).unwrap();
                    if status.state != ScanState::Running {
                        break status;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(done.state, ScanState::Completed);
            assert_eq!(done.indexed, 1);
            assert_eq!(done.reused, 1);
            assert_eq!(done.inspected, 0);
            assert_eq!(done.failed, 0);
        }
        assert_eq!(
            manager.browse(&root.id, "", 0, 30).unwrap().files[0]
                .asset_id
                .as_deref(),
            Some(legacy.id.as_str())
        );
    }
    #[test]
    fn changed_legacy_asset_is_unavailable_and_never_overwrites_saved_metadata() {
        let (_temp, manager, root) = fixture();
        let (path, legacy) = legacy_download(&manager, &root, "existing.wav");
        fs::write(&path, b"different bytes and length").unwrap();
        let (_, cancel) = watch::channel(false);
        assert!(manager
            .reuse_existing_asset(
                &root,
                "existing.wav",
                &fingerprint(&path).unwrap(),
                legacy.clone(),
                &cancel,
                media::hash_file
            )
            .is_err());
        let retained = manager
            .inner
            .store
            .get_asset_by_path(path.to_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(retained.hash, legacy.hash);
        assert_eq!(retained.track.title, legacy.track.title);
        assert_eq!(retained.duration_seconds, Some(15.0));
        assert_eq!(retained.track.playback_range, legacy.track.playback_range);
        assert!(manager.browse(&root.id, "", 0, 30).unwrap().files[0]
            .track
            .is_none());
    }
    #[test]
    fn cancellation_after_legacy_hash_does_not_commit_a_freshness_marker() {
        let (_temp, manager, root) = fixture();
        let (path, legacy) = legacy_download(&manager, &root, "existing.wav");
        let (sender, cancel) = watch::channel(false);
        assert!(manager
            .reuse_existing_asset(
                &root,
                "existing.wav",
                &fingerprint(&path).unwrap(),
                legacy.clone(),
                &cancel,
                |_| {
                    sender.send_replace(true);
                    Ok(legacy.hash.clone())
                }
            )
            .is_err());
        assert!(manager
            .inner
            .store
            .get_asset_by_path(path.to_str().unwrap())
            .unwrap()
            .unwrap()
            .track
            .provenance
            .get("directoryBrowserFingerprint")
            .is_none());
    }
}
