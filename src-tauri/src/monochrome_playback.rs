//! Optional, ephemeral lossless playback path for the Monochrome resolver.
//!
//! This module deliberately has no credential discovery. The application must
//! inject an authorized auth source and a bounded native relay before any
//! resolver request can occur.

use crate::{
    models::{Candidate, Track},
    monochrome_resolver::{
        assess_playback_resource, AuthConfig, EphemeralMediaKey, RecordingRequest, ResolverClient,
        ResolverFailure, ResolverMatchFailure, TargetQuality,
    },
    native_output::{AudioSource, CencDecryptionKey, EncryptedMp4Audio},
    playback_sources::PlaybackDeadline,
    recording_match::MatchLevel,
};
use serde_json::{json, Value};
use std::{fmt, path::PathBuf, sync::Arc};
use tokio::sync::watch;

pub(crate) struct NativeRelayRegistrar {
    transport: Arc<crate::webview_audio_transport::WebviewAudioTransport>,
}

impl NativeRelayRegistrar {
    pub(crate) fn new(
        transport: Arc<crate::webview_audio_transport::WebviewAudioTransport>,
    ) -> Self {
        Self { transport }
    }
}

#[async_trait::async_trait]
impl MonochromeResourceRegistrar for NativeRelayRegistrar {
    async fn register_cenc(
        &self,
        generation: u64,
        track_id: &str,
        remote_url: &str,
        media_key: EphemeralMediaKey,
        ffmpeg: PathBuf,
        deadline: PlaybackDeadline,
        cancel: watch::Receiver<bool>,
    ) -> Result<AudioSource, String> {
        let key = CencDecryptionKey::from_hex(media_key.expose_for_transport())?;
        let lease = self
            .transport
            .register_native_cenc_input(generation, track_id, remote_url, &[], deadline, cancel)
            .await?;
        let input = lease.loopback_url().to_owned();
        Ok(AudioSource {
            input,
            ffmpeg,
            headers: vec![],
            decoder_hint: None,
            cenc: Some(EncryptedMp4Audio::with_registered_lease(key, lease)),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthSourceFailure {
    Unavailable,
    Failed,
}

/// Explicitly authorized source of a current or freshly refreshed resolver
/// auth lease. Implementations must not scrape browser profiles or copy
/// provider credentials implicitly.
#[async_trait::async_trait]
pub(crate) trait MonochromeAuthSource: Send + Sync {
    async fn current_auth(&self) -> Result<Option<AuthConfig>, AuthSourceFailure>;

    async fn fresh_auth(&self) -> Result<Option<AuthConfig>, AuthSourceFailure> {
        Ok(None)
    }
}

/// Adapts an accepted encrypted resource to the bounded native relay. The
/// concrete production adapter is expected to retain its registered lease in
/// AudioSource so preload/adoption and disposal follow normal source ownership.
#[async_trait::async_trait]
pub(crate) trait MonochromeResourceRegistrar: Send + Sync {
    async fn register_cenc(
        &self,
        generation: u64,
        track_id: &str,
        remote_url: &str,
        media_key: EphemeralMediaKey,
        ffmpeg: PathBuf,
        deadline: PlaybackDeadline,
        cancel: watch::Receiver<bool>,
    ) -> Result<AudioSource, String>;
}

#[derive(Debug)]
pub(crate) enum MonochromePlaybackFailure {
    AuthUnavailable,
    AuthFailed,
    Resolver(ResolverFailure),
    Match(ResolverMatchFailure),
    ResourceUnavailable,
    Cancelled,
    Deadline,
    Relay,
}

impl MonochromePlaybackFailure {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::AuthUnavailable => "auth_unavailable",
            Self::AuthFailed => "auth_failed",
            Self::Resolver(ResolverFailure::Unauthorized) => "resolver_unauthorized",
            Self::Resolver(ResolverFailure::Forbidden) => "resolver_forbidden",
            Self::Resolver(ResolverFailure::VerificationRequired) => {
                "resolver_verification_required"
            }
            Self::Resolver(ResolverFailure::RateLimited) => "resolver_rate_limited",
            Self::Resolver(ResolverFailure::NotFound) => "resolver_not_found",
            Self::Resolver(ResolverFailure::Http(_)) => "resolver_http_error",
            Self::Resolver(ResolverFailure::InvalidRequest) => "resolver_invalid_request",
            Self::Resolver(ResolverFailure::InvalidAuth) => "resolver_invalid_auth",
            Self::Resolver(ResolverFailure::Transport) => "resolver_transport_error",
            Self::Resolver(ResolverFailure::ResponseTooLarge) => "resolver_response_too_large",
            Self::Resolver(ResolverFailure::InvalidResponse) => "resolver_invalid_response",
            Self::Match(ResolverMatchFailure::MissingRecordingIdentity) => "match_missing_identity",
            Self::Match(ResolverMatchFailure::SourceBindingMismatch) => "match_source_binding",
            Self::Match(ResolverMatchFailure::ConflictingIsrc) => "match_conflicting_isrc",
            Self::Match(ResolverMatchFailure::UnsupportedResource) => "match_unsupported_resource",
            Self::Match(ResolverMatchFailure::MatchRejected) => "match_rejected",
            Self::ResourceUnavailable => "resource_unavailable",
            Self::Cancelled => "cancelled",
            Self::Deadline => "deadline",
            Self::Relay => "relay_failed",
        }
    }
}

impl fmt::Display for MonochromePlaybackFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AuthUnavailable => "Monochrome resolver authorization is unavailable",
            Self::AuthFailed => "Monochrome resolver authorization could not be refreshed",
            Self::Resolver(_) => "Monochrome resolver request failed",
            Self::Match(_) => "Monochrome resolver result did not pass recording verification",
            Self::ResourceUnavailable => {
                "Monochrome resolver returned no supported lossless resource"
            }
            Self::Cancelled => "Cancelled",
            Self::Deadline => "Playback source resolution exceeded its deadline",
            Self::Relay => "Monochrome media resource could not be registered for native playback",
        })
    }
}

