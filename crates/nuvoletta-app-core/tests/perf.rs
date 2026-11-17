//! Timing harness for large vaults (see `docs/PERFORMANCE.md`). Every test is `#[ignore]`d:
//! run with `just bench` (release build). It prints one `bench:` line per measurement and
//! asserts nothing about speed, so it never makes CI flaky.
//!
//! `NUVOLETTA_BENCH_NOTES` sets the vault size (default 5000), `NUVOLETTA_BENCH_NEW` the number
//! of new notes in the sync-of-new-notes measurement (default 1000).

use nuvoletta_app_core::{App, ManualClock, Settings, SyncEvent, WebDavEvent, WebDavInput};
use nuvoletta_core::{Note, NoteId};
use nuvoletta_crypto::KdfParams;
use nuvoletta_storage::Locked;
use nuvoletta_webdav::testing::{PASS, Policy, TestServer, USER};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

const PW: &str = "correct horse battery";

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn time<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    println!("bench: {label}: {:.1} ms", ms(t.elapsed()));
    r
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Deterministic pseudo-random numbers (xorshift); no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 >> 11).unwrap()
    }
}

const WORDS: &[&str] = &[
    "alpha", "bridge", "cloud", "design", "engine", "forest", "garden", "harbor", "island",
    "journal", "kernel", "lantern", "meadow", "network", "orchard", "planet", "quartz", "river",
    "signal", "temple", "update", "valley", "window", "yellow", "zenith", "protocol", "sync",
    "recipe", "meeting", "project", "budget", "travel", "reading", "garden", "health", "idea",
];

/// A realistic note: about 2 KiB of prose with a few headings, wikilinks and tags.
fn make_note(i: usize, total: usize, rng: &mut Rng) -> Note {
    let mut body = format!("# Note {i}\n\n");
    for p in 0..6 {
        for _ in 0..40 {
            body.push_str(WORDS[rng.next() % WORDS.len()]);
            body.push(' ');
        }
        body.push_str(&format!("[[Note {}]] ", rng.next() % total));
        if p % 3 == 0 {
            body.push_str(&format!("[[Note {}|see also]]", rng.next() % total));
        }
        body.push_str("\n\n");
    }
    let tags = vec![
        WORDS[rng.next() % WORDS.len()].to_owned(),
        format!("area{}", rng.next() % 20),
    ];
    let path = format!("Folder {}/Sub {}/Note {i}", i % 25, i % 7);
    Note::new(&format!("Note {i}"), &path, tags, &body).unwrap()
}

fn build_vault(dir: &Path, n: usize) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut v, _key) = Locked::create(dir, PW, KdfParams::MIN_ACCEPTED).unwrap();
    for i in 0..n {
        v.create_note(&make_note(i, n, &mut rng)).unwrap();
    }
}

fn new_app() -> App {
    let mut app = App::new(Arc::new(ManualClock::new()), Settings::default());
    app.set_kdf_params(KdfParams::MIN_ACCEPTED);
    app
}

fn sync(app: &mut App) {
    app.sync_now().unwrap();
    assert!(matches!(app.wait_sync(), Some(SyncEvent::Finished(f)) if f.ok));
}

fn add_notes(app: &mut App, from: usize, count: usize) {
    let mut rng = Rng(42 + from as u64);
    for i in from..from + count {
        let id = app.create_note("New").unwrap();
        app.rename_note(id, &format!("Fresh {i}")).unwrap();
        app.edit(id, &make_note(i, from + count, &mut rng).body)
            .unwrap();
        app.save_now().unwrap();
    }
}

fn some_ids(app: &App, count: usize) -> Vec<NoteId> {
    let notes = app.notes();
    let step = (notes.len() / count).max(1);
    notes
        .iter()
        .step_by(step)
        .take(count)
        .map(|n| n.id)
        .collect()
}

