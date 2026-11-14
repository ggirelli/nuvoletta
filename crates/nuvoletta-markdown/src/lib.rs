//! Markdown dialect, wikilinks and safe HTML rendering for Nuvoletta (ADR 0003).
//!
//! Pure functions over strings: no I/O and no dependency on storage. Notes are Markdown
//! (`CommonMark` plus tables, strikethrough, task lists and footnotes). Wikilinks (`[[Target]]`,
//! `[[Target|alias]]`, `[[Target#Heading|alias]]`) are stored as text in the body and never
//! rewritten; [`resolve`] maps a target to a note over a candidate set, and [`render_html`] turns
//! a body into HTML that is safe to inject into the preview.

#![forbid(unsafe_code)]

mod links;
mod render;
mod resolve;

use pulldown_cmark::Options;

pub use links::{MarkdownLink, WikiLink, markdown_links, parse_links};
pub use render::{RenderOptions, Resolver, render_html};
pub use resolve::{Candidate, Resolution, resolve};

/// The parser options that define the dialect. Shared by parsing and rendering so that both see
/// the same links.
pub(crate) const fn dialect() -> Options {
    Options::ENABLE_TABLES
        .union(Options::ENABLE_STRIKETHROUGH)
        .union(Options::ENABLE_TASKLISTS)
        .union(Options::ENABLE_FOOTNOTES)
        .union(Options::ENABLE_WIKILINKS)
}
