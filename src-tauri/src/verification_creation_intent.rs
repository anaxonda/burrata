//! Private, one-dispatch intent records; never opens historical account markers.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
/// Only reviewed creation requests are cancelled by confirmed-account changes.
#[derive(Clone, Default)]
pub(super) struct AccountFences(
    std::sync::Arc<
        std::sync::Mutex<
            std::collections::BTreeMap<
                String,
                (String, std::sync::Weak<std::sync::atomic::AtomicBool>),
            >,
        >,
    >,
);
pub(super) struct AccountFence {
    registry: AccountFences,
    key: String,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl AccountFence {
    pub(super) fn cancel_flag(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.cancel.clone()
    }
}
impl Drop for AccountFence {
    fn drop(&mut self) {
        if let Ok(mut active) = self.registry.0.lock() {
            active.remove(&self.key);
        }
    }
}
impl AccountFences {
    pub(super) fn register(
        &self,
        provider: &str,
        actual: &str,
        actual_revision: u64,
        expected: &str,
        expected_revision: u64,
    ) -> Result<AccountFence, String> {
        if actual != expected || actual_revision != expected_revision {
            return Err("Provider identity changed since intent approval".into());
        }
        let mut active = self
            .0
            .lock()
            .map_err(|_| "Intent account state unavailable")?;
        active.retain(|_, (_, flag)| flag.strong_count() > 0);
        if active.values().any(|(owner, _)| owner == provider) {
            return Err("Another verification creation is active for this provider account".into());
        }
        if active.len() >= 32 {
            return Err("Too many active verification intents".into());
        }
        let key = uuid::Uuid::new_v4().to_string();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        active.insert(
            key.clone(),
            (provider.into(), std::sync::Arc::downgrade(&cancel)),
        );
        Ok(AccountFence {
            registry: self.clone(),
            key,
            cancel,
        })
    }
    pub(super) fn invalidate(&self, provider: &str) -> Result<(), String> {
        let mut active = self
            .0
            .lock()
            .map_err(|_| "Intent account state unavailable")?;
        active.retain(|_, (owner, flag)| {
            if let Some(flag) = flag.upgrade() {
                if owner == provider {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                true
            } else {
                false
            }
        });
        Ok(())
    }
}
static PROCESS: LazyLock<(String, Instant)> =
    LazyLock::new(|| (uuid::Uuid::new_v4().to_string(), Instant::now()));
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Contract {
    pub version: u8,
    pub intent_id: String,
    pub provider: String,
    pub account_id: String,
    pub account_revision: u64,
    pub generation: u64,
    pub title: String,
    pub visibility: String,
    pub track_count: u8,
    pub description: Option<String>,
    pub expires_at: u64,
    pub legacy_attempt: Value,
    legacy_fingerprint: String,
    pub prior_creations: Value,
    history_fingerprint: String,
    process: String,
    deadline_ms: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    contract: Contract,
    contract_hash: String,
    review_token_hash: String,
}
fn digest(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
fn valid_uuid(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}
fn epoch() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| "System clock is unavailable".into())
}
fn private_new(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|_| "Intent already exists or cannot be persisted".into())
}
fn sync_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(|_| "Cannot persist intent directory")?;
    }
    Ok(())
}
fn directory(base: &Path) -> Result<PathBuf, String> {
    let dir = base.join("verification-intents");
    match fs::create_dir(&dir) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
                    .map_err(|_| "Cannot secure intent directory")?;
            }
            sync_dir(base)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("Cannot create intent directory".into()),
    };
    if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
        return Err("Intent directory is invalid".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&dir)
            .map_err(|_| "Cannot inspect intent directory")?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err("Intent directory permissions are not private".into());
        }
    }
    Ok(dir)
}
fn legacy_evidence(base: &Path, provider: &str, account: &str) -> (Value, String) {
    let key = digest(format!("{provider}:{account}").as_bytes());
    let path = base.join(format!("verification-playlist-{key}.json"));
    let metadata = fs::symlink_metadata(&path);
    let exists = metadata
        .as_ref()
        .map(|_| true)
        .unwrap_or_else(|e| e.kind() != std::io::ErrorKind::NotFound);
    let bytes = metadata
        .as_ref()
        .ok()
        .filter(|m| m.is_file() && !m.file_type().is_symlink() && m.len() <= 8192)
        .and_then(|_| {
            let mut bytes = Vec::new();
            File::open(&path)
                .ok()?
                .take(8193)
                .read_to_end(&mut bytes)
                .ok()?;
            (bytes.len() <= 8192).then_some(bytes)
        });
    let verified = bytes
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .is_some_and(|v| {
            v.get("verified").and_then(Value::as_bool) == Some(true)
                && v.get("playlistId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty())
        });
    let evidence = json!({"attemptCount":u8::from(exists),"unresolvedCount":u8::from(exists&&!verified),"outcome":if !exists{"NONE"}else if verified{"VERIFIED"}else{"UNKNOWN"},"additionalNewIntent":true,"warning":if exists&&!verified{"An earlier attempt may already have created a playlist. Approving this preview creates an additional NEW private empty playlist; it does not retry or resolve the earlier UNKNOWN attempt."}else{"Approving this preview creates an additional NEW private empty playlist. It does not retry any earlier intent."}});
    let fingerprint=digest(serde_json::to_vec(&json!({"exists":exists,"size":metadata.as_ref().ok().map(|m|m.len()),"modified":metadata.as_ref().ok().and_then(|m|m.modified().ok()).and_then(|t|t.duration_since(UNIX_EPOCH).ok()).map(|d|d.as_nanos().to_string()),"bytes":bytes})).expect("finite legacy evidence"));
    (evidence, fingerprint)
}

/// At most 400 bounded records; never return another account's identity or raw data.
fn prior_intent_evidence(
    base: &Path,
    provider: &str,
    account: &str,
    exclude: &str,
    legacy: &Value,
) -> Result<(Value, String), String> {
    let dir = directory(base)?;
    let mut paths = fs::read_dir(&dir)
        .map_err(|_| "Cannot inspect intent history")?
        .take(401)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Cannot inspect intent history")?;
    let truncated = paths.len() > 400;
    paths.truncate(400);
    paths.sort_by_key(|entry| entry.file_name());
    let mut groups = std::collections::BTreeMap::<
        String,
        std::collections::BTreeMap<String, Option<Vec<u8>>>,
    >::new();
    let mut unattributed = 0u64;
    let mut fingerprints = Vec::<String>::new();
    for entry in paths {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((id, kind)) = name.rsplit_once('.') else {
            unattributed += 1;
            fingerprints.push(digest(name.as_bytes()));
            continue;
        };
        if id == exclude {
            continue;
        }
        if !valid_uuid(id) || !matches!(kind, "json" | "dispatch" | "state" | "created") {
            unattributed += 1;
            fingerprints.push(digest(name.as_bytes()));
            continue;
        }
        let bytes = fs::symlink_metadata(entry.path())
            .ok()
            .filter(|m| m.is_file() && !m.file_type().is_symlink() && m.len() <= 8192)
            .and_then(|_| {
                let mut bytes = Vec::new();
                File::open(entry.path())
                    .ok()?
                    .take(8193)
                    .read_to_end(&mut bytes)
                    .ok()?;
                (bytes.len() <= 8192).then_some(bytes)
            });
        groups
            .entry(id.into())
            .or_default()
            .insert(kind.into(), bytes);
    }
    let mut attempted = 0u64;
    let mut unresolved = 0u64;
    let mut known_ids = 0u64;
    let mut verified_count = 0u64;
    let mut not_dispatched_count = 0u64;
    for (id, files) in groups {
        let parsed = files
            .get("json")
            .and_then(Option::as_deref)
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
        let scope = parsed.as_ref().and_then(|v| v.get("contract"));
        let scoped = scope.is_some_and(|c| {
            c.get("provider").and_then(Value::as_str) == Some(provider)
                && c.get("accountId").and_then(Value::as_str) == Some(account)
        });
        let attributable = scope.is_some_and(|c| {
            matches!(
                c.get("provider").and_then(Value::as_str),
                Some("spotify" | "youtube")
            ) && c
                .get("accountId")
                .and_then(Value::as_str)
                .is_some_and(|a| !a.is_empty() && a.len() <= 512)
        });
        if !scoped {
            if !attributable {
                unattributed += 1;
                fingerprints.push(digest(
                    serde_json::to_vec(&(id, &files))
                        .map_err(|_| "Cannot fingerprint intent history")?,
                ));
            }
            continue;
        }
        let valid_record = files
            .get("json")
            .and_then(Option::as_deref)
            .and_then(|bytes| serde_json::from_slice::<Record>(bytes).ok())
            .is_some_and(|r| {
                r.contract.version == 1
                    && r.contract.visibility == "private"
                    && r.contract.track_count == 0
                    && r.contract.description.is_none()
                    && r.review_token_hash.len() == 64
                    && r.review_token_hash.bytes().all(|b| b.is_ascii_hexdigit())
                    && r.contract.intent_id == id
                    && r.contract.title == format!("Burrata verification {id}")
                    && r.contract_hash
                        == digest(serde_json::to_vec(&r.contract).unwrap_or_default())
            });
        let has_attempt = files.contains_key("dispatch")
            || files.contains_key("state")
            || files.contains_key("created");
        if !has_attempt && valid_record {
            continue;
        } // An unused preview is not a creation attempt.
        fingerprints.push(digest(
            serde_json::to_vec(&(id.clone(), &files))
                .map_err(|_| "Cannot fingerprint intent history")?,
        ));
        attempted += 1;
        let object = |kind: &str| {
            files
                .get(kind)
                .and_then(Option::as_deref)
                .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        };
        let dispatch_valid = object("dispatch").is_some_and(|v| {
            v.as_object().is_some_and(|o| o.len() == 4)
                && v.get("status").and_then(Value::as_str) == Some("DISPATCH_LOCKED")
                && v.get("intentId").and_then(Value::as_str) == Some(id.as_str())
                && v.get("contractHash") == parsed.as_ref().and_then(|r| r.get("contractHash"))
                && v.get("mutationDispatched") == Some(&Value::Null)
        });
        let finite_records = files.iter().all(|(kind, bytes)| {
            kind == "json"
                || bytes
                    .as_deref()
                    .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
                    .is_some_and(|value| {
                        value.as_object().is_some_and(|o| {
                            o.keys().all(|key| {
                                [
                                    "status",
                                    "intentId",
                                    "contractHash",
                                    "requestId",
                                    "mutationDispatched",
                                    "provider",
                                    "diagnostic",
                                    "creationReceipt",
                                    "playlistId",
                                    "url",
                                    "accountId",
                                    "name",
                                    "visibility",
                                    "total",
                                    "verified",
                                ]
                                .contains(&key.as_str())
                            })
                        })
                    })
        });
        let valid = valid_record && dispatch_valid && finite_records;
        let created = object("created");
        let state = object("state");
        let valid_id = |id: &str| {
            if provider == "spotify" {
                id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric())
            } else {
                id.starts_with("PL")
                    && (12..=80).contains(&id.len())
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            }
        };
        let known = created
            .as_ref()
            .filter(|v| {
                v.get("intentId").and_then(Value::as_str) == Some(id.as_str())
                    && v.get("provider").and_then(Value::as_str) == Some(provider)
            })
            .and_then(|v| v.get("playlistId").and_then(Value::as_str))
            .filter(|id| valid_id(id));
        if known.is_some() {
            known_ids += 1;
        }
        let verified = valid
            && known.is_some()
            && state.as_ref().is_some_and(|v| {
                v.get("verified").and_then(Value::as_bool) == Some(true)
                    && v.get("name").and_then(Value::as_str)
                        == Some(format!("Burrata verification {id}").as_str())
                    && v.get("visibility").and_then(Value::as_str) == Some("private")
                    && v.get("total").and_then(Value::as_u64) == Some(0)
                    && v.get("requestId")
                        .and_then(Value::as_str)
                        .is_some_and(|request| {
                            v.get("creationReceipt")
                                .and_then(|r| super::bound_creation_receipt(r, request))
                                .is_some_and(|r| {
                                    r.get("mutationDispatched").and_then(Value::as_bool)
                                        == Some(true)
                                        && r.get("playlistId").and_then(Value::as_str) == known
                                })
                        })
                    && v.get("provider").and_then(Value::as_str) == Some(provider)
                    && v.get("accountId").and_then(Value::as_str) == Some(account)
                    && v.get("intentId").and_then(Value::as_str) == Some(id.as_str())
                    && v.get("playlistId").and_then(Value::as_str) == known
            });
        let not_dispatched = valid
            && state.as_ref().is_some_and(|v| {
                v.get("status").and_then(Value::as_str) == Some("NOT_DISPATCHED")
                    && v.get("requestId")
                        .and_then(Value::as_str)
                        .is_some_and(|request| {
                            v.pointer("/diagnostic/creationReceipt")
                                .and_then(|r| super::bound_creation_receipt(r, request))
                                .is_some_and(|r| {
                                    r.get("mutationDispatched").and_then(Value::as_bool)
                                        == Some(false)
                                })
                        })
            });
        if verified {
            verified_count += 1;
        } else if not_dispatched {
            not_dispatched_count += 1;
        } else {
            unresolved += 1;
        }
    }
    if truncated {
        unattributed += 1;
    }
    let total = legacy
        .get("unresolvedCount")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        + unresolved
        + unattributed;
    let disclosure = json!({"legacyAttemptCount":legacy["attemptCount"],"legacyUnresolvedCount":legacy["unresolvedCount"],"newAttemptCount":attempted,"newUnresolvedCount":unresolved,"newKnownIdCount":known_ids,"newVerifiedCount":verified_count,"newNotDispatchedCount":not_dispatched_count,"unattributedHistoryCount":unattributed,"historyTruncated":truncated,"totalUnresolvedCount":total,"outcome":if total>0{"UNKNOWN"}else{"NONE_UNRESOLVED"},"additionalNewIntent":true,"warning":if total>0{format!("Earlier creation outcomes remain unresolved ({total}). Some attempts may already have created playlists. Approving this creates an additional NEW private empty playlist; it does not retry or resolve earlier intents.")}else{"Approving this creates an additional NEW private empty playlist. No earlier unresolved creation is observed in the bounded history; this does not retry an earlier intent.".into()}});
    fingerprints.sort();
    let fingerprint = digest(
        serde_json::to_vec(&(fingerprints, truncated))
            .map_err(|_| "Cannot fingerprint intent history")?,
    );
    Ok((disclosure, fingerprint))
}
pub(super) fn preview(
    base: &Path,
    provider: &str,
    account: &str,
    revision: u64,
    generation: u64,
) -> Result<Value, String> {
    if !matches!(provider, "spotify" | "youtube") || account.is_empty() || account.len() > 512 {
        return Err("Invalid confirmed provider identity".into());
    }
    let dir = directory(base)?;
    if fs::read_dir(&dir)
        .map_err(|_| "Cannot inspect intents")?
        .take(401)
        .count()
        >= 400
    {
        return Err("Intent history requires review before creating another preview".into());
    }
    let intent_id = uuid::Uuid::new_v4().to_string();
    let title = format!("Burrata verification {intent_id}");
    let (legacy_attempt, legacy_fingerprint) = legacy_evidence(base, provider, account);
    let (prior_creations, history_fingerprint) =
        prior_intent_evidence(base, provider, account, &intent_id, &legacy_attempt)?;
    let contract = Contract {
        version: 1,
        intent_id: intent_id.clone(),
        provider: provider.into(),
        account_id: account.into(),
        account_revision: revision,
        generation,
        title,
        visibility: "private".into(),
        track_count: 0,
        description: None,
        expires_at: epoch()? + 600,
        legacy_attempt,
        legacy_fingerprint,
        prior_creations,
        history_fingerprint,
        process: PROCESS.0.clone(),
        deadline_ms: PROCESS.1.elapsed().as_millis() as u64 + 600_000,
    };
    let contract_hash = digest(serde_json::to_vec(&contract).map_err(|_| "Cannot encode intent")?);
    let review_token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let record = Record {
        contract: contract.clone(),
        contract_hash: contract_hash.clone(),
        review_token_hash: digest(review_token.as_bytes()),
    };
    let mut file = private_new(&dir.join(format!("{intent_id}.json")))?;
    file.write_all(&serde_json::to_vec(&record).map_err(|_| "Cannot encode intent")?)
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot persist intent")?;
    sync_dir(&dir)?;
    Ok(
        json!({"contract":contract,"contractHash":contract_hash,"reviewToken":review_token,"requiresExplicitApproval":true,"legacyUnknownOutcomesUnchanged":true,"legacyAttempt":contract.legacy_attempt,"priorCreations":contract.prior_creations}),
    )
}
fn reviewed(
    base: &Path,
    id: &str,
    token: &str,
    hash: &str,
    provider: &str,
    account: &str,
    revision: u64,
    generation: u64,
) -> Result<(PathBuf, Contract), String> {
    if !valid_uuid(id)
        || token.len() != 64
        || !token.bytes().all(|b| b.is_ascii_hexdigit())
        || hash.len() != 64
        || !hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Invalid intent approval".into());
    }
    let dir = directory(base)?;
    let path = dir.join(format!("{id}.json"));
    if !fs::symlink_metadata(&path)
        .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink() && m.len() <= 8192)
    {
        return Err("Intent is unavailable".into());
    }
    let mut bytes = Vec::new();
    File::open(&path)
        .and_then(|f| f.take(8193).read_to_end(&mut bytes))
        .map_err(|_| "Cannot read intent")?;
    if bytes.len() > 8192 {
        return Err("Invalid intent record".into());
    }
    let record: Record = serde_json::from_slice(&bytes).map_err(|_| "Invalid intent record")?;
    let c = record.contract;
    let calculated = digest(serde_json::to_vec(&c).map_err(|_| "Invalid intent contract")?);
    let (legacy, fingerprint) = legacy_evidence(base, provider, account);
    let (history, history_hash) = prior_intent_evidence(base, provider, account, id, &legacy)?;
    if history != c.prior_creations || history_hash != c.history_fingerprint {
        return Err(
            "Prior creation history changed; review a fresh additional-new-intent preview".into(),
        );
    }
    if legacy != c.legacy_attempt || fingerprint != c.legacy_fingerprint {
        return Err("An earlier creation outcome changed; review a fresh new-intent preview before proceeding".into());
    }
    if record.contract_hash != hash
        || calculated != hash
        || record.review_token_hash != digest(token.as_bytes())
        || c.version != 1
        || c.intent_id != id
        || c.provider != provider
        || c.account_id != account
        || c.account_revision != revision
        || c.generation != generation
        || c.process != PROCESS.0
        || PROCESS.1.elapsed().as_millis() as u64 >= c.deadline_ms
        || epoch()? >= c.expires_at
        || c.title != format!("Burrata verification {id}")
        || c.visibility != "private"
        || c.track_count != 0
        || c.description.is_some()
    {
        return Err(
            "Intent approval expired or no longer matches the confirmed account and contract"
                .into(),
        );
    }
    Ok((dir, c))
}
pub(super) fn validate(
    base: &Path,
    id: &str,
    token: &str,
    hash: &str,
    provider: &str,
    account: &str,
    revision: u64,
    generation: u64,
) -> Result<(), String> {
    reviewed(
        base, id, token, hash, provider, account, revision, generation,
    )
    .map(|_| ())
}
pub(super) fn consume(
    base: &Path,
    id: &str,
    token: &str,
    hash: &str,
    provider: &str,
    account: &str,
    revision: u64,
    generation: u64,
) -> Result<(Contract, File), String> {
    let (dir, c) = reviewed(
        base, id, token, hash, provider, account, revision, generation,
    )?;
    let mut lock = private_new(&dir.join(format!("{id}.dispatch")))?;
    lock.write_all(&serde_json::to_vec(&json!({"status":"DISPATCH_LOCKED","intentId":id,"contractHash":hash,"mutationDispatched":null})).map_err(|_|"Cannot encode dispatch record")?).and_then(|_|lock.sync_all()).map_err(|_|"Cannot persist dispatch lock")?;
    sync_dir(&dir)?;
    // This immutable lock is never truncated, removed or reopened for retry.
    let state = private_new(&dir.join(format!("{id}.state")))?;
    sync_dir(&dir)?;
    Ok((c, state))
}
pub(super) fn consume_fenced(
    base: &Path,
    id: &str,
    token: &str,
    hash: &str,
    provider: &str,
    account: &str,
    revision: u64,
    generation: u64,
    fences: &AccountFences,
) -> Result<(Contract, File, AccountFence), String> {
    let (_, approved) = reviewed(
        base, id, token, hash, provider, account, revision, generation,
    )?;
    let guard = fences.register(
        provider,
        account,
        revision,
        &approved.account_id,
        approved.account_revision,
    )?;
    // Register BEFORE consumption; every failure drops and unregisters the guard.
    let (contract, file) = consume(
        base, id, token, hash, provider, account, revision, generation,
    )?;
    Ok((contract, file, guard))
}
pub(super) fn persist_created(base: &Path, id: &str, receipt: &Value) -> Result<(), String> {
    if !valid_uuid(id) {
        return Err("Invalid intent identity".into());
    }
    let dir = directory(base)?;
    let mut file = private_new(&dir.join(format!("{id}.created")))?;
    file.write_all(&serde_json::to_vec(receipt).map_err(|_| "Cannot encode created identity")?)
        .and_then(|_| file.sync_all())
        .map_err(|_| {
            "Created identity could not be persisted; inspect the provider and do not retry"
        })?;
    sync_dir(&dir)
}
pub(super) async fn persist_created_then<F, Fut, T>(
    base: &Path,
    id: &str,
    receipt: &Value,
    after_persist: F,
) -> Result<T, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    persist_created(base, id, receipt)?;
    after_persist().await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_review_consumes_once_without_touching_legacy_marker() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join("verification-playlist-legacy.json");
        fs::write(&legacy, b"{\"status\":\"ATTEMPTED\"}").unwrap();
        let p = preview(temp.path(), "spotify", "account", 7, 8).unwrap();
        let id = p["contract"]["intentId"].as_str().unwrap();
        let token = p["reviewToken"].as_str().unwrap();
        let hash = p["contractHash"].as_str().unwrap();
        assert!(consume(temp.path(), id, token, hash, "spotify", "other", 7, 8).is_err());
        let (c, _) = consume(temp.path(), id, token, hash, "spotify", "account", 7, 8).unwrap();
        assert_eq!(c.title, format!("Burrata verification {id}"));
        assert!(temp
            .path()
            .join("verification-intents")
            .join(format!("{id}.dispatch"))
            .is_file());
        assert!(consume(temp.path(), id, token, hash, "spotify", "account", 7, 8).is_err());
        assert_eq!(fs::read(legacy).unwrap(), b"{\"status\":\"ATTEMPTED\"}");
    }
    #[test]
    fn review_rejects_changed_contract_token_revision_and_paths() {
        let temp = tempfile::tempdir().unwrap();
        let p = preview(temp.path(), "youtube", "channel", 1, 2).unwrap();
        let id = p["contract"]["intentId"].as_str().unwrap();
        let token = p["reviewToken"].as_str().unwrap();
        let hash = p["contractHash"].as_str().unwrap();
        for bad in ["../outside", "00000000-0000-0000-0000-000000000000"] {
            assert!(consume(temp.path(), bad, token, hash, "youtube", "channel", 1, 2).is_err());
        }
        assert!(consume(
            temp.path(),
            id,
            &"0".repeat(64),
            hash,
            "youtube",
            "channel",
            1,
            2
        )
        .is_err());
        assert!(consume(temp.path(), id, token, hash, "youtube", "channel", 2, 2).is_err());
        assert!(consume(
            temp.path(),
            id,
            token,
            &"0".repeat(64),
            "youtube",
            "channel",
            1,
            2
        )
        .is_err());
        assert!(consume(temp.path(), id, token, hash, "youtube", "channel", 1, 2).is_ok());
    }
    #[test]
    fn concurrent_approval_dispatches_once_and_created_identity_survives_state_loss() {
        let temp = tempfile::tempdir().unwrap();
        let p = preview(temp.path(), "spotify", "account", 1, 2).unwrap();
        let id = p["contract"]["intentId"].as_str().unwrap().to_string();
        let token = p["reviewToken"].as_str().unwrap().to_string();
        let hash = p["contractHash"].as_str().unwrap().to_string();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let (base, id, token, hash, barrier) = (
                    temp.path().to_path_buf(),
                    id.clone(),
                    token.clone(),
                    hash.clone(),
                    barrier.clone(),
                );
                std::thread::spawn(move || {
                    barrier.wait();
                    consume(&base, &id, &token, &hash, "spotify", "account", 1, 2).is_ok()
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .filter(|ok| *ok)
                .count(),
            1
        );
        let identity = json!({"playlistId":"fixture-known-id","intentId":id});
        persist_created(temp.path(), &id, &identity).unwrap();
        let dir = temp.path().join("verification-intents");
        fs::write(dir.join(format!("{id}.state")), b"broken-state").unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(dir.join(format!("{id}.created"))).unwrap())
                .unwrap(),
            identity
        );
        assert!(persist_created(temp.path(), &id, &identity).is_err());
    }

    #[test]
    fn expiry_and_process_restart_are_rejected_even_with_matching_updated_hash() {
        for process_changed in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let p = preview(temp.path(), "spotify", "account", 1, 2).unwrap();
            let id = p["contract"]["intentId"].as_str().unwrap();
            let token = p["reviewToken"].as_str().unwrap();
            let path = temp
                .path()
                .join("verification-intents")
                .join(format!("{id}.json"));
            let mut record: Record = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            if process_changed {
                record.contract.process = uuid::Uuid::new_v4().to_string();
            } else {
                record.contract.expires_at = epoch().unwrap().saturating_sub(1);
                record.contract.deadline_ms = 0;
            }
            record.contract_hash = digest(serde_json::to_vec(&record.contract).unwrap());
            let hash = record.contract_hash.clone();
            fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
            assert!(consume(temp.path(), id, token, &hash, "spotify", "account", 1, 2).is_err());
            assert!(!temp
                .path()
                .join("verification-intents")
                .join(format!("{id}.dispatch"))
                .exists());
        }
    }
    #[tokio::test]
    async fn identity_persistence_failure_prevents_readback_and_success_precedes_readback_failure()
    {
        let temp = tempfile::tempdir().unwrap();
        let p = preview(temp.path(), "youtube", "channel", 1, 2).unwrap();
        let id = p["contract"]["intentId"].as_str().unwrap();
        let dir = temp.path().join("verification-intents");
        let created = dir.join(format!("{id}.created"));
        let called = std::sync::atomic::AtomicBool::new(false);
        let receipt = json!({"intentId":id,"playlistId":"PLfixture123456789"});
        // Actual filesystem fault: reserved destination is a directory, so identity cannot be written.
        fs::create_dir(&created).unwrap();
        let failed = persist_created_then(temp.path(), id, &receipt, || async {
            called.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert!(failed.is_err());
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
        fs::remove_dir(&created).unwrap();
        let readback_failure: Result<(), String> =
            persist_created_then(temp.path(), id, &receipt, || async {
                assert_eq!(
                    serde_json::from_slice::<Value>(&fs::read(&created).unwrap()).unwrap(),
                    receipt
                );
                called.store(true, std::sync::atomic::Ordering::SeqCst);
                Err("fixture readback failure".into())
            })
            .await;
        assert!(readback_failure.is_err());
        assert!(called.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(created).unwrap()).unwrap(),
            receipt
        );
    }
    #[test]
    fn account_fence_interleaving_cancels_only_old_provider_and_cleans_every_guard() {
        let fences = AccountFences::default();
        let old = fences
            .register("spotify", "account-a", 1, "account-a", 1)
            .unwrap();
        let youtube = fences
            .register("youtube", "channel", 3, "channel", 3)
            .unwrap();
        // Production record_account invalidates under the same state lock used to register.
        fences.invalidate("spotify").unwrap();
        assert!(old.cancel_flag().load(std::sync::atomic::Ordering::SeqCst));
        assert!(!youtube
            .cancel_flag()
            .load(std::sync::atomic::Ordering::SeqCst));
        assert!(fences
            .register("spotify", "account-b", 2, "account-a", 1)
            .is_err());
        assert!(fences
            .register("spotify", "account-a", 3, "account-a", 1)
            .is_err());
        assert!(fences
            .register("spotify", "account-b", 2, "account-b", 2)
            .is_err());
        drop(old);
        let fresh = fences
            .register("spotify", "account-b", 2, "account-b", 2)
            .unwrap();
        assert!(!fresh
            .cancel_flag()
            .load(std::sync::atomic::Ordering::SeqCst));
        drop(youtube);
        drop(fresh);
        assert!(fences.0.lock().unwrap().is_empty());
    }

    #[test]
    fn preview_surfaces_exact_legacy_unknown_without_modifying_marker() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(format!(
            "verification-playlist-{}.json",
            digest(b"spotify:account")
        ));
        fs::write(&path, br#"{"status":"ATTEMPTED"}"#).unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        let p = preview(temp.path(), "spotify", "account", 1, 2).unwrap();
        assert_eq!(p["legacyAttempt"]["outcome"], "UNKNOWN");
        assert_eq!(p["legacyAttempt"]["unresolvedCount"], 1);
        assert_eq!(p["legacyAttempt"]["additionalNewIntent"], true);
        assert!(p["legacyAttempt"]["warning"]
            .as_str()
            .unwrap()
            .contains("may already have created"));
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
        assert_eq!(fs::read(path).unwrap(), br#"{"status":"ATTEMPTED"}"#);
    }
    #[test]
    fn changed_legacy_state_requires_fresh_preview_without_consuming_approval() {
        let temp = tempfile::tempdir().unwrap();
        let p = preview(temp.path(), "spotify", "account", 1, 2).unwrap();
        let id = p["contract"]["intentId"].as_str().unwrap();
        let path = temp.path().join(format!(
            "verification-playlist-{}.json",
            digest(b"spotify:account")
        ));
        fs::write(&path, br#"{"status":"ATTEMPTED"}"#).unwrap();
        assert!(consume_fenced(
            temp.path(),
            id,
            p["reviewToken"].as_str().unwrap(),
            p["contractHash"].as_str().unwrap(),
            "spotify",
            "account",
            1,
            2,
            &AccountFences::default()
        )
        .is_err());
        assert!(!temp
            .path()
            .join("verification-intents")
            .join(format!("{id}.dispatch"))
            .exists());
        let fresh = preview(temp.path(), "spotify", "account", 1, 2).unwrap();
        assert_eq!(fresh["contract"]["legacyAttempt"]["outcome"], "UNKNOWN");
    }
    #[test]
    fn admission_failure_never_consumes_intent_and_failure_drops_fence() {
        let temp = tempfile::tempdir().unwrap();
        let p = preview(temp.path(), "spotify", "account", 1, 2).unwrap();
        let id = p["contract"]["intentId"].as_str().unwrap();
        let fences = AccountFences::default();
        let held = (0..32)
            .map(|i| {
                fences
                    .register(&format!("fixture-{i}"), "account", 1, "account", 1)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(consume_fenced(
            temp.path(),
            id,
            p["reviewToken"].as_str().unwrap(),
            p["contractHash"].as_str().unwrap(),
            "spotify",
            "account",
            1,
            2,
            &fences
        )
        .is_err());
        assert!(!temp
            .path()
            .join("verification-intents")
            .join(format!("{id}.dispatch"))
            .exists());
        drop(held);
        let admitted = consume_fenced(
            temp.path(),
            id,
            p["reviewToken"].as_str().unwrap(),
            p["contractHash"].as_str().unwrap(),
            "spotify",
            "account",
            1,
            2,
            &fences,
        )
        .unwrap();
        drop(admitted);
        assert!(fences.0.lock().unwrap().is_empty());
        assert!(consume_fenced(
            temp.path(),
            id,
            p["reviewToken"].as_str().unwrap(),
            p["contractHash"].as_str().unwrap(),
            "spotify",
            "account",
            1,
            2,
            &fences
        )
        .is_err());
        assert!(fences.0.lock().unwrap().is_empty());
    }
    #[test]
    fn legacy_and_new_share_account_admission() {
        let fences = AccountFences::default();
        let legacy = fences
            .register("spotify", "account", 1, "account", 1)
            .unwrap();
        assert!(fences
            .register("spotify", "account", 1, "account", 1)
            .is_err());
        drop(legacy);
        let new = fences
            .register("spotify", "account", 1, "account", 1)
            .unwrap();
        assert!(fences
            .register("spotify", "account", 1, "account", 1)
            .is_err());
        drop(new);
        assert!(fences.0.lock().unwrap().is_empty());
    }
    #[test]
    fn prior_new_unknown_is_disclosed_and_history_changes_reject_before_consumption() {
        let temp = tempfile::tempdir().unwrap();
        let first = preview(temp.path(), "spotify", "account", 1, 2).unwrap();
        let first_id = first["contract"]["intentId"].as_str().unwrap();
        let (_, mut state) = consume(
            temp.path(),
            first_id,
            first["reviewToken"].as_str().unwrap(),
            first["contractHash"].as_str().unwrap(),
            "spotify",
            "account",
            1,
            2,
        )
        .unwrap();
        state.write_all(br#"{"status":"UNKNOWN_OUTCOME"}"#).unwrap();
        state.sync_all().unwrap();
        let second = preview(temp.path(), "spotify", "account", 1, 2).unwrap();
        assert_eq!(
            second["contract"]["priorCreations"]["newUnresolvedCount"],
            1
        );
        assert_eq!(
            second["contract"]["priorCreations"]["totalUnresolvedCount"],
            1
        );
        assert_eq!(second["contract"]["priorCreations"]["outcome"], "UNKNOWN");
        let second_id = second["contract"]["intentId"].as_str().unwrap();
        persist_created(temp.path(),first_id,&json!({"intentId":first_id,"provider":"spotify","playlistId":"1234567890123456789012"})).unwrap();
        assert!(consume_fenced(
            temp.path(),
            second_id,
            second["reviewToken"].as_str().unwrap(),
            second["contractHash"].as_str().unwrap(),
            "spotify",
            "account",
            1,
            2,
            &AccountFences::default()
        )
        .is_err());
        assert!(!temp
            .path()
            .join("verification-intents")
            .join(format!("{second_id}.dispatch"))
            .exists());
        let fresh = preview(temp.path(), "spotify", "account", 1, 2).unwrap();
        assert_eq!(fresh["priorCreations"]["newKnownIdCount"], 1);
        assert_eq!(fresh["priorCreations"]["newUnresolvedCount"], 1);
    }
    #[test]
    fn another_accounts_history_never_exposes_identifiers_or_blocks_current_preview() {
        let temp = tempfile::tempdir().unwrap();
        let p = preview(temp.path(), "spotify", "other-private-account", 1, 2).unwrap();
        let id = p["contract"]["intentId"].as_str().unwrap();
        consume(
            temp.path(),
            id,
            p["reviewToken"].as_str().unwrap(),
            p["contractHash"].as_str().unwrap(),
            "spotify",
            "other-private-account",
            1,
            2,
        )
        .unwrap();
        let own = preview(temp.path(), "spotify", "account", 1, 2).unwrap();
        assert_eq!(own["priorCreations"]["newUnresolvedCount"], 0);
        assert!(!serde_json::to_string(&own)
            .unwrap()
            .contains("other-private-account"));
        consume(
            temp.path(),
            own["contract"]["intentId"].as_str().unwrap(),
            own["reviewToken"].as_str().unwrap(),
            own["contractHash"].as_str().unwrap(),
            "spotify",
            "account",
            1,
            2,
        )
        .unwrap();
    }
    #[test]
    fn malformed_same_account_attempt_remains_unknown_without_exposing_raw_state() {
        let temp = tempfile::tempdir().unwrap();
        let first = preview(temp.path(), "youtube", "channel", 1, 2).unwrap();
        let id = first["contract"]["intentId"].as_str().unwrap();
        consume(
            temp.path(),
            id,
            first["reviewToken"].as_str().unwrap(),
            first["contractHash"].as_str().unwrap(),
            "youtube",
            "channel",
            1,
            2,
        )
        .unwrap();
        fs::write(
            temp.path()
                .join("verification-intents")
                .join(format!("{id}.state")),
            b"malformed-private-state",
        )
        .unwrap();
        let fresh = preview(temp.path(), "youtube", "channel", 1, 2).unwrap();
        assert_eq!(fresh["priorCreations"]["totalUnresolvedCount"], 1);
        assert_eq!(fresh["priorCreations"]["outcome"], "UNKNOWN");
        assert!(!serde_json::to_string(&fresh)
            .unwrap()
            .contains("malformed-private-state"));
    }
}
