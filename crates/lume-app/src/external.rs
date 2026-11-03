//! Apertura di URL tramite il launcher nativo del sistema operativo.

use std::process::{Command, Stdio};

const MAX_URL_BYTES: usize = 2048;

fn valid_port(value: &str) -> bool {
    !value.is_empty() && value.parse::<u16>().is_ok()
}

fn validate_web_url(url: &str) -> bool {
    if url.len() > MAX_URL_BYTES
        || url
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || url.contains('\\')
    {
        return false;
    }
    let Some((scheme, remainder)) = url.split_once("://") else {
        return false;
    };
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return false;
    }
    let authority = remainder.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return false;
    }

    if let Some(ipv6) = authority.strip_prefix('[') {
        let Some((host, suffix)) = ipv6.split_once(']') else {
            return false;
        };
        return host.parse::<std::net::Ipv6Addr>().is_ok()
            && (suffix.is_empty() || suffix.strip_prefix(':').is_some_and(valid_port));
    }

    let (host, port_valid) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, valid_port(port)),
        None => (authority, true),
    };
    port_valid
        && host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn spawn_reaper(child: std::process::Child) -> Result<std::thread::JoinHandle<()>, String> {
    let child = std::sync::Arc::new(std::sync::Mutex::new(Some(child)));
    let worker_child = child.clone();
    match std::thread::Builder::new()
        .name("lume-url-reaper".into())
        .spawn(move || {
            if let Ok(mut slot) = worker_child.lock()
                && let Some(mut child) = slot.take()
            {
                let _ = child.wait();
            }
        }) {
        Ok(handle) => Ok(handle),
        Err(error) => {
            if let Ok(mut slot) = child.lock()
                && let Some(mut child) = slot.take()
            {
                let _ = child.kill();
                let _ = child.wait();
            }
            Err(format!("impossibile avviare il reaper URL: {error}"))
        }
    }
}

pub fn open(url: &str) -> Result<(), String> {
    if !validate_web_url(url) {
        return Err("Lume apre solo hyperlink HTTP/HTTPS dal terminale".into());
    }

    let mut command = if cfg!(target_os = "windows") {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler").arg(url);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("impossibile aprire {url}: {error}"))?;
    spawn_reaper(child).map(|_| ())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::{open, spawn_reaper, validate_web_url};

    #[test]
    fn rejects_non_web_schemes_before_spawning() {
        let error = open("file:///etc/passwd").unwrap_err();
        assert!(error.contains("HTTP/HTTPS"));
    }

    #[test]
    fn validates_host_port_and_bounds_without_spawning() {
        assert!(validate_web_url("https://example.com:8443/path?q=1"));
        assert!(validate_web_url("http://[::1]:8080/"));
        assert!(!validate_web_url("https://"));
        assert!(!validate_web_url("https://example.com bad"));
        assert!(!validate_web_url("https://user@example.com"));
        assert!(!validate_web_url(&format!(
            "https://example.com/{}",
            "x".repeat(2048)
        )));
    }

    #[test]
    fn reaper_waits_for_a_real_child_process() {
        let mut command = if cfg!(target_os = "windows") {
            let mut command = std::process::Command::new("cmd.exe");
            command.args(["/D", "/S", "/C", "exit 0"]);
            command
        } else {
            let mut command = std::process::Command::new("sh");
            command.args(["-c", "true"]);
            command
        };
        let child = command.spawn().unwrap();
        spawn_reaper(child).unwrap().join().unwrap();
    }
}
