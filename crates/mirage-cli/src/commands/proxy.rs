//! `mirage proxy check`: resolve the exit IP and latency through a proxy.

use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use mirage_proxy::check::check_exit_ip;

use crate::cli::ProxyCheckArgs;
use crate::error::CliError;
use crate::output::{to_json_value, Render};
use crate::proxy_url::parse_proxy_url;

/// Result of a proxy exit-IP check. Always returned (even on failure) so the
/// `--json` document and the human message can report the error; the process
/// exit code still reflects success.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyCheckReport {
    /// Proxy scheme (`http` / `socks5`).
    pub scheme: String,
    /// Proxy host.
    pub host: String,
    /// Proxy port.
    pub port: u16,
    /// Whether the check succeeded.
    pub ok: bool,
    /// The observed exit IP, on success.
    pub ip: Option<String>,
    /// Round-trip latency of the whole check, in milliseconds.
    pub latency_ms: Option<u64>,
    /// Failure reason, on failure.
    pub error: Option<String>,
}

/// Dial the proxy, fetch the exit IP and measure latency.
pub async fn run_proxy_check(args: &ProxyCheckArgs) -> Result<ProxyCheckReport, CliError> {
    let parsed = parse_proxy_url(&args.url)?;
    let config = parsed.to_client();

    let start = Instant::now();
    let result = check_exit_ip(&config, Duration::from_millis(args.timeout_ms)).await;
    let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

    let (ok, ip, error) = match result {
        Ok(ip) => (true, Some(ip.to_string()), None),
        Err(error) => (false, None, Some(error.to_string())),
    };

    Ok(ProxyCheckReport {
        scheme: parsed.scheme.as_str().to_string(),
        host: parsed.host.clone(),
        port: parsed.port,
        ok,
        ip,
        latency_ms: Some(latency_ms),
        error,
    })
}

impl Render for ProxyCheckReport {
    fn human(&self) -> String {
        if self.ok {
            format!(
                "{}: exit ip {} ({} ms)",
                self.endpoint(),
                self.ip.as_deref().unwrap_or("?"),
                self.latency_ms.unwrap_or(0)
            )
        } else {
            format!(
                "{}: FAILED ({} ms): {}",
                self.endpoint(),
                self.latency_ms.unwrap_or(0),
                self.error.as_deref().unwrap_or("unknown error")
            )
        }
    }

    fn json(&self) -> Value {
        to_json_value(self)
    }
}

impl ProxyCheckReport {
    /// `scheme://host:port` of the checked proxy.
    fn endpoint(&self) -> String {
        format!("{}://{}:{}", self.scheme, self.host, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_shows_success_and_failure() {
        let ok = ProxyCheckReport {
            scheme: "socks5".into(),
            host: "1.2.3.4".into(),
            port: 1080,
            ok: true,
            ip: Some("9.9.9.9".into()),
            latency_ms: Some(120),
            error: None,
        };
        assert!(ok.human().contains("9.9.9.9"));
        assert!(ok.human().contains("socks5://1.2.3.4:1080"));

        let failed = ProxyCheckReport {
            ok: false,
            ip: None,
            error: Some("proxy operation timed out".into()),
            ..ok.clone()
        };
        assert!(failed.human().contains("FAILED"));
        assert!(failed.human().contains("timed out"));
    }
}
