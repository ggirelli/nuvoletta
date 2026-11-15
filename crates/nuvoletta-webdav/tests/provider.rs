//! The provider against the in-process server: listing, reading, creating, error mapping,
//! throttling, timeouts, redirects, interrupted uploads, TLS trust.

mod common;

use common::*;
use nuvoletta_sync::{Provider, ProviderError};
use nuvoletta_webdav::{
    DavError, Options, RetryPolicy, TlsFailure, Verdict, WebDavConfig, WebDavProvider,
    fetch_fingerprint,
};
use std::time::Duration;

const HDR: &str = "h/00000001.nvhdr";

fn seg(dev: &str, seq: u64) -> String {
    format!("d/{dev}/l/{seq:016x}.nvlog")
}

fn obj(id: &str) -> String {
    format!("o/{}/{id}.nvobj", &id[..2])
}

const DEV: &str = "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a";
const OID: &str = "ab00000000000000000000000000000f";

#[test]
fn create_list_get_and_taken_names() {
    let s = TestServer::start(Policy::create_only());
    let p = provider(&s);
    p.put_new(HDR, b"header").unwrap();
    p.put_new(&seg(DEV, 1), b"segment one").unwrap();
    p.put_new(&obj(OID), b"object").unwrap();
    assert!(matches!(
        p.put_new(HDR, b"other"),
        Err(ProviderError::AlreadyExists(_))
    ));
    assert_eq!(
        p.get(HDR).unwrap(),
        b"header",
        "the taken name was not modified"
    );
    assert!(matches!(
        p.get("h/00000002.nvhdr"),
        Err(ProviderError::NotFound(_))
    ));

    let all = p.list("").unwrap();
    let names: Vec<&str> = all.iter().map(|e| e.name.as_str()).collect();
    let o = obj(OID);
    let l = seg(DEV, 1);
    assert!(
        names.contains(&HDR) && names.contains(&l.as_str()) && names.contains(&o.as_str()),
        "{names:?}"
    );
    let e = all.iter().find(|e| e.name == l).unwrap();
    assert_eq!(e.size, 11);
    assert!(e.etag.is_some() && e.mtime_ms.is_some());
    let caps = p.capabilities();
    assert!(
        caps.create_only && !caps.can_delete && !caps.atomic_rename && caps.etags && caps.mtimes
    );
    assert!(caps.authoritative);

    // Subtree and missing-directory listings (the check's stable probe files are listed too:
    // they are stray, never protocol files).
    let d: Vec<_> = p
        .list("d")
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .filter(|n| !n.contains("probe"))
        .collect();
    assert_eq!(d, vec![seg(DEV, 1)]);
    assert!(p.list("s").unwrap().is_empty());
}

#[test]
fn a_cycle_listing_skips_the_object_shards() {
    let s = TestServer::start(Policy::create_only());
    let p = provider(&s);
    p.put_new(HDR, b"h").unwrap();
    p.put_new(&seg(DEV, 1), b"x").unwrap();
    p.put_new(&obj(OID), b"o").unwrap();
    s.clear_log();
    let names: Vec<String> = p
        .list_cycle()
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(
        names.contains(&HDR.to_owned()) && !names.iter().any(|n| n.starts_with("o/")),
        "{names:?}"
    );
    // The probe files of the check are not protocol files but are listed (and ignored later).
    let propfinds = s.methods().iter().filter(|m| *m == "PROPFIND").count();
    assert!(propfinds <= 6, "{propfinds} requests");
    assert!(p.list("o").unwrap().iter().any(|e| e.name == obj(OID)));
}

#[test]
fn normal_operation_never_deletes_moves_or_overwrites() {
    let s = TestServer::start(Policy::create_only());
    let p = provider(&s);
    p.list("").unwrap(); // runs the check
    s.clear_log();
    p.put_new(HDR, b"h").unwrap();
    let _ = p.put_new(HDR, b"again");
    p.put_new(&seg(DEV, 1), b"x").unwrap();
    p.list("").unwrap();
    p.get(HDR).unwrap();
    assert!(matches!(p.delete(HDR), Err(ProviderError::Forbidden(_))));
    for m in s.methods() {
        assert!(
            matches!(m.as_str(), "PROPFIND" | "GET" | "PUT" | "MKCOL"),
            "{m}"
        );
    }
}

#[test]
fn only_protocol_names_may_be_created_and_hostile_names_are_refused() {
    let s = TestServer::start(Policy::create_only());
    let p = provider(&s);
    for bad in [
        "../x",
        "a/../b",
        "a//b",
        "a\\b",
        "/etc/passwd",
        "h/readme.txt",
        ".nuvoletta-probe",
        "x\0y",
    ] {
        assert!(p.put_new(bad, b"x").is_err(), "{bad}");
    }
    for bad in ["../x", "a/../b", "a//b", "a\\b", "/etc/passwd", "x\0y"] {
        assert!(p.get(bad).is_err(), "{bad}");
    }
    assert!(p.list("../x").is_err());
}

