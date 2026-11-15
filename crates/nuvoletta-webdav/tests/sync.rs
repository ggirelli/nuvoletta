//! The sync engine over WebDAV: two vault instances converge through the server, the remote
//! holds only ciphertext under opaque names, nothing is ever deleted or overwritten, and a
//! half-written upload does not block a device forever.

mod common;

use common::*;
use nuvoletta_sync::names::{Class, classify};
use nuvoletta_sync::{GcOptions, GcSkip, Warning};

fn protocol_files(s: &TestServer) -> Vec<(String, Vec<u8>)> {
    s.with(|st| {
        st.files
            .iter()
            .filter(|(n, _)| {
                !n.rsplit('/')
                    .next()
                    .unwrap()
                    .starts_with(".nuvoletta-probe")
            })
            .map(|(n, f)| (n.clone(), f.data.clone()))
            .collect()
    })
}

#[test]
fn two_devices_converge_through_webdav() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Device::new(&tmp, "a", &s, None);
    let mut b = Device::new(&tmp, "b", &s, Some(&a));

    let n = a.create("Plan", "plan", "the secret plan");
    a.sync();
    b.sync();
    assert_eq!(b.body(&n).as_deref(), Some("the secret plan"));

    // An edit travels back, and a second note the other way.
    b.set_body(&n, "the secret plan, revised");
    let m = b.create("Todo", "todo", "buy milk");
    b.sync();
    a.sync();
    assert_eq!(a.body(&n).as_deref(), Some("the secret plan, revised"));
    assert_eq!(a.body(&m).as_deref(), Some("buy milk"));

    // A repeated cycle with nothing to do changes nothing.
    let before = protocol_files(&s);
    let r = a.sync();
    assert_eq!(
        (r.objects_pushed, r.segments_uploaded, r.entries_applied),
        (0, 0, 0)
    );
    assert_eq!(protocol_files(&s), before);
}

#[test]
fn the_remote_holds_ciphertext_under_opaque_names_only() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Device::new(&tmp, "a", &s, None);
    a.create(
        "Very Private Title",
        "diary/2026",
        "needle-in-the-haystack body text",
    );
    a.sync();
    let files = protocol_files(&s);
    assert!(
        files.len() >= 3,
        "{:?}",
        files.iter().map(|f| &f.0).collect::<Vec<_>>()
    );
    for (name, data) in &files {
        assert!(matches!(classify(name), Class::Protocol(_)), "{name}");
        for secret in ["needle", "Private", "diary", "haystack"] {
            assert!(
                !name.to_lowercase().contains(&secret.to_lowercase()),
                "{name}"
            );
            assert!(
                !data
                    .windows(secret.len())
                    .any(|w| w.eq_ignore_ascii_case(secret.as_bytes())),
                "{secret} leaked into {name}"
            );
        }
    }
    // Everything else on the server is a stray probe file.
    for n in s.file_names() {
        let f = n.rsplit('/').next().unwrap().to_owned();
        assert!(
            f.starts_with(".nuvoletta-probe") || matches!(classify(&n), Class::Protocol(_)),
            "{n}"
        );
    }
    // The credential (user name and password) never appears in file contents either.
    for (_, data) in &files {
        assert!(!data.windows(PASS.len()).any(|w| w == PASS.as_bytes()));
    }
}

#[test]
fn no_delete_or_overwrite_is_ever_sent_outside_the_credential_check() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Device::new(&tmp, "a", &s, None);
    let mut b = Device::new(&tmp, "b", &s, Some(&a));
    let n = a.create("x", "x", "one");
    a.sync();
    b.sync();
    b.set_body(&n, "two");
    b.sync();
    a.sync();
    s.with(|st| {
        for (m, path) in &st.log {
            let probe = path.contains(".nuvoletta-probe");
            match m.as_str() {
                "PROPFIND" | "GET" | "MKCOL" => {}
                "PUT" => {}
                other => assert!(probe, "{other} {path}"),
            }
        }
    });
}

#[test]
fn garbage_collection_is_unavailable_on_webdav() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Device::new(&tmp, "a", &s, None);
    let n = a.create("x", "x", "one");
    for i in 0..8 {
        a.set_body(&n, &format!("rev {i}"));
        a.sync();
    }
    let opts = GcOptions {
        now_ms: nuvoletta_core::now_ms() + 400 * 24 * 3600 * 1000,
        ..GcOptions::default()
    };
    let r = a.syncer.collect_garbage(&a.vault, &opts).unwrap();
    assert_eq!(r.skipped, Some(GcSkip::NoDelete));
    assert_eq!(r.objects_collected, 0);
}

