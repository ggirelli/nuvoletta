//! Export and import of plaintext Markdown.

mod common;

use common::{all_files, create, note};
use nuvoletta_core::{Note, NoteId};
use nuvoletta_storage::portable::{SkipReason, export_markdown, import_markdown};
use nuvoletta_storage::{Error, Unlocked};
use std::fs;
use std::path::Path;

fn fresh_vault(dir: &Path, name: &str) -> Unlocked {
    create(&dir.join(name)).0
}

fn rel_files(dest: &Path) -> Vec<String> {
    let mut v: Vec<String> = all_files(dest)
        .iter()
        .map(|p| {
            p.strip_prefix(dest)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    v.sort();
    v
}

fn sorted_notes(v: &Unlocked) -> Vec<Note> {
    v.list()
        .iter()
        .map(|s| v.read_note(&s.id).unwrap())
        .collect()
}

fn with_note(v: &mut Unlocked, title: &str, path: &str, tags: &[&str], body: &str) -> Note {
    let mut n = note(title, path, body);
    n.tags = tags.iter().map(|s| (*s).to_owned()).collect();
    v.create_note(&n).unwrap();
    n
}

#[test]
fn export_then_import_round_trips_every_field() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = fresh_vault(dir.path(), "a");
    let titles = [
        (
            "Caf\u{e9} \u{65e5}\u{672c} \u{1f600}",
            "dossier/caf\u{e9}/\u{65e5}\u{672c}",
        ),
        ("key: value", "yaml/colon"),
        ("# not a heading", "yaml/hash"),
        ("\"quoted\" and 'single'", "yaml/quotes"),
        ("line one\nline two\ttab\r\nthree", "yaml/newlines"),
        ("---", "yaml/dashes"),
        ("back\\slash \\n", "yaml/backslash"),
        ("  padded  ", "yaml/padded"),
        ("", "empty-title"),
        ("sep \u{2028} \u{85} \u{feff}", "yaml/unicode-separators"),
    ];
    let bodies = [
        "",
        "plain body\n",
        "---\nnot: front matter\n---\nreal body\n",
        "---\n",
        "\n\n---\n---\n",
        "no trailing newline",
        "crlf\r\nbody\r\n",
        "id: \"abc\"\ntitle: fake\n",
    ];
    let mut originals = Vec::new();
    for (i, (title, path)) in titles.iter().enumerate() {
        let mut n = note(title, path, bodies[i % bodies.len()]);
        n.created_at = 1_700_000_000_000 + i as i64 * 1234;
        n.modified_at = n.created_at + 987;
        n.tags = match i % 3 {
            0 => vec![],
            1 => vec!["a, b".into(), "c]".into(), "\"q\"".into(), String::new()],
            _ => vec!["#tag".into(), "caf\u{e9}".into()],
        };
        a.create_note(&n).unwrap();
        originals.push(n);
    }
    for (i, b) in bodies.iter().enumerate() {
        let mut n = note("B", &format!("bodies/{i}"), b);
        n.created_at = -1;
        n.modified_at = i64::MAX;
        a.create_note(&n).unwrap();
        originals.push(n);
    }

    let dest = dir.path().join("out");
    let rep = export_markdown(&a, &dest).unwrap();
    assert_eq!(rep.exported.len(), originals.len());
    assert!(rep.failed.is_empty());
    assert!(dest.join("dossier/caf\u{e9}/\u{65e5}\u{672c}.md").is_file());

    let mut b = fresh_vault(dir.path(), "b");
    let rep = import_markdown(&mut b, &dest).unwrap();
    assert_eq!(rep.imported.len(), originals.len(), "{:?}", rep.skipped);
    assert!(rep.skipped.is_empty() && rep.conflicts.is_empty());
    for o in &originals {
        assert_eq!(&b.read_note(&o.id).unwrap(), o, "{:?}", o.title);
    }
    assert_eq!(sorted_notes(&a), sorted_notes(&b));
}

#[test]
fn exported_file_starts_with_documented_front_matter_and_keeps_body_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = fresh_vault(dir.path(), "v");
    let mut n = note("Hello: world", "a/b", "# Body\n---\nx\n");
    n.created_at = 1_700_000_000_123;
    n.modified_at = 1_700_000_001_000;
    v.create_note(&n).unwrap();
    let dest = dir.path().join("out");
    export_markdown(&v, &dest).unwrap();
    let text = fs::read_to_string(dest.join("a/b.md")).unwrap();
    let expected = format!(
        "---\nid: \"{}\"\ntitle: \"Hello: world\"\npath: \"a/b\"\ntags: [\"tag-one\", \"tag-two\"]\n\
         created: 2023-11-14T22:13:20.123Z\nmodified: 2023-11-14T22:13:21.000Z\n---\n# Body\n---\nx\n",
        n.id
    );
    assert_eq!(text, expected);
}