#[test]
fn a_taken_name_that_answers_403_is_still_already_exists() {
    // A server that checks permissions before preconditions.
    let s = TestServer::start(Policy {
        honour_if_none_match: false,
        ..Policy::create_only()
    });
    let p = provider(&s);
    p.put_new(HDR, b"first").unwrap();
    assert!(matches!(
        p.put_new(HDR, b"second"),
        Err(ProviderError::AlreadyExists(_))
    ));
    assert_eq!(p.get(HDR).unwrap(), b"first");
}

#[test]
fn hrefs_outside_the_root_are_ignored() {
    let s = TestServer::start(Policy::create_only());
    let p = provider(&s);
    p.put_new(HDR, b"h").unwrap();
    s.with(|st| {
        st.faults.extra_hrefs = vec![
            "/other/h/00000009.nvhdr".to_owned(),
            "/dav/../etc/passwd".to_owned(),
            "/dav/h/%2e%2e/x".to_owned(),
            "/dav/h/a%2Fb".to_owned(),
            "https://evil.example/dav/h/00000008.nvhdr".to_owned(),
            "http://127.0.0.1/dav/h/00000007.nvhdr".to_owned(),
            "/dav/h/00000006.nvhdr".to_owned(), // a legitimate extra file: listed
        ];
    });
    let names: Vec<String> = p
        .list_cycle()
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&HDR.to_owned()));
    assert!(
        !names.iter().any(|n| n.contains("other")
            || n.contains("passwd")
            || n.contains("..")
            || n.contains("evil")
            || n.contains("00000008")
            || n.contains("00000007")
            || n.contains("%2F")
            || n.contains("a/b")),
        "{names:?}"
    );
}