fn local_ops(n: usize) -> (tempfile::TempDir, App) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("v");
    time(
        &format!("n={n} generate vault (storage create_note)"),
        || {
            build_vault(&dir, n);
        },
    );
    time(&format!("n={n} storage unlock (decrypt all)"), || {
        let l = Locked::open(&dir).unwrap();
        drop(l.unlock_with_passphrase(PW).unwrap());
    });
    let mut app = new_app();
    app.open_vault(&dir).unwrap();
    time(
        &format!("n={n} app unlock (storage scan + index build)"),
        || {
            app.unlock_with_passphrase(PW).unwrap();
        },
    );
    time(&format!("n={n} notes() listing"), || app.notes().len());
    time(&format!("n={n} tree()"), || app.tree().len());
    time(&format!("n={n} 20 searches (2 terms, limit 50)"), || {
        for q in [
            "garden river",
            "protocol sync",
            "note 12",
            "zeni",
            "yellow lan",
        ] {
            for _ in 0..4 {
                let _ = app.search(q, 50);
            }
        }
    });
    let ids = some_ids(&app, 50);
    time(&format!("n={n} 50 backlinks()"), || {
        for id in &ids {
            let _ = app.backlinks(*id);
        }
    });
    time(&format!("n={n} unresolved()"), || app.unresolved().len());
    time(&format!("n={n} tags()"), || app.tags().len());
    time(&format!("n={n} open_note + 20 edit/save cycles"), || {
        app.open_note(ids[0]).unwrap();
        let mut text = app.buffer().unwrap().to_owned();
        for i in 0..20 {
            text.push_str(&format!("\nline {i}"));
            app.edit(ids[0], &text).unwrap();
            app.save_now().unwrap();
        }
    });
    time(&format!("n={n} lock"), || app.lock(true).unwrap());
    (tmp, app)
}

#[test]
#[ignore = "benchmark; run with `just bench`"]
fn vault_operations() {
    let n = env_usize("NUVOLETTA_BENCH_NOTES", 5000);
    let _ = local_ops(n);
}

#[test]
#[ignore = "benchmark; run with `just bench`"]
fn sync_over_folder() {
    let n = env_usize("NUVOLETTA_BENCH_NOTES", 5000);
    let new = env_usize("NUVOLETTA_BENCH_NEW", 1000);
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("v");
    let shared = tmp.path().join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    build_vault(&dir, n);
    let mut app = new_app();
    app.open_vault(&dir).unwrap();
    app.unlock_with_passphrase(PW).unwrap();
    app.configure_sync(&shared).unwrap();
    time(&format!("n={n} folder: first sync (push all)"), || {
        sync(&mut app)
    });
    time(&format!("n={n} folder: sync, no changes"), || {
        sync(&mut app)
    });
    add_notes(&mut app, n, new);
    time(&format!("n={n} folder: sync of {new} new notes"), || {
        sync(&mut app)
    });
    time(&format!("n={n} folder: sync, no changes (again)"), || {
        sync(&mut app)
    });
    time(&format!("n={n} folder: GC dry run"), || {
        app.cleanup_preview().unwrap()
    });
}

#[test]
#[ignore = "benchmark; run with `just bench`"]
fn sync_over_webdav() {
    let n = env_usize("NUVOLETTA_BENCH_NOTES", 5000);
    let new = env_usize("NUVOLETTA_BENCH_NEW", 1000);
    let s = TestServer::start(Policy::create_only());
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("v");
    build_vault(&dir, n);
    let mut app = new_app();
    app.open_vault(&dir).unwrap();
    app.unlock_with_passphrase(PW).unwrap();
    let input = |pin: bool| WebDavInput {
        url: s.url(),
        username: USER.to_owned(),
        password: Zeroizing::new(PASS.to_owned()),
        pin: pin.then(|| nuvoletta_webdav::format_fingerprint(&s.fingerprint)),
    };
    for pin in [false, true] {
        app.webdav_check_start(&input(pin)).unwrap();
        let e = loop {
            if let Some(e) = app.poll_webdav() {
                break e;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(matches!(
            e,
            WebDavEvent::NeedsPin { .. } | WebDavEvent::Saved
        ));
    }
    time(&format!("n={n} webdav: first sync (push all)"), || {
        sync(&mut app)
    });
    time(&format!("n={n} webdav: sync, no changes"), || {
        sync(&mut app)
    });
    add_notes(&mut app, n, new);
    time(&format!("n={n} webdav: sync of {new} new notes"), || {
        sync(&mut app)
    });
    time(&format!("n={n} webdav: sync, no changes (again)"), || {
        sync(&mut app)
    });
}
