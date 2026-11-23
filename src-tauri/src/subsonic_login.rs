pub(crate) struct StrawberryLogin {
    pub(crate) endpoint: String,
    pub(crate) username: String,
    pub(crate) password: String,
}

#[cfg(target_os = "linux")]
pub(crate) use linux::discover_strawberry;

#[cfg(not(target_os = "linux"))]
pub(crate) async fn discover_strawberry() -> Option<StrawberryLogin> {
    None
}

#[cfg(target_os = "linux")]
mod linux {
    use super::StrawberryLogin;
    use crate::subsonic_client::{canonical_endpoint, canonical_username};
    use std::{
        collections::HashMap,
        fs::OpenOptions,
        io::Read,
        os::unix::fs::OpenOptionsExt,
        path::{Path, PathBuf},
        time::Duration,
    };
    use zbus::{
        zvariant::{OwnedObjectPath, OwnedValue, Value},
        Connection, Proxy,
    };

    const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
    const SECRET_SERVICE: &str = "org.freedesktop.secrets";

    pub(crate) async fn discover_strawberry() -> Option<StrawberryLogin> {
        tokio::time::timeout(Duration::from_secs(4), discover())
            .await
            .ok()?
    }

    async fn discover() -> Option<StrawberryLogin> {
        let path = dirs::config_dir()?.join("strawberry/strawberry.conf");
        let (endpoint, username) = configured_account(path.clone()).await?;
        let password = read_password().await?;
        // Strawberry's keyring entry is scoped to the service, not the account.
        // Do not pair a secret with settings that changed during lookup.
        if configured_account(path).await? != (endpoint.clone(), username.clone()) {
            return None;
        }
        Some(StrawberryLogin {
            endpoint,
            username,
            password,
        })
    }

    async fn configured_account(path: PathBuf) -> Option<(String, String)> {
        tokio::task::spawn_blocking(move || read_account(&path))
            .await
            .ok()?
    }

