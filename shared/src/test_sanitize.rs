#![cfg(test)]

extern crate std;

use soroban_sdk::{Bytes, Env};
use crate::errors::Error;
use crate::sanitize::{
    sanitize_text, sanitize_url, trim_whitespace, validate_external_url, validate_safe_text,
    SafeScheme, MAX_URL_LEN,
};

#[test]
fn test_valid_urls() {
    let env = Env::default();

    // HTTPS
    let https_url = Bytes::from_slice(&env, b"https://api.trellis.org/metadata/v1/contract.json");
    assert_eq!(
        validate_external_url(&https_url, MAX_URL_LEN, false),
        Ok(SafeScheme::Https)
    );

    // IPFS
    let ipfs_url = Bytes::from_slice(&env, b"ipfs://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi");
    assert_eq!(
        validate_external_url(&ipfs_url, MAX_URL_LEN, false),
        Ok(SafeScheme::Ipfs)
    );

    // IPNS
    let ipns_url = Bytes::from_slice(&env, b"ipns://k51qzi5uqu5dl11xvi25278m0q36b75eakpwhc1809072r2pndv3q538wz1p3w");
    assert_eq!(
        validate_external_url(&ipns_url, MAX_URL_LEN, false),
        Ok(SafeScheme::Ipns)
    );

    // Arweave
    let ar_url = Bytes::from_slice(&env, b"ar://aB1c2D3e4F5g6H7i8J9k0L1m2N3o4P5q6R7s8T9u0V1");
    assert_eq!(
        validate_external_url(&ar_url, MAX_URL_LEN, false),
        Ok(SafeScheme::Arweave)
    );

    // DID
    let did_url = Bytes::from_slice(&env, b"did:pkh:stellar:GABC1234567890");
    assert_eq!(
        validate_external_url(&did_url, MAX_URL_LEN, false),
        Ok(SafeScheme::Did)
    );
}

#[test]
fn test_http_policy() {
    let env = Env::default();
    let http_url = Bytes::from_slice(&env, b"http://localhost:8000/api");

    // Rejected when allow_http is false
    assert_eq!(
        validate_external_url(&http_url, MAX_URL_LEN, false),
        Err(Error::InvalidArgument)
    );

    // Accepted when allow_http is true
    assert_eq!(
        validate_external_url(&http_url, MAX_URL_LEN, true),
        Ok(SafeScheme::Http)
    );
}

#[test]
fn test_rejects_dangerous_schemes() {
    let env = Env::default();

    let dangerous = [
        b"javascript:alert(1)".as_slice(),
        b"JAVASCRIPT:alert(1)".as_slice(),
        b"JaVaScRiPt:alert(document.cookie)".as_slice(),
        b"data:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==".as_slice(),
        b"DATA:application/json,{}".as_slice(),
        b"file:///etc/passwd".as_slice(),
        b"FILE://localhost/etc/shadow".as_slice(),
        b"vbscript:MsgBox(\"XSS\")".as_slice(),
        b"blob:https://example.com/uuid".as_slice(),
        b"about:blank".as_slice(),
        b"https://evil.com?redirect=javascript:alert(1)".as_slice(),
    ];

    for &bad in &dangerous {
        let b = Bytes::from_slice(&env, bad);
        assert_eq!(
            validate_external_url(&b, MAX_URL_LEN, true),
            Err(Error::InvalidArgument),
            "Failed to reject dangerous URL: {:?}",
            std::str::from_utf8(bad).unwrap_or("non-utf8")
        );
    }
}

#[test]
fn test_rejects_crlf_and_null_bytes() {
    let env = Env::default();

    let payloads = [
        b"https://example.com\r\nSet-Cookie:admin=true".as_slice(),
        b"https://example.com\nmalicious".as_slice(),
        b"https://example.com\0hidden".as_slice(),
        b"https://example.com\x01evil".as_slice(),
        b"https://example .com".as_slice(),
    ];

    for &p in &payloads {
        let b = Bytes::from_slice(&env, p);
        assert_eq!(
            validate_external_url(&b, MAX_URL_LEN, true),
            Err(Error::InvalidArgument)
        );
    }
}

#[test]
fn test_missing_host() {
    let env = Env::default();

    let invalid_hosts = [
        b"https://".as_slice(),
        b"https:///path".as_slice(),
        b"https://?query=1".as_slice(),
        b"https://#fragment".as_slice(),
        b"http://".as_slice(),
        b"http:///path".as_slice(),
    ];

    for &inv in &invalid_hosts {
        let b = Bytes::from_slice(&env, inv);
        assert_eq!(
            validate_external_url(&b, MAX_URL_LEN, true),
            Err(Error::InvalidArgument)
        );
    }
}

#[test]
fn test_trim_and_sanitize_url() {
    let env = Env::default();
    let raw = Bytes::from_slice(&env, b"  https://trellis.org/metadata.json  \n");
    let sanitized = sanitize_url(&env, &raw, MAX_URL_LEN, false).unwrap();
    let expected = Bytes::from_slice(&env, b"https://trellis.org/metadata.json");
    assert_eq!(sanitized, expected);
}

#[test]
fn test_validate_and_sanitize_text() {
    let env = Env::default();

    // Valid text
    let good_text = Bytes::from_slice(&env, b"Humanitarian relief fund for region A.");
    assert_eq!(validate_safe_text(&good_text, 100, false), Ok(()));

    // Multiline
    let multiline = Bytes::from_slice(&env, b"Line 1\nLine 2\nLine 3");
    assert_eq!(validate_safe_text(&multiline, 100, true), Ok(()));
    assert_eq!(validate_safe_text(&multiline, 100, false), Err(Error::InvalidArgument));

    // Script injection in text
    let xss_texts = [
        b"Hello <script>alert(1)</script> world".as_slice(),
        b"Check this <iframe src=\"evil.com\"></iframe>".as_slice(),
        b"<svg onload=alert(document.cookie)>".as_slice(),
        b"<img src=x onerror=alert(1)>".as_slice(),
        b"<a href=\"javascript:alert(1)\">Click here</a>".as_slice(),
        b"Null byte \0 in text".as_slice(),
    ];

    for &xss in &xss_texts {
        let b = Bytes::from_slice(&env, xss);
        assert_eq!(
            validate_safe_text(&b, 1000, true),
            Err(Error::InvalidArgument)
        );
    }

    // Sanitize text trims whitespace
    let padded = Bytes::from_slice(&env, b"  Legitimate Description  ");
    let clean = sanitize_text(&env, &padded, 100, false).unwrap();
    assert_eq!(clean, Bytes::from_slice(&env, b"Legitimate Description"));
}