pub(crate) struct MonochromePlaybackService {
    client: ResolverClient,
    auth: Arc<dyn MonochromeAuthSource>,
    registrar: Arc<dyn MonochromeResourceRegistrar>,
}

impl fmt::Debug for MonochromePlaybackService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MonochromePlaybackService")
            .field("client", &self.client)
            .field("auth", &"[INJECTED]")
            .field("registrar", &"[INJECTED]")
            .finish()
    }
}

pub(crate) struct MonochromeResolvedPlayback {
    pub(crate) candidate: Candidate,
    pub(crate) source: AudioSource,
    pub(crate) quality: Value,
    pub(crate) duration: Option<f64>,
}

pub(crate) fn same_resolver_identity(old: &Candidate, new: &Candidate) -> bool {
    let old_metadata = old.source_metadata.as_ref();
    let new_metadata = new.source_metadata.as_ref();
    old.provider == "monochrome"
        && new.provider == "monochrome"
        && old.id == new.id
        && old_metadata
            .and_then(|metadata| metadata.get("resolverProtocol"))
            .and_then(Value::as_str)
            == Some("v2")
        && old_metadata.and_then(|metadata| metadata.get("trackId"))
            == new_metadata.and_then(|metadata| metadata.get("trackId"))
        && old_metadata.and_then(|metadata| metadata.get("resolverSource"))
            == new_metadata.and_then(|metadata| metadata.get("resolverSource"))
        && new_metadata
            .and_then(|metadata| metadata.get("resolverProtocol"))
            .and_then(Value::as_str)
            == Some("v2")
}