#[test]
fn a_server_without_etags_or_mtimes_works() {
    let s = TestServer::start(Policy {
        send_etags: false,
        send_mtimes: false,
        ..Policy::create_only()
    });
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Device::new(&tmp, "a", &s, None);
    let mut b = Device::new(&tmp, "b", &s, Some(&a));
    let n = a.create("x", "x", "one");
    a.sync();
    b.sync();
    assert_eq!(b.body(&n).as_deref(), Some("one"));
    b.set_body(&n, "two");
    b.sync();
    a.sync();
    assert_eq!(a.body(&n).as_deref(), Some("two"));
}

#[test]
fn an_unsafe_credential_stops_sync_before_anything_is_written() {
    let s = TestServer::start(Policy::full());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Device::new(&tmp, "a", &s, None);
    a.create("x", "x", "one");
    let e = a.try_sync().unwrap_err();
    assert!(
        matches!(
            e,
            nuvoletta_sync::Error::Provider(nuvoletta_sync::ProviderError::UnsafeCredentials(_))
        ),
        "{e}"
    );
    assert!(protocol_files(&s).is_empty());
    assert!(s.file_names().is_empty(), "probe files were cleaned up");
}

#[test]
fn an_interrupted_segment_upload_does_not_block_the_log_forever() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Device::new(&tmp, "a", &s, None);
    let mut b = Device::new(&tmp, "b", &s, Some(&a));
    let n = a.create("x", "x", "content that must arrive");
    // The segment upload is cut in half and the server keeps the half file.
    s.with(|st| st.faults.partial_put = vec![(".nvlog".to_owned(), 1)]);
    let old = a.syncer.device_id();
    // The cycle (or the next one, depending on whether the server dropped a reused connection)
    // finds its own name taken by a file that never completes. The grace period is zero in the
    // tests: the device continues under a new id and re-sends the entries.
    let mut warnings = Vec::new();
    warnings.extend(a.try_sync().map(|r| r.warnings).unwrap_or_default());
    warnings.extend(a.sync().warnings);
    let new = a.syncer.device_id();
    assert_ne!(old, new, "the device must have switched ids: {warnings:?}");
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, Warning::LogRestarted { .. })),
        "{warnings:?}"
    );
    b.sync();
    assert_eq!(b.body(&n).as_deref(), Some("content that must arrive"));
    // Nothing was removed or overwritten: the half file is still there, untouched.
    let half = s.with(|st| {
        st.files
            .keys()
            .filter(|k| k.contains(&format!("d/{old}/l/")))
            .cloned()
            .collect::<Vec<_>>()
    });
    assert_eq!(half.len(), 1);
    // Later edits keep flowing, both ways.
    a.set_body(&n, "second");
    a.sync();
    b.sync();
    assert_eq!(b.body(&n).as_deref(), Some("second"));
    b.set_body(&n, "third");
    b.sync();
    a.sync();
    assert_eq!(a.body(&n).as_deref(), Some("third"));
}

#[test]
fn an_interrupted_object_upload_is_resealed_under_a_new_id() {
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Device::new(&tmp, "a", &s, None);
    let mut b = Device::new(&tmp, "b", &s, Some(&a));
    let n = a.create("x", "x", "object content");
    s.with(|st| st.faults.partial_put = vec![(".nvobj".to_owned(), 1)]);
    let mut warnings = Vec::new();
    warnings.extend(a.try_sync().map(|r| r.warnings).unwrap_or_default());
    warnings.extend(a.sync().warnings);
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, Warning::ObjectResealed { .. })),
        "{warnings:?}"
    );
    // The note itself is unchanged locally, and the peer receives it.
    assert_eq!(a.body(&n).as_deref(), Some("object content"));
    a.sync();
    b.sync();
    assert_eq!(b.body(&n).as_deref(), Some("object content"));
}

#[test]
fn a_synced_folder_style_wait_applies_when_the_grace_period_has_not_passed() {
    // With a long grace period the device keeps waiting (the half file may still be growing).
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Device::new(&tmp, "a", &s, None);
    a.syncer = nuvoletta_sync::Syncer::open_with(
        &a.vault,
        std::sync::Arc::new(provider(&s)),
        nuvoletta_sync::SyncConfig {
            partial_upload_grace_ms: 3_600_000,
            ..sync_config()
        },
    )
    .unwrap();
    a.create("x", "x", "wait for it");
    s.with(|st| st.faults.partial_put = vec![(".nvlog".to_owned(), 1)]);
    let old = a.syncer.device_id();
    let _ = a.try_sync();
    let r = a.sync();
    assert_eq!(a.syncer.device_id(), old, "{:?}", r.warnings);
}