#[test]
fn throttling_is_retried_then_reported_as_transient() {
    let s = TestServer::start(Policy::create_only());
    let p = provider(&s);
    p.put_new(HDR, b"h").unwrap();
    // Two 503s with Retry-After: 0, then success.
    s.with(|st| st.faults.throttle = Some((2, Some(0))));
    assert_eq!(p.get(HDR).unwrap(), b"h");
    // Persistent throttling gives up with a transient error (the cycle is retried later).
    s.with(|st| st.faults.throttle = Some((100, None)));
    match p.get(HDR) {
        Err(e @ ProviderError::Transient(_)) => assert!(e.is_retryable()),
        other => panic!("{other:?}"),
    }
    // A Retry-After longer than we are willing to wait ends the retries at once.
    s.with(|st| st.faults.throttle = Some((100, Some(3600))));
    let start = std::time::Instant::now();
    assert!(matches!(p.get(HDR), Err(ProviderError::Transient(_))));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn slow_servers_time_out_as_transient_errors() {
    let s = TestServer::start(Policy::create_only());
    let opts = Options {
        response_timeout: Duration::from_millis(300),
        total_timeout: Duration::from_secs(2),
        retry: RetryPolicy {
            attempts: 1,
            ..fast_opts().retry
        },
        ..fast_opts()
    };
    let p = WebDavProvider::new(cfg(&s), opts);
    p.put_new(HDR, b"h").unwrap();
    s.with(|st| st.faults.delay = vec![("00000001".to_owned(), Duration::from_millis(1500))]);
    match p.get(HDR) {
        Err(ProviderError::Transient(m)) => {
            assert!(m.contains("in time") || m.contains("took too long"), "{m}")
        }
        other => panic!("{other:?}"),
    }
    // The server is fine again afterwards (the broken connection was dropped).
    s.with(|st| st.faults.delay.clear());
    assert_eq!(p.get(HDR).unwrap(), b"h");
}

#[test]
fn redirects_stay_on_the_origin() {
    let s = TestServer::start(Policy::create_only());
    let p = provider(&s);
    p.put_new("h/00000002.nvhdr", b"moved here").unwrap();
    s.with(|st| {
        st.faults.redirect = vec![("00000001".to_owned(), "/dav/h/00000002.nvhdr".to_owned())]
    });
    assert_eq!(
        p.get(HDR).unwrap(),
        b"moved here",
        "a same-origin redirect is followed"
    );
    // Off-origin: refused, and nothing is sent there (we do not even resolve it).
    s.with(|st| {
        st.faults.redirect = vec![(
            "00000001".to_owned(),
            "https://evil.example/steal".to_owned(),
        )]
    });
    match p.get(HDR) {
        Err(ProviderError::Other(m)) => assert!(m.contains("redirect"), "{m}"),
        other => panic!("{other:?}"),
    }
    s.with(|st| {
        st.faults.redirect = vec![("00000001".to_owned(), "http://127.0.0.1/steal".to_owned())]
    });
    assert!(p.get(HDR).is_err());
    // A redirect loop ends.
    s.with(|st| st.faults.redirect = vec![("h/".to_owned(), "/dav/h/00000001.nvhdr".to_owned())]);
    assert!(p.get(HDR).is_err());
}

#[test]
fn an_interrupted_upload_leaves_a_half_file_that_is_never_overwritten() {
    let s = TestServer::start(Policy::create_only());
    let p = provider(&s);
    p.list("").unwrap();
    s.with(|st| st.faults.partial_put = vec![("00000001.nvhdr".to_owned(), 1)]);
    // The dropped connection is retried once on a fresh connection (the connection was kept
    // alive from the check), which finds the half file: either answer is correct and safe.
    let r = p.put_new(HDR, b"0123456789");
    assert!(
        matches!(
            r,
            Err(ProviderError::Transient(_) | ProviderError::AlreadyExists(_))
        ),
        "{r:?}"
    );
    // The server kept half of it; the name is now taken and the half stays as it is.
    assert!(matches!(
        p.put_new(HDR, b"0123456789"),
        Err(ProviderError::AlreadyExists(_))
    ));
    assert_eq!(p.get(HDR).unwrap(), b"01234");
}

#[test]
fn missing_etags_and_mtimes_are_reported_in_the_capabilities() {
    let s = TestServer::start(Policy {
        send_etags: false,
        send_mtimes: false,
        ..Policy::create_only()
    });
    let p = provider(&s);
    p.put_new(HDR, b"h").unwrap();
    let l: Vec<_> = p
        .list("h")
        .unwrap()
        .into_iter()
        .filter(|e| !e.name.contains("probe"))
        .collect();
    assert_eq!(l.len(), 1);
    assert!(l[0].etag.is_none() && l[0].mtime_ms.is_none());
    let c = p.capabilities();
    assert!(!c.etags && !c.mtimes);
}

#[test]
fn plain_http_is_refused() {
    assert!(WebDavConfig::new("http://127.0.0.1:1/dav/", USER, PASS, None).is_err());
}

#[test]
fn wrong_password_is_an_authentication_error() {
    let s = TestServer::start(Policy::create_only());
    let c = WebDavConfig::new(&s.url(), USER, "wrong", Some(s.fingerprint)).unwrap();
    let p = WebDavProvider::new(c, fast_opts());
    assert_eq!(p.verify().unwrap_err(), DavError::Auth);
    assert!(matches!(p.list(""), Err(ProviderError::Auth(_))));
}

#[test]
fn an_untrusted_certificate_is_reported_and_a_pin_fixes_it() {
    let s = TestServer::start(Policy::create_only());
    // No pin: the self-signed certificate does not chain to a web root.
    let c = WebDavConfig::new(&s.url(), USER, PASS, None).unwrap();
    let p = WebDavProvider::new(c, fast_opts());
    match p.verify() {
        Err(DavError::Tls(TlsFailure::Untrusted(why))) => assert!(!why.is_empty()),
        other => panic!("{other:?}"),
    }
    // Fetching the fingerprint sends nothing but the handshake.
    s.clear_log();
    let fp = fetch_fingerprint("127.0.0.1", s.port, Duration::from_secs(5)).unwrap();
    assert_eq!(fp, s.fingerprint);
    assert!(s.methods().is_empty(), "no request was sent");
    // Pinned: works. A wrong pin is a mismatch, never a silent fallback.
    let c = WebDavConfig::new(&s.url(), USER, PASS, Some(fp)).unwrap();
    assert!(matches!(
        WebDavProvider::new(c, fast_opts()).verify().unwrap(),
        Verdict::Safe { .. }
    ));
    let c = WebDavConfig::new(&s.url(), USER, PASS, Some([7; 32])).unwrap();
    assert_eq!(
        WebDavProvider::new(c, fast_opts()).verify().unwrap_err(),
        DavError::Tls(TlsFailure::PinMismatch)
    );
}

#[test]
fn an_unreachable_server_is_a_transient_network_error() {
    // Bind and drop a listener to get a closed port.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let c = WebDavConfig::new(
        &format!("https://127.0.0.1:{port}/dav/"),
        USER,
        PASS,
        Some([1; 32]),
    )
    .unwrap();
    let p = WebDavProvider::new(c, fast_opts());
    assert!(matches!(p.list(""), Err(ProviderError::Transient(_))));
}