impl MonochromePlaybackService {
    pub(crate) fn new(
        endpoint: &str,
        auth: Arc<dyn MonochromeAuthSource>,
        registrar: Arc<dyn MonochromeResourceRegistrar>,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            client: ResolverClient::new(endpoint)?,
            auth,
            registrar,
        })
    }

    /// Resolve one verified lossless CENC resource. `Ok(None)` means the
    /// resolver did not produce an eligible path; errors are stable and safe
    /// for logs. No auth or media data is persisted by this service.
    pub(crate) async fn resolve(
        &self,
        requested: &Track,
        level: MatchLevel,
        ffmpeg: PathBuf,
        generation: u64,
        deadline: PlaybackDeadline,
        cancel: watch::Receiver<bool>,
    ) -> Result<Option<MonochromeResolvedPlayback>, MonochromePlaybackFailure> {
        let auth = bounded(deadline, cancel.clone(), self.auth.current_auth())
            .await?
            .map_err(|_| MonochromePlaybackFailure::AuthFailed)?
            .ok_or(MonochromePlaybackFailure::AuthUnavailable)?;

        let request = RecordingRequest {
            title: requested.title.clone(),
            artists: vec![requested.artist.clone()],
            album: requested.album.clone(),
            isrc: requested.isrc.clone(),
            duration_seconds: requested.duration_seconds,
            quality: TargetQuality::Lossless,
        };
        let response = match bounded(
            deadline,
            cancel.clone(),
            self.client.resolve(&request, &auth),
        )
        .await?
        {
            Ok(response) => response,
            Err(error) if error.needs_turnstile_refresh() => {
                let fresh = bounded(deadline, cancel.clone(), self.auth.fresh_auth())
                    .await?
                    .map_err(|_| MonochromePlaybackFailure::AuthFailed)?
                    .ok_or(MonochromePlaybackFailure::AuthFailed)?;
                bounded(
                    deadline,
                    cancel.clone(),
                    self.client.resolve(&request, &fresh),
                )
                .await?
                .map_err(MonochromePlaybackFailure::Resolver)?
            }
            Err(error) => return Err(MonochromePlaybackFailure::Resolver(error)),
        };
        let assessed = match assess_playback_resource(requested, &response, level) {
            Ok(value) => value,
            Err(ResolverMatchFailure::UnsupportedResource) => return Ok(None),
            Err(error) => return Err(MonochromePlaybackFailure::Match(error)),
        };
        let Some(key) = assessed.resource.media_key.clone() else {
            return Ok(None);
        };
        let track_id = assessed
            .candidate
            .source_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("trackId"))
            .and_then(Value::as_str)
            .ok_or(MonochromePlaybackFailure::ResourceUnavailable)?;
        let source = bounded(
            deadline,
            cancel.clone(),
            self.registrar.register_cenc(
                generation,
                track_id,
                &assessed.resource.url,
                key,
                ffmpeg,
                deadline,
                cancel,
            ),
        )
        .await?
        .map_err(|_| MonochromePlaybackFailure::Relay)?;

        let candidate = assessed.candidate.clone();
        let resource = assessed.resource;
        let quality = json!({
            "provider": "monochrome",
            "resourceKind": resource.kind,
            "delivery": "direct",
            "codec": resource.codec,
            "container": resource.container,
            "mimeType": resource.mime_type,
            "lossless": resource.lossless,
            "quality": resource.quality,
            "bitDepth": resource.bit_depth,
            "sampleRateHz": resource.sample_rate_hz,
            "channels": resource.channels,
            "bitrateKbps": resource.bitrate_kbps,
            "source": resource.source,
            "matchLevel": format!("{level:?}").to_ascii_lowercase(),
        });
        Ok(Some(MonochromeResolvedPlayback {
            duration: candidate.duration_seconds,
            candidate,
            source,
            quality,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NoAuth(AtomicUsize);
    #[async_trait::async_trait]
    impl MonochromeAuthSource for NoAuth {
        async fn current_auth(&self) -> Result<Option<AuthConfig>, AuthSourceFailure> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    }

    struct NeverRegister;
    #[async_trait::async_trait]
    impl MonochromeResourceRegistrar for NeverRegister {
        async fn register_cenc(
            &self,
            _: u64,
            _: &str,
            _: &str,
            _: EphemeralMediaKey,
            _: PathBuf,
            _: PlaybackDeadline,
            _: watch::Receiver<bool>,
        ) -> Result<AudioSource, String> {
            panic!("registrar must not be reached without auth")
        }
    }

    fn candidate(id: &str, source: &str) -> Candidate {
        Candidate {
            id: format!("monochrome:{id}"),
            provider: "monochrome".into(),
            source_metadata: Some(json!({
                "resolverProtocol":"v2",
                "trackId":id,
                "resolverSource":source,
            })),
            ..Candidate::default()
        }
    }

    #[test]
    fn resolver_refresh_is_limited_to_auth_challenges() {
        assert!(ResolverFailure::Unauthorized.needs_turnstile_refresh());
        assert!(ResolverFailure::VerificationRequired.needs_turnstile_refresh());
        assert!(!ResolverFailure::Forbidden.needs_turnstile_refresh());
        assert!(!ResolverFailure::RateLimited.needs_turnstile_refresh());
    }

    #[test]
    fn recovery_requires_same_stable_v2_id_and_source() {
        let old = candidate("B0ABC123", "amazon");
        assert!(same_resolver_identity(
            &old,
            &candidate("B0ABC123", "amazon")
        ));
        assert!(!same_resolver_identity(
            &old,
            &candidate("B0ABC124", "amazon")
        ));
        assert!(!same_resolver_identity(
            &old,
            &candidate("B0ABC123", "tidal")
        ));
        let legacy = Candidate {
            provider: "monochrome".into(),
            ..Candidate::default()
        };
        assert!(!same_resolver_identity(&legacy, &old));
    }

    #[tokio::test]
    async fn route_is_inert_without_explicit_auth_and_honors_cancel() {
        let auth = Arc::new(NoAuth(AtomicUsize::new(0)));
        let service = MonochromePlaybackService::new(
            "https://resolver.example",
            auth.clone(),
            Arc::new(NeverRegister),
        )
        .unwrap();
        let track = Track {
            title: "Title".into(),
            artist: "Artist".into(),
            ..Track::default()
        };
        let deadline = PlaybackDeadline::after(std::time::Duration::from_secs(3));
        let (_cancel_tx, cancel) = watch::channel(false);
        let result = service
            .resolve(
                &track,
                MatchLevel::Balanced,
                PathBuf::new(),
                1,
                deadline,
                cancel,
            )
            .await;
        assert!(matches!(
            result,
            Err(MonochromePlaybackFailure::AuthUnavailable)
        ));
        assert_eq!(auth.0.load(Ordering::SeqCst), 1);

        let (cancel_tx, cancel) = watch::channel(false);
        cancel_tx.send_replace(true);
        let result = service
            .resolve(
                &track,
                MatchLevel::Balanced,
                PathBuf::new(),
                1,
                deadline,
                cancel,
            )
            .await;
        assert!(matches!(result, Err(MonochromePlaybackFailure::Cancelled)));
        assert_eq!(auth.0.load(Ordering::SeqCst), 1);
    }
}

async fn bounded<T, F>(
    deadline: PlaybackDeadline,
    cancel: watch::Receiver<bool>,
    future: F,
) -> Result<T, MonochromePlaybackFailure>
where
    F: std::future::Future<Output = T>,
{
    tokio::select! {
        biased;
        _ = crate::playback_sources::wait_for_cancellation(cancel) => Err(MonochromePlaybackFailure::Cancelled),
        _ = tokio::time::sleep_until(deadline.instant()) => Err(MonochromePlaybackFailure::Deadline),
        result = future => Ok(result),
    }
}