#[test]
fn export_refuses_a_non_empty_destination_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = fresh_vault(dir.path(), "v");
    with_note(&mut v, "t", "a", &[], "x");
    let dest = dir.path().join("out");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("a.md"), "precious").unwrap();
    assert!(matches!(
        export_markdown(&v, &dest),
        Err(Error::DestinationNotEmpty)
    ));
    assert_eq!(fs::read_to_string(dest.join("a.md")).unwrap(), "precious");

    let file = dir.path().join("file");
    fs::write(&file, "").unwrap();
    assert!(matches!(
        export_markdown(&v, &file),
        Err(Error::UnsafeEntry(_))
    ));
    // An empty existing directory and a missing one are fine.
    let empty = dir.path().join("empty");
    fs::create_dir(&empty).unwrap();
    export_markdown(&v, &empty).unwrap();
    export_markdown(&v, &dir.path().join("new/nested/dest")).unwrap();
}

#[cfg(unix)]
#[test]
fn export_refuses_a_symlinked_destination_and_writes_private_files() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let mut v = fresh_vault(dir.path(), "v");
    with_note(&mut v, "t", "a/b", &[], "x");
    let real = dir.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = dir.path().join("link");
    symlink(&real, &link).unwrap();
    assert!(matches!(
        export_markdown(&v, &link),
        Err(Error::UnsafeEntry(_))
    ));
    let dest = dir.path().join("out");
    export_markdown(&v, &dest).unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dest.join("a/b.md")), 0o600);
    assert_eq!(mode(&dest.join("a")), 0o700);
}

#[test]
fn reserved_and_invalid_names_are_escaped_and_stay_inside_the_destination() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = fresh_vault(dir.path(), "v");
    let paths = [
        "CON",
        "nul/Aux.txt",
        "a:b/c*d?",
        "trailing./dir ",
        ".hidden/.git",
        "back\\slash",
        "a..b/..c",
        &"x".repeat(400),
    ];
    for p in paths {
        with_note(&mut v, "t", p, &[], "x");
    }
    let dest = dir.path().join("out");
    export_markdown(&v, &dest).unwrap();
    let files = rel_files(&dest);
    assert_eq!(files.len(), paths.len());
    for f in &files {
        for seg in f.split('/') {
            assert!(
                !seg.contains([':', '*', '?', '\\', '<', '>', '|', '"']),
                "{f}"
            );
            assert!(
                !seg.starts_with('.') && !seg.ends_with('.') && !seg.ends_with(' '),
                "{f}"
            );
            assert!(seg.len() <= 255, "{f}");
            let stem = seg.split('.').next().unwrap().to_uppercase();
            assert!(!["CON", "NUL", "AUX"].contains(&stem.as_str()), "{f}");
        }
    }
    assert!(files.contains(&"%43ON.md".to_owned()), "{files:?}");
    assert!(files.contains(&"a%3Ab/c%2Ad%3F.md".to_owned()), "{files:?}");
    assert!(
        files.contains(&"trailing%2E/dir%20.md".to_owned()),
        "{files:?}"
    );
    // Nothing escaped the destination.
    let outside: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(outside.len(), 2); // "v" and "out"
}

