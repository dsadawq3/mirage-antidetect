//! Netscape `cookies.txt` reader and writer.
//!
//! One data line is seven TAB-separated fields:
//! `domain, include_subdomains, path, secure, expiry, name, value`.
//! Lines starting with `#` are comments, except `#HttpOnly_` which marks an
//! HttpOnly cookie and whose domain follows the prefix. An expiry of `0`
//! means a session cookie. The reader is tolerant of the header, comments,
//! blank lines and `#HttpOnly_`, and reports malformed data lines with their
//! line number instead of panicking.

use crate::error::CookieError;
use crate::model::Cookie;

/// Conventional first line of a Netscape cookies file.
pub const HEADER: &str = "# Netscape HTTP Cookie File";

/// Prefix marking an HttpOnly cookie.
const HTTPONLY_PREFIX: &str = "#HttpOnly_";

/// Number of TAB-separated fields in a data line.
const FIELD_COUNT: usize = 7;

/// Parse the contents of a Netscape `cookies.txt` file.
///
/// Comments, the header and blank lines are ignored. A data line that does
/// not have exactly seven TAB-separated fields, or whose boolean/expiry
/// fields are not well formed, yields [`CookieError::Parse`] with the
/// 1-based line number. Empty input yields an empty vector.
pub fn parse_netscape(input: &str) -> Result<Vec<Cookie>, CookieError> {
    let mut cookies = Vec::new();

    // A UTF-8 BOM would otherwise become part of the first domain field.
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);

    for (index, raw_line) in input.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.trim_end_matches('\r');

        if line.is_empty() {
            continue;
        }

        // `#HttpOnly_` is a marker, not a comment; strip it and parse the
        // rest as a normal data line.
        let (http_only, body) = match line.strip_prefix(HTTPONLY_PREFIX) {
            Some(rest) => (true, rest),
            None => {
                if line.starts_with('#') {
                    continue;
                }
                (false, line)
            }
        };

        let fields: Vec<&str> = body.split('\t').collect();
        if fields.len() != FIELD_COUNT {
            return Err(CookieError::Parse {
                line: line_number,
                reason: format!(
                    "expected {FIELD_COUNT} TAB-separated fields, found {}",
                    fields.len()
                ),
            });
        }

        let include_subdomains = parse_bool(fields[1], line_number, "include_subdomains")?;
        let secure = parse_bool(fields[3], line_number, "secure")?;
        let expiry = parse_expiry(fields[4], line_number)?;

        cookies.push(Cookie {
            name: fields[5].to_owned(),
            value: fields[6].to_owned(),
            domain: normalize_domain(fields[0], include_subdomains),
            path: fields[2].to_owned(),
            secure,
            http_only,
            expiry,
        });
    }

    Ok(cookies)
}

/// Render cookies as a Netscape `cookies.txt` document.
///
/// The header is always emitted. `include_subdomains` is derived from the
/// leading dot of the domain (Chromium convention), `TRUE`/`FALSE` are
/// upper-case, and a session cookie writes expiry `0`.
pub fn serialize_netscape(cookies: &[Cookie]) -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push('\n');

    for cookie in cookies {
        let prefix = if cookie.http_only {
            HTTPONLY_PREFIX
        } else {
            ""
        };
        let include_subdomains = if cookie.domain.starts_with('.') {
            "TRUE"
        } else {
            "FALSE"
        };
        let secure = if cookie.secure { "TRUE" } else { "FALSE" };
        let expiry = cookie.expiry.unwrap_or(0);

        out.push_str(prefix);
        out.push_str(&cookie.domain);
        out.push('\t');
        out.push_str(include_subdomains);
        out.push('\t');
        out.push_str(&cookie.path);
        out.push('\t');
        out.push_str(secure);
        out.push('\t');
        out.push_str(&expiry.to_string());
        out.push('\t');
        out.push_str(&cookie.name);
        out.push('\t');
        out.push_str(&cookie.value);
        out.push('\n');
    }

    out
}

/// Parse a `TRUE`/`FALSE` field (case-insensitive).
fn parse_bool(value: &str, line: usize, field: &str) -> Result<bool, CookieError> {
    match value.to_ascii_uppercase().as_str() {
        "TRUE" => Ok(true),
        "FALSE" => Ok(false),
        other => Err(CookieError::Parse {
            line,
            reason: format!("{field} must be TRUE or FALSE, found `{other}`"),
        }),
    }
}

/// Parse the expiry field: `0` means a session cookie (`None`).
fn parse_expiry(value: &str, line: usize) -> Result<Option<i64>, CookieError> {
    let seconds: i64 = value.trim().parse().map_err(|_| CookieError::Parse {
        line,
        reason: format!("expiry must be an integer number of seconds, found `{value}`"),
    })?;

    if seconds == 0 {
        Ok(None)
    } else {
        Ok(Some(seconds))
    }
}

/// Turn the `include_subdomains` flag into Chromium's leading-dot convention.
///
/// A domain cookie is identified inside Chromium by a leading dot on the
/// `host_key`. When a file uses `TRUE` without the dot, add it — except for
/// hosts where a dot is meaningless (IP literals, `localhost`).
fn normalize_domain(domain: &str, include_subdomains: bool) -> String {
    if include_subdomains && !domain.starts_with('.') && !is_special_host(domain) {
        let mut dotted = String::with_capacity(domain.len() + 1);
        dotted.push('.');
        dotted.push_str(domain);
        dotted
    } else {
        domain.to_owned()
    }
}

/// Hosts that never take a leading dot (`localhost`, IP literals).
fn is_special_host(domain: &str) -> bool {
    let bare = domain.trim_start_matches('.');
    bare.eq_ignore_ascii_case("localhost") || bare.parse::<std::net::IpAddr>().is_ok()
}
