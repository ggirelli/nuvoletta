//! Throwaway feasibility spike: Dioxus + CodeMirror 6 editor, safe Markdown preview,
//! keyring and app-private file checks.
use dioxus::prelude::*;
use pulldown_cmark::{Event, Options, Parser, html};

const EDITOR_JS: Asset = asset!("/assets/editor.js");
const CSS: Asset = asset!("/assets/main.css");

const SAMPLE: &str = "# Sample note\n\nSome **bold** text and a [[wikilink]].\n\n\
<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>\n\nInline <b onclick=alert(1)>html</b> too.\n";

/// Render Markdown to HTML with raw HTML turned into escaped text (inert).
fn render(md: &str) -> String {
    let parser = Parser::new_ext(md, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH).map(|e| match e {
        Event::Html(s) | Event::InlineHtml(s) => Event::Text(s), // escaped by push_html
        other => other,
    });
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

fn data_dir() -> std::path::PathBuf {
    #[cfg(target_os = "android")]
    {
        // App files dir; a real app should obtain it via JNI (Context.getFilesDir()).
        std::path::PathBuf::from("/data/data/dev.nuvoletta.spike/files")
    }
    #[cfg(not(target_os = "android"))]
    {
        dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("nuvoletta-spike")
    }
}

fn file_check() -> String {
    let dir = data_dir();
    let run = || -> std::io::Result<String> {
        std::fs::create_dir_all(&dir)?;
        let p = dir.join("note.md");
        std::fs::write(&p, "hello")?;
        let back = std::fs::read_to_string(&p)?;
        Ok(format!("{} -> read back {back:?}", p.display()))
    };
    run().unwrap_or_else(|e| format!("FAIL {}: {e}", dir.display()))
}

fn keyring_check() -> String {
    #[cfg(target_os = "android")]
    {
        "n/a on Android (needs Keystore via JNI)".into()
    }
    #[cfg(not(target_os = "android"))]
    {
        let run = || -> Result<String, keyring::Error> {
            let e = keyring::Entry::new("nuvoletta-spike", "dummy")?;
            e.set_password("s3cret")?;
            let got = e.get_password()?;
            e.delete_credential()?;
            let gone = matches!(e.get_password(), Err(keyring::Error::NoEntry));
            Ok(format!("set/get={got:?} deleted={gone}"))
        };
        run().unwrap_or_else(|e| format!("FAIL: {e}"))
    }
}

#[component]
fn App() -> Element {
    let mut text = use_signal(|| SAMPLE.to_string());
    let mut show_preview = use_signal(|| false);
    let html = use_memo(move || render(&text()));
    let status = use_hook(|| format!("keyring: {}\nfile: {}", keyring_check(), file_check()));

    // Bridge: JS -> Rust via eval channel (dioxus.send); wait for the local bundle to load.
    use_effect(move || {
        let init = serde_json_string(&SAMPLE);
        let mut ev = document::eval(&format!(
            r#"
            const init = {init};
            const wait = () => new Promise(r => {{
              const t = setInterval(() => {{ if (window.nvEditor && document.getElementById('cm')) {{ clearInterval(t); r(); }} }}, 50);
            }});
            await wait();
            if (!window.nvEditor.view) window.nvEditor.mount(document.getElementById('cm'), init, t => dioxus.send(t));
            "#
        ));
        spawn(async move {
            while let Ok(t) = ev.recv::<String>().await {
                text.set(t);
            }
        });
    });

    rsx! {
        document::Stylesheet { href: CSS }
        document::Script { src: EDITOR_JS }
        div { class: "bar",
            button { onclick: move |_| show_preview.toggle(), "toggle preview (narrow screens)" }
            pre { class: "status", "{status}" }
        }
        div { class: if show_preview() { "panes show-preview" } else { "panes" },
            div { id: "cm", class: "pane editor" }
            div { class: "pane preview", dangerous_inner_html: "{html}" }
        }
    }
}

/// Tiny JSON string escaper (avoids a serde_json dep for the spike).
fn serde_json_string(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '<' => o.push_str("\\u003c"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn main() {
    dioxus::launch(App);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_html_is_escaped() {
        let h = render(SAMPLE);
        assert!(!h.contains("<script"), "{h}");
        assert!(!h.contains("<img"), "{h}");
        assert!(!h.contains("<b onclick"), "{h}");
        assert!(h.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    }
    #[test]
    fn keyring_and_file() {
        println!("{}\n{}", keyring_check(), file_check());
    }
}