#[test]
fn colliding_names_are_both_kept_including_on_case_insensitive_file_systems() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = fresh_vault(dir.path(), "v");
    let a = with_note(&mut v, "1", "Notes/Idea", &[], "one");
    let b = with_note(&mut v, "2", "notes/idea", &[], "two");
    let c = with_note(&mut v, "3", "a:b", &[], "three"); // escapes to a%3Ab
    let d = with_note(&mut v, "4", "a%3Ab", &[], "four");
    let e = with_note(&mut v, "5", "x.md", &[], "file x.md");
    let f = with_note(&mut v, "6", "x.md/y", &[], "dir x.md");
    let g = with_note(&mut v, "7", "cafe\u{301}", &[], "nfd"); // normalised to NFC by NotePath
    let h = with_note(&mut v, "8", "Caf\u{e9}", &[], "nfc");
    let dest = dir.path().join("out");
    let rep = export_markdown(&v, &dest).unwrap();
    assert_eq!(rep.exported.len(), 8);
    let mut folded: Vec<String> = rel_files(&dest).iter().map(|s| s.to_lowercase()).collect();
    folded.sort();
    folded.dedup();
    assert_eq!(folded.len(), 8, "{:?}", rel_files(&dest));
    assert_eq!(rep.exported.iter().filter(|x| x.disambiguated).count(), 3);

    let mut w = fresh_vault(dir.path(), "w");
    let imp = import_markdown(&mut w, &dest).unwrap();
    assert_eq!(imp.imported.len(), 8);
    for n in [a, b, c, d, e, f, g, h] {
        assert_eq!(w.read_note(&n.id).unwrap(), n);
    }
}

#[test]
fn files_without_front_matter_get_title_path_and_a_fresh_id_from_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("projects/2026")).unwrap();
    fs::write(src.join("projects/2026/Idea.md"), "# Just text\n").unwrap();
    fs::write(src.join("TOP.MD"), "---\nhorizontal rule, never closed\n").unwrap();
    fs::write(src.join("empty.md"), "").unwrap();
    let mut v = fresh_vault(dir.path(), "v");
    let rep = import_markdown(&mut v, &src).unwrap();
    assert_eq!(rep.imported.len(), 3, "{:?}", rep.skipped);
    let notes = sorted_notes(&v);
    let idea = notes.iter().find(|n| n.title == "Idea").unwrap();
    assert_eq!(idea.path.as_str(), "projects/2026/Idea");
    assert_eq!(idea.body, "# Just text\n");
    assert!(idea.tags.is_empty());
    assert_eq!(idea.created_at, idea.modified_at);
    let top = notes.iter().find(|n| n.title == "TOP").unwrap();
    assert_eq!(top.body, "---\nhorizontal rule, never closed\n");
    assert!(
        notes
            .iter()
            .any(|n| n.title == "empty" && n.body.is_empty())
    );
    // Re-importing creates new notes: ids are fresh each time for files without ids.
    import_markdown(&mut v, &src).unwrap();
    assert_eq!(v.list().len(), 6);
}

#[test]
fn partial_front_matter_uses_what_is_present_and_reports_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(
        src.join("n.md"),
        "---\ntitle: Custom\ntags:\n  - x\n  - y\ncreated: 2023-11-14T22:13:20Z\nauthor: me\naliases: [q]\n---\nbody",
    )
    .unwrap();
    let mut v = fresh_vault(dir.path(), "v");
    let rep = import_markdown(&mut v, &src).unwrap();
    assert_eq!(rep.imported.len(), 1);
    assert_eq!(rep.imported[0].unknown_keys, ["author", "aliases"]);
    let n = v.read_note(&rep.imported[0].id).unwrap();
    assert_eq!(n.title, "Custom");
    assert_eq!(n.tags, ["x", "y"]);
    assert_eq!(n.path.as_str(), "n");
    assert_eq!(
        (n.created_at, n.modified_at),
        (1_700_000_000_000, 1_700_000_000_000)
    );
    assert_eq!(n.body, "body");
}

