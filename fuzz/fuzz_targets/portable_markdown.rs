//! Portable (plaintext Markdown) import parser and export emitter: front matter, YAML subset,
//! timestamps and file name escaping.
#![no_main]
use libfuzzer_sys::fuzz_target;
use nuvoletta_core::Note;
use nuvoletta_storage::portable::fuzzing::{emit_file, escape_segment, parse_note};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let (name, content) = text.split_once('\u{0}').unwrap_or(("untitled", text));
    let segs: Vec<String> = name.split('/').map(str::to_owned).collect();

    // Whatever a file contains, import either skips it or yields a valid note.
    if let Ok(note) = parse_note(content, &segs) {
        note.validate().expect("imported note is valid");
        // Export then import is lossless for every note import can produce.
        let again = parse_note(&emit_file(&note), &segs).expect("exported file re-imports");
        assert_note_eq(&note, &again);
    }

    // File names are plain names: nothing can leave the destination folder.
    let esc = escape_segment(name);
    assert!(!esc.is_empty() && esc != "." && esc != "..");
    assert!(!esc.starts_with('.') && !esc.ends_with(['.', ' ']));
    assert!(!esc.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*']));
    assert!(!esc.chars().any(char::is_control));
    // Cut to 200 bytes, then at most three single characters become `%XX`.
    assert!(esc.len() <= 206);
});

fn assert_note_eq(a: &Note, b: &Note) {
    assert_eq!(a.id, b.id);
    assert_eq!(a.title, b.title);
    assert_eq!(a.path, b.path);
    assert_eq!(a.tags, b.tags);
    assert_eq!(a.created_at, b.created_at);
    assert_eq!(a.modified_at, b.modified_at);
    assert_eq!(a.body, b.body);
}
