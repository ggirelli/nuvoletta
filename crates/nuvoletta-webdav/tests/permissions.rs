//! The credential check: read+create accounts pass; accounts that can delete, overwrite, move
//! or copy over files are refused; unknown answers fail closed.

mod common;

use common::*;
use nuvoletta_sync::{Provider, ProviderError};
use nuvoletta_webdav::{RefusalReason, Verdict};

fn verdict(server: &TestServer) -> Verdict {
    provider(server).verify().expect("check completes")
}

fn reason(v: Verdict) -> RefusalReason {
    match v {
        Verdict::Refused(r) => r.reason,
        Verdict::Safe { .. } => panic!("expected a refusal"),
    }
}

#[test]
fn read_create_only_account_is_accepted() {
    let s = TestServer::start(Policy::create_only());
    let v = verdict(&s);
    assert!(matches!(v, Verdict::Safe { .. }), "{v:?}");
    // The protocol folders exist now and nothing but probe files was written.
    let names = s.file_names();
    assert!(names.iter().all(|n| {
        n.rsplit('/')
            .next()
            .unwrap()
            .starts_with(".nuvoletta-probe")
    }));
    // Everything the check tried to destroy is still there.
    assert!(names.contains(&".nuvoletta-probe".to_owned()));
}

#[test]
fn a_second_check_reuses_the_probe_files() {
    let s = TestServer::start(Policy::create_only());
    assert!(matches!(verdict(&s), Verdict::Safe { .. }));
    let first = s.file_names().len();
    // A new session (no setup) adds nothing.
    let p = provider(&s);
    p.list("").unwrap();
    assert_eq!(s.file_names().len(), first);
}

#[test]
fn delete_capable_accounts_are_refused() {
    let s = TestServer::start(Policy {
        all_delete: true,
        ..Policy::create_only()
    });
    assert!(matches!(reason(verdict(&s)), RefusalReason::CanDelete(_)));
    // The probe files were cleaned up again by the delete-capable account.
    assert!(s.file_names().is_empty(), "{:?}", s.file_names());
}

#[test]
fn delete_allowed_only_in_a_subdirectory_is_refused() {
    let mut p = Policy::create_only();
    p.delete_in.insert("o".to_owned());
    let s = TestServer::start(p);
    match reason(verdict(&s)) {
        RefusalReason::CanDelete(w) => assert!(w.contains("o/"), "{w}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn overwrite_capable_accounts_are_refused() {
    let s = TestServer::start(Policy {
        allow_overwrite: true,
        ..Policy::create_only()
    });
    assert!(matches!(
        reason(verdict(&s)),
        RefusalReason::CanOverwrite(_)
    ));
}

#[test]
fn copy_over_capable_accounts_are_refused() {
    let s = TestServer::start(Policy {
        allow_copy_over: true,
        ..Policy::create_only()
    });
    assert!(matches!(
        reason(verdict(&s)),
        RefusalReason::CanOverwrite(_)
    ));
}

#[test]
fn move_capable_accounts_are_refused() {
    let s = TestServer::start(Policy {
        allow_move: true,
        ..Policy::create_only()
    });
    assert!(matches!(reason(verdict(&s)), RefusalReason::CanMove(_)));
}

#[test]
fn accounts_that_cannot_create_are_refused() {
    let s = TestServer::start(Policy {
        allow_create: false,
        ..Policy::create_only()
    });
    assert!(matches!(
        reason(verdict(&s)),
        RefusalReason::CannotCreate(_)
    ));
    let s = TestServer::start(Policy {
        allow_mkcol: false,
        ..Policy::create_only()
    });
    assert!(matches!(
        reason(verdict(&s)),
        RefusalReason::CannotCreate(_)
    ));
}

#[test]
fn unknown_answers_fail_closed() {
    // DELETE answers 500: neither success nor a refusal.
    let s = TestServer::start(Policy {
        delete_status_override: Some(500),
        ..Policy::create_only()
    });
    assert!(matches!(
        reason(verdict(&s)),
        RefusalReason::Indeterminate(_)
    ));
    let s = TestServer::start(Policy {
        delete_status_override: Some(404),
        ..Policy::create_only()
    });
    assert!(matches!(
        reason(verdict(&s)),
        RefusalReason::Indeterminate(_)
    ));
}

#[test]
fn nextcloud_permission_letters_refuse_without_writing() {
    for (perm, ok) in [
        ("RGDNVCK", false),
        ("SRGCKW", false),
        ("SGCKNV", false),
        ("SG", false),
    ] {
        let s = TestServer::start(Policy {
            oc_permissions: Some(perm.to_owned()),
            ..Policy::create_only()
        });
        let v = verdict(&s);
        assert_eq!(matches!(v, Verdict::Safe { .. }), ok, "{perm}");
        assert!(
            s.file_names().is_empty(),
            "the fast path must not write ({perm})"
        );
    }
    // A create-only share passes the fast path and is then proven by the probes.
    let s = TestServer::start(Policy {
        oc_permissions: Some("SGCK".to_owned()),
        ..Policy::create_only()
    });
    assert!(matches!(verdict(&s), Verdict::Safe { .. }));
}

#[test]
fn a_server_that_lies_about_permissions_is_still_refused() {
    // Reports a harmless permission string but really allows deletion: the probe decides.
    let s = TestServer::start(Policy {
        oc_permissions: Some("SGCK".to_owned()),
        all_delete: true,
        ..Policy::create_only()
    });
    assert!(matches!(reason(verdict(&s)), RefusalReason::CanDelete(_)));
}

#[test]
fn an_unchecked_provider_refuses_every_operation_when_unsafe() {
    let s = TestServer::start(Policy::full());
    let p = provider(&s);
    for r in [
        p.list("").map(|_| ()),
        p.get("h/00000001.nvhdr").map(|_| ()),
        p.put_new("h/00000001.nvhdr", b"x"),
    ] {
        assert!(
            matches!(r, Err(ProviderError::UnsafeCredentials(_))),
            "{r:?}"
        );
    }
    // Nothing was written (the probes were cleaned up) and no protocol file exists.
    assert!(s.file_names().is_empty());
}

#[test]
fn permissions_that_change_are_caught_by_the_next_session() {
    let s = TestServer::start(Policy::create_only());
    let p = provider(&s);
    p.list("").unwrap();
    s.set_policy(Policy::full());
    // The same session keeps working (it was checked), a new one is refused.
    let p2 = provider(&s);
    assert!(matches!(
        p2.list(""),
        Err(ProviderError::UnsafeCredentials(_))
    ));
}