#[test]
fn an_existing_id_is_a_reported_conflict_imported_as_a_new_note_never_an_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = fresh_vault(dir.path(), "a");
    let original = with_note(&mut a, "Mine", "p/q", &["t"], "original body");
    let dest = dir.path().join("out");
    export_markdown(&a, &dest).unwrap();

    // Identical content: nothing to do, not a conflict.
    let rep = import_markdown(&mut a, &dest).unwrap();
    assert_eq!(rep.already_present, ["p/q.md"]);
    assert!(rep.imported.is_empty() && rep.conflicts.is_empty() && rep.skipped.is_empty());
    assert_eq!(a.list().len(), 1);

    // Edited export: same id, different content.
    let file = dest.join("p/q.md");
    let edited = fs::read_to_string(&file)
        .unwrap()
        .replace("original body", "edited elsewhere");
    fs::write(&file, edited).unwrap();
    let rep = import_markdown(&mut a, &dest).unwrap();
    assert_eq!(rep.conflicts.len(), 1);
    assert_eq!(rep.conflicts[0].existing_id, original.id);
    assert_ne!(rep.conflicts[0].new_id, original.id);
    assert_eq!(rep.imported.len(), 1);
    assert_eq!(rep.imported[0].id, rep.conflicts[0].new_id);
    assert_eq!(a.read_note(&original.id).unwrap(), original);
    let new = a.read_note(&rep.conflicts[0].new_id).unwrap();
    assert_eq!(new.body, "edited elsewhere");
    assert_eq!(new.title, "Mine");
}

#[test]
fn two_files_with_the_same_id_are_both_kept() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir(&src).unwrap();
    let id = NoteId::from_bytes([7; 16]);
    fs::write(src.join("a.md"), format!("---\nid: {id}\n---\nA")).unwrap();
    fs::write(src.join("b.md"), format!("---\nid: {id}\n---\nB")).unwrap();
    let mut v = fresh_vault(dir.path(), "v");
    let rep = import_markdown(&mut v, &src).unwrap();
    assert_eq!(rep.imported.len(), 2);
    assert_eq!(rep.conflicts.len(), 1);
    assert_eq!(rep.conflicts[0].file, "b.md");
    assert_eq!(v.read_note(&id).unwrap().body, "A");
}

#[test]
fn bad_files_are_skipped_with_a_reason_and_do_not_stop_the_import() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join(".git")).unwrap();
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("good.md"), "ok").unwrap();
    fs::write(src.join("sub/also good.md"), "ok").unwrap();
    fs::write(src.join("binary.md"), [0xff, 0xfe, 0x00, 0xc3, 0x28]).unwrap();
    fs::write(src.join("malformed.md"), "---\ntitle: \"open\n---\nbody").unwrap();
    fs::write(src.join("badid.md"), "---\nid: zzz\n---\nbody").unwrap();
    fs::write(src.join("badpath.md"), "---\npath: \"a/../b\"\n---\nbody").unwrap();
    fs::write(
        src.join("longtitle.md"),
        format!("---\ntitle: {}\n---\n", "t".repeat(2000)),
    )
    .unwrap();
    fs::write(src.join("image.png"), "x").unwrap();
    fs::write(src.join(".md"), "no stem").unwrap();
    fs::write(src.join(".git/config.md"), "ignored").unwrap();

    let mut v = fresh_vault(dir.path(), "v");
    let rep = import_markdown(&mut v, &src).unwrap();
    assert_eq!(rep.imported.len(), 2);
    let reason = |f: &str| {
        rep.skipped
            .iter()
            .find(|s| s.file == f)
            .unwrap_or_else(|| panic!("{f} not skipped: {:?}", rep.skipped))
            .reason
            .clone()
    };
    assert_eq!(reason("binary.md"), SkipReason::NotUtf8);
    assert!(matches!(
        reason("malformed.md"),
        SkipReason::MalformedFrontMatter(_)
    ));
    assert!(matches!(
        reason("badid.md"),
        SkipReason::MalformedFrontMatter(_)
    ));
    assert!(matches!(reason("badpath.md"), SkipReason::InvalidPath(_)));
    assert!(matches!(reason("longtitle.md"), SkipReason::InvalidNote(_)));
    assert_eq!(reason("image.png"), SkipReason::NotMarkdown);
    assert!(matches!(
        reason(".md"),
        SkipReason::NotMarkdown | SkipReason::InvalidPath(_)
    ));
    assert_eq!(reason(".git"), SkipReason::HiddenDirectory);
    assert_eq!(v.list().len(), 2);
}