    fn read_account(path: &Path) -> Option<(String, String)> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return None;
        }
        parse_config(std::str::from_utf8(&bytes).ok()?)
    }

    fn parse_config(config: &str) -> Option<(String, String)> {
        let mut in_subsonic = false;
        let mut url = None;
        let mut username = None;
        let mut enabled = None;
        for line in config.trim_start_matches('\u{feff}').lines().map(str::trim) {
            if line.starts_with('[') {
                in_subsonic = line == "[Subsonic]";
            } else if in_subsonic {
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let field = match key.trim() {
                    "url" => &mut url,
                    "username" => &mut username,
                    "enabled" => &mut enabled,
                    _ => continue,
                };
                if field.replace(value.trim()).is_some() {
                    return None;
                }
            }
        }
        if enabled.is_some_and(|enabled| enabled != "true") {
            return None;
        }
        let url = url?;
        let endpoint = if let Some(payload) = url
            .strip_prefix("@Variant(")
            .and_then(|v| v.strip_suffix(')'))
        {
            let variant = decode_qt_value(payload)?;
            // QDataStream Qt_4_0 QVariant: QUrl type (17), then its encoded QByteArray.
            let (header, body) = variant.split_at_checked(8)?;
            if header[..4] != 17_u32.to_be_bytes()
                || u32::from_be_bytes(header[4..].try_into().ok()?) as usize != body.len()
            {
                return None;
            }
            canonical_endpoint(std::str::from_utf8(body).ok()?).ok()?
        } else {
            let plain_url = decode_qt_string(url)?;
            canonical_endpoint(&plain_url).ok()?
        };
        let raw_username = decode_qt_string(username?)?;
        let username = canonical_username(&raw_username).ok()?;
        (username == raw_username).then_some((endpoint, username))
    }

    fn decode_qt_string(value: &str) -> Option<String> {
        let value = if let Some(value) = value.strip_prefix('"') {
            value.strip_suffix('"')?
        } else {
            if value.contains([',', ';', '"']) {
                return None;
            }
            value
        };
        let value = if value.starts_with("@@") {
            &value[1..]
        } else {
            if value.starts_with('@') {
                return None;
            }
            value
        };
        String::from_utf8(decode_qt_value(value)?).ok()
    }

    fn decode_qt_value(value: &str) -> Option<Vec<u8>> {
        let mut input = value.bytes().peekable();
        let mut output = Vec::new();
        while let Some(byte) = input.next() {
            let byte = if byte == b'\\' {
                match input.next()? {
                    b'0' => 0,
                    b'\\' => b'\\',
                    b'"' => b'"',
                    b'x' => {
                        let mut number = (input.next()? as char).to_digit(16)? as u8;
                        while let Some(digit) = input.peek().and_then(|b| (*b as char).to_digit(16))
                        {
                            input.next();
                            number = number.checked_mul(16)?.checked_add(digit as u8)?;
                        }
                        number
                    }
                    _ => return None,
                }
            } else {
                byte
            };
            output.push(byte);
        }
        Some(output)
    }

    async fn read_password() -> Option<String> {
        type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

        let bus = Connection::session().await.ok()?;
        let service = Proxy::new(
            &bus,
            SECRET_SERVICE,
            "/org/freedesktop/secrets",
            "org.freedesktop.Secret.Service",
        )
        .await
        .ok()?;
        // Strawberry's libsecret backend uses these two fixed application attributes.
        let attributes = HashMap::from([("application", "strawberry"), ("service", "subsonic")]);
        let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
            service.call("SearchItems", &(attributes,)).await.ok()?;
        let [path] = unlocked.as_slice() else {
            return None;
        };
        if !locked.is_empty() {
            return None;
        }
        let (_, session): (OwnedValue, OwnedObjectPath) = service
            .call("OpenSession", &("plain", Value::from("")))
            .await
            .ok()?;
        let secrets: zbus::Result<HashMap<OwnedObjectPath, Secret>> =
            service.call("GetSecrets", &(vec![path], &session)).await;
        if let Ok(proxy) = Proxy::new(
            &bus,
            SECRET_SERVICE,
            session.as_str(),
            "org.freedesktop.Secret.Session",
        )
        .await
        {
            let _ = proxy.call::<_, _, ()>("Close", &()).await;
        }
        let mut secrets = secrets.ok()?;
        let (returned_session, parameters, bytes, _) = secrets.remove(path)?;
        if returned_session != session
            || !parameters.is_empty()
            || bytes.is_empty()
            || bytes.len() > 4096
            || bytes.contains(&0)
        {
            return None;
        }
        String::from_utf8(bytes).ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_qt_saved_url_and_username() {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("strawberry.conf");
            let config = r#"[Subsonic]
enabled=true
url=@Variant(\0\0\0\x11\0\0\0\x1dhttps://example.com:443/music)
username="élise,e = \"name\""
[Jellyfin]
url=https://other.example
username=other
"#;
            std::fs::write(&path, config).unwrap();
            assert_eq!(
                read_account(&path),
                Some((
                    "https://example.com/music".into(),
                    "élise,e = \"name\"".into(),
                ))
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), config);
        }

        #[test]
        fn rejects_ambiguous_invalid_or_disabled_accounts() {
            let account = "[Subsonic]\nurl=https://example.com/music\nusername=listener\n";
            assert!(parse_config(account).is_some());
            for fragment in [
                "enabled=false",
                "url=https://other.example",
                "username=other",
            ] {
                assert!(parse_config(&format!("{account}{fragment}")).is_none());
            }
            for url in [
                "https://listener:password@example.com/music",
                "https://example.com/music?token=example",
                r"@Variant(\0\0\0\x11\0\0\0\x1https://example.com)",
                r"@Variant(\0\0\0\x12\0\0\0\x13https://example.com)",
            ] {
                assert!(
                    parse_config(&format!("[Subsonic]\nurl={url}\nusername=listener")).is_none()
                );
            }
            assert!(
                parse_config("[Jellyfin]\nurl=https://example.com\nusername=listener").is_none()
            );
        }
    }
}
