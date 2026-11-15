# Accessibility

Phase 7 review of the application views against the basics of WCAG 2.2 level AA. The views are
Dioxus components in `apps/nuvoletta-app/src`, the styles are `apps/nuvoletta-app/assets/main.css`
and the dialog glue is `assets/a11y.js`; behaviour stays in `nuvoletta-app-core`. This was a review
of the code and the palette, not a test with assistive technology: see
[manual testing](#manual-testing-still-needed).

## Already in place

* Native controls (`button`, `input`, `label`) everywhere, so keyboard operation and names come for
  free; icon-only buttons have an accessible name.
* Every dialog has `role="dialog"` or `"alertdialog"`, `aria-modal`, a labelling heading and an
  initial focus (`autofocus`); the settings, sync, conflict and Recently deleted dialogs close with
  Escape.
* Errors use `role="alert"`, confirmations and status text `role="status"` (picker, unlock, settings,
  sync, WebDAV set-up, banners, top-bar status, sync button text).
* Visible focus (`:focus-visible`, 2 px ring, at least 5.9:1 against the surfaces) and a skip link.
* No animation, no hover-only information, no content that depends on colour alone (unresolved links
  are also wavy-underlined, errors have text).
* Touch targets of about 44 px under `(pointer: coarse)`, 16 px inputs (no zoom on focus), and a
  single-column layout on narrow screens.
* Light and dark palettes follow the system setting.

## Fixed in this review

| Area | Finding | Fix |
| --- | --- | --- |
| Focus (2.4.3, 2.1.2) | `aria-modal` does not stop Tab from reaching the page behind a dialog in WebKitGTK, WebView2 or Android WebView | `a11y.js` keeps Tab inside the topmost modal and returns focus to the control that opened it |
| Keyboard (2.1.1) | Delete confirmation and "vault still unlocked" dialogs had no Escape | Escape means the safe choice (cancel, keep working); nothing is deleted or discarded |
| Names and roles (4.1.2) | Note list used `role="tree"` and `treeitem` without arrow-key behaviour or nesting, which promises behaviour that is not there | Plain nested lists with buttons; Tab moves between notes |
| Names and roles (4.1.2) | Recovery key was a `textbox` role on a `div` | `group` with a label, so the groups of characters are read as text |
| Status (4.1.3) | Search results changed without any announcement | A visually hidden status line says how many notes were found |
| Bypass blocks (2.4.1) | The skip link pointed to a non-focusable `div` | It moves focus into the editor |
| Non-text contrast (1.4.11) | Input and button borders were 1.4:1 to 1.8:1 | New `--nv-control` colour: 3.2:1 or more (light) and 4.3:1 or more (dark) on every surface |
| Contrast (1.4.3) | Search snippets in the selected row were 3.7:1 in the light theme | They inherit the row's text colour (5.9:1) |
| Target size (2.5.8) | Link-style buttons could be about 21 px high with a mouse | Minimum 24 px |
| Resize text (1.4.4) | Base size was `15px` | `0.9375rem`, so the system or browser text size applies; the top bar wraps |
| Motion (2.3.3) | No reduced-motion handling (the app has no animation today) | `prefers-reduced-motion` rule that disables any future animation |
| Forced colours | Borders and the selected note relied on colours that high contrast mode removes | Explicit `forced-colors` rules |
| Language (3.1.1) | No `lang` on the document | `lang="en"` is set at start-up |

## Palette contrast

Ratios are WCAG relative luminance contrast, computed from the hex values in `main.css`. Text needs
4.5:1, focus rings and control borders 3:1.

| Pair | Light | Dark |
| --- | --- | --- |
| Text on page / panel / raised | 7.8 / 8.3 / 8.9 | 17.4 / 16.0 / 14.2 |
| Muted text on page / panel / raised | 4.9 / 5.2 / 5.6 | 9.6 / 8.9 / 7.8 |
| Muted text on hover | 4.6 | 7.4 |
| Accent (links) on page / panel / raised | 5.3 / 5.6 / 6.1 | 12.4 / 11.5 / 10.2 |
| Text on selected row | 5.9 | 8.3 |
| Button text on the primary button | 8.3 | 14.6 |
| Error text on page / panel / error banner | 6.6 / 7.0 / 6.5 | 11.1 / 10.2 / 9.0 |
| Text on success banner | 7.7 | 12.5 |
| Focus ring on page / panel / raised | 5.9 / 6.3 / 6.8 | 9.6 / 8.8 / 7.8 |
| Control border (new) on page / panel / raised | 3.8 / 4.0 / 4.3 | 5.6 / 5.2 / 4.6 |

Decorative borders (`--nv-border`, 1.4:1 to 1.8:1) separate panels only; they never carry the
boundary of a control. The brand blue is used for the blockquote bar and the recovery-key frame,
which are decoration.

## Remaining items

* **Dialog background is not inert.** The Tab trap and Escape keep keyboard users inside, but a
  screen reader's virtual cursor can still read the page behind a modal. Marking the background
  `inert` is the proper fix; it needs the root view to set the attribute on `.app`, to be done
  together with the screen reader tests.
* **Editor.** CodeMirror 6 has its own accessibility support (a labelled `contenteditable`, optional
  screen reader hints) which was not reviewed here. Check that its label, the Tab key behaviour (Tab
  indents; Escape then Tab leaves the editor) and the line announcements are acceptable, and
  document the Escape-Tab exit in the user guide.
* **Live preview** is re-rendered on every edit and is not announced (correct), but screen reader
  users have no cue that it exists apart from its label; consider a "Preview" heading.
* **Conflict view** shows added and removed lines with colour and background only; the diff lines
  should also carry a text or visually hidden prefix ("added", "removed") for screen readers.
* **Status text** in the top bar is a live region that changes after every save; if testing shows it
  is too chatty, limit announcements to errors.
* **Text scaling on Android** follows the system font scale in the WebView, but layouts at 200 % on
  a narrow phone were not checked.
* **Zoom and reflow (1.4.10)** at 400 % on desktop was not checked.
* **Drag and drop** is not used anywhere; there is no pointer-only gesture (2.5.1 holds).

## Manual testing still needed

Automated checks cannot say whether the experience is usable. Before the first release, run through
unlock, create and edit a note, search, open the conflict view, Recently deleted, sync and WebDAV
settings, and the recovery-key screen with:

* Orca with Firefox-style navigation in WebKitGTK (Linux);
* NVDA and Narrator (Windows, WebView2);
* VoiceOver (macOS, WKWebView);
* TalkBack (Android, with large font size and display size at their maximum, and with an external
  keyboard);
* keyboard only on every desktop platform, and a high-contrast theme on Windows.

Record findings as issues, linked from this page.
