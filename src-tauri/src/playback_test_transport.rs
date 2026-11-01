//! Synthetic TLS trust for isolated resolver/decoder composition tests only.
//! No environment flag, broad certificate bypass, real provider host, or
//! production-build registration exists. Each RAII entry owns one exact host.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Clone)]
struct Fixture {
    address: SocketAddr,
    ca: Arc<Vec<u8>>,
}
fn fixtures() -> &'static Mutex<HashMap<String, Fixture>> {
    static FIXTURES: OnceLock<Mutex<HashMap<String, Fixture>>> = OnceLock::new();
    FIXTURES.get_or_init(Mutex::default)
}
pub(crate) struct Registration(String);
impl Drop for Registration {
    fn drop(&mut self) {
        fixtures().lock().unwrap().remove(&self.0);
    }
}
pub(crate) fn register(host: String, address: SocketAddr, ca: Vec<u8>) -> Registration {
    assert!(host.ends_with(".fixture.sndcdn.com") || host.ends_with(".fixture.googlevideo.com"));
    assert!(host
        .split('.')
        .next()
        .is_some_and(|value| value.len() >= 16));
    assert_eq!(address, SocketAddr::from(([8, 8, 8, 8], 443)));
    assert!(!ca.is_empty());
    assert!(fixtures()
        .lock()
        .unwrap()
        .insert(
            host.clone(),
            Fixture {
                address,
                ca: Arc::new(ca)
            }
        )
        .is_none());
    Registration(host)
}
fn lookup(url: &url::Url) -> Option<Fixture> {
    if url.scheme() != "https" || url.port_or_known_default() != Some(443) {
        return None;
    }
    fixtures().lock().unwrap().get(url.host_str()?).cloned()
}
pub(crate) fn fixture_address(url: &url::Url) -> Option<SocketAddr> {
    lookup(url).map(|fixture| fixture.address)
}
pub(crate) fn configure_preflight(
    builder: reqwest::ClientBuilder,
    url: &url::Url,
) -> Result<reqwest::ClientBuilder, String> {
    match lookup(url) {
        Some(fixture) => Ok(builder.add_root_certificate(
            reqwest::Certificate::from_pem(&fixture.ca)
                .map_err(|_| "Invalid synthetic test root")?,
        )),
        None => Ok(builder),
    }
}
pub(crate) fn configure_native(
    builder: stream_download::http::reqwest::ClientBuilder,
    url: &url::Url,
) -> Result<stream_download::http::reqwest::ClientBuilder, String> {
    match lookup(url) {
        Some(fixture) => Ok(builder
            .no_proxy()
            .resolve(url.host_str().unwrap(), fixture.address)
            .add_root_certificate(
                stream_download::http::reqwest::Certificate::from_pem(&fixture.ca)
                    .map_err(|_| "Invalid synthetic test root")?,
            )),
        None => Ok(builder),
    }
}

#[test]
fn synthetic_tls_registration_is_exact_host_https_and_raii_scoped() {
    for suffix in ["sndcdn.com", "googlevideo.com"] {
        let host = format!("{}.fixture.{suffix}", uuid::Uuid::new_v4().simple());
        let url = url::Url::parse(&format!("https://{host}/fixture.mp3")).unwrap();
        assert!(fixture_address(&url).is_none());
        let guard = register(
            host.clone(),
            ([8, 8, 8, 8], 443).into(),
            b"test-only-not-a-valid-certificate".to_vec(),
        );
        assert_eq!(fixture_address(&url), Some(([8, 8, 8, 8], 443).into()));
        for other in [
            format!("http://{host}/a"),
            format!("https://{host}:8443/a"),
            format!("https://other.{host}/a"),
            "https://cf-media.sndcdn.com/fixture.mp3".into(),
            "https://rr1---sn-test.googlevideo.com/videoplayback".into(),
            "https://www.youtube.com/watch?v=Syn11111111".into(),
        ] {
            assert!(fixture_address(&url::Url::parse(&other).unwrap()).is_none());
        }
        drop(guard);
        assert!(fixture_address(&url).is_none());
    }
}
