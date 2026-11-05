//! Format-level tests: Netscape and JSON, platform independent.

use mirage_cookies::{
    parse_json, parse_netscape, serialize_json, serialize_netscape, Cookie, CookieError,
};

fn sample() -> Vec<Cookie> {
    vec![
        Cookie {
            name: "sid".to_owned(),
            value: "abc123".to_owned(),
            domain: ".example.com".to_owned(),
            path: "/".to_owned(),
            secure: true,
            http_only: true,
            expiry: Some(1_800_000_000),
        },
        Cookie {
            name: "theme".to_owned(),
            value: "dark".to_owned(),
            domain: "shop.example.com".to_owned(),
            path: "/app".to_owned(),
            secure: false,
            http_only: false,
            expiry: None,
        },
    ]
}

#[test]
fn netscape_roundtrip_preserves_every_field() {
    let cookies = sample();
    let text = serialize_netscape(&cookies);
    let parsed = parse_netscape(&text).expect("serialized output must parse");
    assert_eq!(parsed, cookies);
}

#[test]
fn netscape_writes_header_and_httponly_prefix() {
    let text = serialize_netscape(&sample());
    assert!(text.starts_with("# Netscape HTTP Cookie File\n"));
    assert!(text.contains("#HttpOnly_.example.com\tTRUE\t/\tTRUE\t1800000000\tsid\tabc123"));
    // Host-only cookie: no leading dot, FALSE flag, session expiry 0.
    assert!(text.contains("shop.example.com\tFALSE\t/app\tFALSE\t0\ttheme\tdark"));
}

#[test]
fn netscape_parses_comments_blanks_and_httponly() {
    let input = "\u{feff}# Netscape HTTP Cookie File\n\
                 \n\
                 # a plain comment\n\
                 #HttpOnly_.example.com\tTRUE\t/\tTRUE\t123\tname\tvalue\n\
                 example.com\tFALSE\t/\tFALSE\t0\tsession\tsv\r\n";

    let cookies = parse_netscape(input).expect("tolerant parse");
    assert_eq!(cookies.len(), 2);

    assert_eq!(cookies[0].name, "name");
    assert_eq!(cookies[0].value, "value");
    assert_eq!(cookies[0].domain, ".example.com");
    assert!(cookies[0].http_only);
    assert!(cookies[0].secure);
    assert_eq!(cookies[0].expiry, Some(123));

    assert_eq!(cookies[1].domain, "example.com");
    assert!(!cookies[1].http_only);
    assert_eq!(cookies[1].expiry, None, "expiry 0 is a session cookie");
}

#[test]
fn netscape_subdomains_flag_normalizes_into_a_leading_dot() {
    // TRUE without a dot gains one; special hosts are left alone.
    let input = "example.com\tTRUE\t/\tFALSE\t0\ta\t1\n\
                 127.0.0.1\tTRUE\t/\tFALSE\t0\tb\t2\n\
                 localhost\tTRUE\t/\tFALSE\t0\tc\t3\n";
    let cookies = parse_netscape(input).expect("parse");
    assert_eq!(cookies[0].domain, ".example.com");
    assert_eq!(cookies[1].domain, "127.0.0.1");
    assert_eq!(cookies[2].domain, "localhost");
}

#[test]
fn netscape_bad_line_reports_its_number() {
    let input = "# header\n\
                 .example.com\tTRUE\t/\tTRUE\t123\tname\tvalue\n\
                 broken\tline\n";
    let error = parse_netscape(input).expect_err("must fail");
    match error {
        CookieError::Parse { line, .. } => assert_eq!(line, 3),
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn netscape_bad_expiry_is_a_parse_error() {
    let input = ".example.com\tTRUE\t/\tTRUE\tnot-a-number\tname\tvalue\n";
    let error = parse_netscape(input).expect_err("must fail");
    assert!(matches!(error, CookieError::Parse { line: 1, .. }));
}

#[test]
fn netscape_bad_bool_is_a_parse_error() {
    let input = ".example.com\tMAYBE\t/\tFALSE\t0\tname\tvalue\n";
    let error = parse_netscape(input).expect_err("must fail");
    assert!(matches!(error, CookieError::Parse { line: 1, .. }));
}

#[test]
fn empty_netscape_input_is_empty() {
    assert!(parse_netscape("").expect("empty parses").is_empty());
}

#[test]
fn json_roundtrip_includes_null_expiry() {
    let cookies = sample();
    let json = serialize_json(&cookies).expect("serialize");
    let parsed = parse_json(&json).expect("parse");
    assert_eq!(parsed, cookies);
}

#[test]
fn json_uses_camel_case_and_defaults_missing_fields() {
    let parsed = parse_json(
        r#"[{"name":"a","value":"b","domain":".x.com","path":"/","secure":true,"httpOnly":true,"expiry":null}]"#,
    )
    .expect("parse");
    assert_eq!(
        parsed,
        vec![Cookie {
            name: "a".to_owned(),
            value: "b".to_owned(),
            domain: ".x.com".to_owned(),
            path: "/".to_owned(),
            secure: true,
            http_only: true,
            expiry: None,
        }]
    );

    // Missing optional fields fall back to defaults.
    let parsed = parse_json(r#"[{"name":"n","domain":"d"}]"#).expect("parse");
    assert_eq!(parsed[0].expiry, None);
    assert!(!parsed[0].secure);
    assert_eq!(parsed[0].path, "");
}