#[test]
fn a_file_over_the_body_limit_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("big.md"), vec![b'a'; nuvoletta_core::MAX_BODY + 1]).unwrap();
    fs::write(
        src.join("huge.md"),
        vec![b'a'; nuvoletta_core::MAX_BODY + 2 * 1024 * 1024],
    )
    .unwrap();
    let mut v = fresh_vault(dir.path(), "v");
    let rep = import_markdown(&mut v, &src).unwrap();
    assert!(rep.imported.is_empty());
    assert_eq!(rep.skipped.len(), 2);
    assert!(rep.skipped.iter().all(|s| s.reason == SkipReason::TooLarge));
}

#[cfg(unix)]
#[test]
fn symlinks_are_skipped_not_followed() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.md"), "outside the tree").unwrap();
    let src = dir.path().join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("real.md"), "in").unwrap();
    symlink(outside.join("secret.md"), src.join("link.md")).unwrap();
    symlink(&outside, src.join("linkdir")).unwrap();
    let mut v = fresh_vault(dir.path(), "v");
    let rep = import_markdown(&mut v, &src).unwrap();
    assert_eq!(rep.imported.len(), 1);
    let mut skipped: Vec<_> = rep
        .skipped
        .iter()
        .map(|s| (s.file.as_str(), s.reason.clone()))
        .collect();
    skipped.sort_by_key(|s| s.0);
    assert_eq!(
        skipped,
        [
            ("link.md", SkipReason::Symlink),
            ("linkdir", SkipReason::Symlink)
        ]
    );
    // A symlinked source root is refused.
    let root_link = dir.path().join("rootlink");
    symlink(&src, &root_link).unwrap();
    assert!(import_markdown(&mut v, &root_link).is_err());
}

#[test]
fn import_of_a_missing_source_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = fresh_vault(dir.path(), "v");
    assert!(import_markdown(&mut v, &dir.path().join("nope")).is_err());
}

#[test]
fn export_into_the_vault_and_import_from_around_it_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = fresh_vault(dir.path(), "v");
    with_note(&mut v, "t", "a", &[], "x");
    let root = dir.path().join("v");
    for dest in [
        root.clone(),
        root.join("export"),
        root.join("notes/../new/deep"),
    ] {
        assert!(matches!(
            export_markdown(&v, &dest),
            Err(Error::DestinationInsideVault)
        ));
    }
    assert!(!root.join("export").exists());
    // Parent of the vault (contains it) and a folder inside it.
    for src in [dir.path().to_path_buf(), root.clone(), root.join("notes")] {
        assert!(matches!(
            import_markdown(&mut v, &src),
            Err(Error::SourceOverlapsVault)
        ));
    }
    #[cfg(unix)]
    {
        let link = dir.path().join("alias");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert!(matches!(
            export_markdown(&v, &link.join("out")),
            Err(Error::DestinationInsideVault)
        ));
    }
    // A sibling is fine.
    export_markdown(&v, &dir.path().join("vault-export")).unwrap();
}
