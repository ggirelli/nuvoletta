//! The application session: state machine, open-note buffer, autosave and auto-lock.

use crate::clock::Clock;
use crate::error::{Error, Result};
use crate::join::{JoinState, Joining};
use crate::platform::KdfLimit;
use crate::settings::Settings;
use crate::sync::{SyncSession, SyncTiming};
use crate::sync_settings::SyncSettings;
use crate::sync_types::SyncEvent;
use crate::tree::{TreeRow, build_tree};
use nuvoletta_core::{Note, NoteId, NotePath, now_ms};
use nuvoletta_crypto::{KdfParams, RecoveryKey};
use nuvoletta_markdown::{Index, RenderOptions, TagCount, render_html};
use nuvoletta_storage::portable::{
    ExportReport, ImportReport, export_markdown_avoiding, import_markdown,
};
use nuvoletta_storage::{Locked, NoteSummary, Unlocked};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

/// How long after the last edit the open note is saved automatically.
pub const DEFAULT_AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(1000);

/// Where the session is in its life cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No vault chosen yet: show the picker.
    NoVault,
    /// A vault is chosen but locked: show the unlock screen.
    Locked,
    /// The vault is unlocked.
    Unlocked,
}

/// A note as listed in panels: id, title and virtual path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRef {
    /// Note id.
    pub id: NoteId,
    /// Title.
    pub title: String,
    /// Virtual path.
    pub path: String,
}

/// A wikilink target that matches no note, with the note that contains it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedRef {
    /// The note containing the link.
    pub source: NoteRef,
    /// The link target as written.
    pub target: String,
}

/// A search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    /// The matching note.
    pub note: NoteRef,
    /// An excerpt around the first body match, if any.
    pub snippet: Option<String>,
}

/// Metadata and state of the open note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteInfo {
    /// Note id.
    pub id: NoteId,
    /// Title.
    pub title: String,
    /// Virtual path.
    pub path: String,
    /// Tags.
    pub tags: Vec<String>,
    /// The buffer differs from what is stored.
    pub dirty: bool,
    /// Why the last save failed; the buffer is kept.
    pub save_error: Option<String>,
    /// Sync replaced the stored version while there were unsaved edits; saving keeps both
    /// versions and shows the note under conflicts.
    pub changed_by_sync: bool,
    /// Bumped when sync replaced the editor content; the editor must reload it.
    pub generation: u64,
}

/// What a call to [`App::tick`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickOutcome {
    /// Nothing to do.
    Idle,
    /// The open note was autosaved.
    Saved,
    /// Autosave failed; the edits stay in the buffer and will be retried.
    SaveFailed(String),
    /// The inactivity period elapsed and the vault was locked.
    Locked,
    /// The inactivity period elapsed but unsaved edits could not be written, so the vault was
    /// **not** locked. The UI must tell the user and offer retry or an explicit forced lock.
    LockBlocked(String),
    /// The inactivity period elapsed while a sync cycle was running: the vault locks as soon as
    /// the cycle hands it back (after saving unsaved edits).
    LockPending,
    /// Something happened to the sync cycle (started, finished, or a pending lock completed).
    Sync(SyncEvent),
}

pub(crate) struct OpenNote {
    /// The stored revision (its body is the last saved body).
    pub(crate) note: Note,
    /// The editor content.
    pub(crate) buffer: Zeroizing<String>,
    pub(crate) last_edit: Duration,
    pub(crate) save_error: Option<String>,
    /// Sync replaced the stored version while the buffer had unsaved edits (see `sync.rs`).
    pub(crate) changed_by_sync: bool,
    /// The revision the unsaved edits were typed on, when sync replaced the stored note under
    /// them: the save is queued as its sibling (a visible conflict), never as a successor of
    /// the synced revision.
    pub(crate) sync_base: Option<[u8; 16]>,
    /// Bumped when sync replaces the buffer, so the editor reloads it.
    pub(crate) generation: u64,
}

impl OpenNote {
    pub(crate) fn dirty(&self) -> bool {
        *self.buffer != self.note.body
    }
}

pub(crate) struct Session {
    /// The vault. `None` only while a sync cycle on the worker thread has exclusive use of it;
    /// operations that need it then fail with [`Error::SyncBusy`].
    pub(crate) vault: Option<Unlocked>,
    /// The vault directory (known even while the vault is away).
    pub(crate) root: PathBuf,
    /// The note list as of the moment the vault was handed to the sync worker.
    pub(crate) listing: Vec<NoteSummary>,
    pub(crate) index: Index,
    pub(crate) open: Option<OpenNote>,
    pub(crate) last_activity: Duration,
    /// Set after a recovery-key unlock: the user must choose a new passphrase.
    pub(crate) must_change_passphrase: bool,
    pub(crate) sync: SyncSession,
    /// A join whose first sync has not succeeded yet (see `join.rs`).
    pub(crate) joining: Option<Joining>,
}

pub(crate) enum State {
    NoVault,
    Locked(Box<Locked>),
    Unlocked(Box<Session>),
}

/// The application: owns the vault, the index and the open note, and enforces the rules for
/// saving, navigating and locking. See the crate documentation.
pub struct App {
    clock: Arc<dyn Clock>,
    pub(crate) settings: Settings,
    kdf: KdfParams,
    pub(crate) kdf_limit: KdfLimit,
    pub(crate) join: Option<JoinState>,
    debounce: Duration,
    pub(crate) state: State,
    pub(crate) sync_cfg: SyncSettings,
    pub(crate) sync_timing: SyncTiming,
}

impl App {
    /// A new application with no vault open.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>, settings: Settings) -> Self {
        Self {
            clock,
            settings,
            kdf: KdfParams::DEFAULT,
            kdf_limit: KdfLimit::platform(),
            join: None,
            debounce: DEFAULT_AUTOSAVE_DEBOUNCE,
            state: State::NoVault,
            sync_cfg: SyncSettings::default(),
            sync_timing: SyncTiming::default(),
        }
    }

    /// Use other Argon2id parameters for vaults and passphrases created from now on. Tests only:
    /// the default is the production setting.
    pub fn set_kdf_params(&mut self, kdf: KdfParams) {
        self.kdf = kdf;
    }

    /// Change the Argon2id cost limit applied to headers taken from a sync location (tests, and
    /// the platform default otherwise).
    pub fn set_kdf_limit(&mut self, limit: KdfLimit) {
        self.kdf_limit = limit;
    }

    /// Change the autosave debounce.
    pub fn set_autosave_debounce(&mut self, debounce: Duration) {
        self.debounce = debounce;
    }

    /// Current life-cycle phase.
    #[must_use]
    pub fn phase(&self) -> Phase {
        match self.state {
            State::NoVault => Phase::NoVault,
            State::Locked(_) => Phase::Locked,
            State::Unlocked(_) => Phase::Unlocked,
        }
    }

    /// The current settings.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Set the auto-lock period (clamped to the valid range) and restart the idle timer.
    pub fn set_auto_lock_minutes(&mut self, minutes: u32) {
        self.settings.auto_lock_minutes = Settings::clamp_minutes(minutes);
        self.touch();
    }

    /// The vault directory, if a vault is chosen.
    #[must_use]
    pub fn vault_root(&self) -> Option<&Path> {
        match &self.state {
            State::NoVault => None,
            State::Locked(l) => Some(l.root()),
            State::Unlocked(s) => Some(&s.root),
        }
    }

    pub(crate) fn clock_now(&self) -> Duration {
        self.clock.now()
    }

    pub(crate) fn session(&self) -> Result<&Session> {
        match &self.state {
            State::Unlocked(s) => Ok(s),
            State::Locked(_) => Err(Error::Locked),
            State::NoVault => Err(Error::NoVault),
        }
    }

    pub(crate) fn session_mut(&mut self) -> Result<&mut Session> {
        match &mut self.state {
            State::Unlocked(s) => Ok(s),
            State::Locked(_) => Err(Error::Locked),
            State::NoVault => Err(Error::NoVault),
        }
    }

    pub(crate) fn not_unlocked(&self) -> Result<()> {
        if matches!(self.state, State::Unlocked(_)) {
            Err(Error::AlreadyUnlocked)
        } else {
            Ok(())
        }
    }

    // ---- vault life cycle -------------------------------------------------------------------

    /// Create a vault in `dir` and unlock it. Returns the recovery key in printable form; the
    /// caller must show it to the user exactly once and then drop it.
    ///
    /// # Errors
    /// The vault is unlocked already, a vault exists in `dir`, the passphrase is too short, or
    /// writing fails.
    pub fn create_vault(&mut self, dir: &Path, passphrase: &str) -> Result<Zeroizing<String>> {
        self.not_unlocked()?;
        let (vault, key) = Locked::create(dir, passphrase, self.kdf)?;
        let shown = key.to_display_string();
        self.install(vault, false)?;
        self.settings.last_vault = Some(dir.to_path_buf());
        Ok(shown)
    }

    /// Choose an existing vault directory; it stays locked.
    ///
    /// # Errors
    /// The vault is unlocked already, or `dir` is not a valid vault.
    pub fn open_vault(&mut self, dir: &Path) -> Result<()> {
        self.not_unlocked()?;
        // Release the vault chosen before (its single-instance lock), so choosing the same one
        // again works; if the new one cannot be opened the picker is shown.
        self.state = State::NoVault;
        self.state = State::Locked(Box::new(Locked::open(dir)?));
        self.settings.last_vault = Some(dir.to_path_buf());
        Ok(())
    }

    /// Unlock with the passphrase.
    ///
    /// # Errors
    /// Wrong passphrase (the state stays locked), or the vault cannot be read.
    pub fn unlock_with_passphrase(&mut self, passphrase: &str) -> Result<()> {
        let State::Locked(l) = &self.state else {
            return Err(self.wrong_state());
        };
        let vault = l.unlock_with_passphrase(passphrase)?;
        self.install(vault, false)
    }

    /// Unlock with the printable recovery key. The session then requires a new passphrase
    /// ([`App::must_change_passphrase`]).
    ///
    /// # Errors
    /// Malformed or wrong key (the state stays locked), or the vault cannot be read.
    pub fn unlock_with_recovery_key(&mut self, key: &str) -> Result<()> {
        let State::Locked(l) = &self.state else {
            return Err(self.wrong_state());
        };
        let key = RecoveryKey::parse(key)?;
        let vault = l.unlock_with_recovery_key(&key)?;
        self.install(vault, true)
    }

    pub(crate) fn wrong_state(&self) -> Error {
        match self.state {
            State::NoVault => Error::NoVault,
            _ => Error::AlreadyUnlocked,
        }
    }

    pub(crate) fn install(&mut self, vault: Unlocked, must_change: bool) -> Result<()> {
        let mut session = Session {
            root: vault.root().to_path_buf(),
            vault: Some(vault),
            listing: Vec::new(),
            index: Index::new(),
            open: None,
            last_activity: self.clock.now(),
            must_change_passphrase: must_change,
            sync: SyncSession::default(),
            joining: None,
        };
        session.rebuild_index();
        self.state = State::Unlocked(Box::new(session));
        self.sync_on_unlock();
        Ok(())
    }

    /// Lock the vault. Unsaved edits are saved first; if that fails the vault stays unlocked and
    /// [`Error::Unsaved`] is returned, unless `force` is set, which discards the unsaved edits
    /// (the UI must ask the user before forcing). Locking drops the keys, the index and the
    /// editor buffer.
    ///
    /// # Errors
    /// [`Error::Unsaved`], or [`Error::NoVault`].
    pub fn lock(&mut self, force: bool) -> Result<()> {
        let now = self.clock.now();
        match &mut self.state {
            State::NoVault => return Err(Error::NoVault),
            State::Locked(_) => return Ok(()),
            State::Unlocked(s) => {
                if s.vault.is_none() {
                    // A sync cycle has the vault: the lock happens when it hands it back.
                    s.sync.lock_after = Some(force || s.sync.lock_after == Some(true));
                    return Ok(());
                }
                if !force {
                    s.save(now).map_err(|e| Error::Unsaved(Box::new(e)))?;
                }
            }
        }
        if let State::Unlocked(s) = std::mem::replace(&mut self.state, State::NoVault) {
            let mut s = *s;
            // Moving the vault out drops (and zeroizes) the index and the buffer.
            self.state = match s.vault.take() {
                Some(v) => State::Locked(Box::new(v.lock())),
                // Unreachable (checked above); keep the session rather than lose the vault.
                None => State::Unlocked(Box::new(s)),
            };
        }
        Ok(())
    }

    /// Lock, then forget the vault so the picker is shown. See [`App::lock`] for `force`.
    ///
    /// # Errors
    /// As [`App::lock`].
    pub fn close_vault(&mut self, force: bool) -> Result<()> {
        if !matches!(self.state, State::NoVault) {
            // Closing waits for an in-flight sync cycle (it is short and safe to wait for), so
            // the unsaved buffer can be written before the vault is dropped.
            self.wait_sync();
            self.lock(force)?;
            self.state = State::NoVault;
        }
        Ok(())
    }

    /// True after a recovery-key unlock until a new passphrase is set.
    #[must_use]
    pub fn must_change_passphrase(&self) -> bool {
        self.session().is_ok_and(|s| s.must_change_passphrase)
    }

    /// Set a new passphrase (the notes are untouched).
    ///
    /// # Errors
    /// Passphrase too short or writing fails.
    pub fn change_passphrase(&mut self, new_passphrase: &str) -> Result<()> {
        let kdf = self.kdf;
        let s = self.session_mut()?;
        s.vault_mut()?.change_passphrase(new_passphrase, kdf)?;
        s.must_change_passphrase = false;
        Ok(())
    }

    /// Replace the recovery key; needs the current passphrase. Returns the new key in printable
    /// form, to be shown once.
    ///
    /// # Errors
    /// Wrong passphrase or writing fails.
    pub fn rotate_recovery_key(&mut self, passphrase: &str) -> Result<Zeroizing<String>> {
        let key = self
            .session_mut()?
            .vault_mut()?
            .rotate_recovery_key(passphrase)?;
        Ok(key.to_display_string())
    }

    /// Files in the vault that could not be read at unlock (corrupt, stray, symlinks).
    #[must_use]
    pub fn issues(&self) -> Vec<String> {
        self.session().map_or_else(
            |_| Vec::new(),
            |s| {
                s.vault.as_ref().map_or_else(Vec::new, |v| {
                    v.issues()
                        .iter()
                        .map(|i| format!("{:?}: {}", i.kind, i.name))
                        .collect()
                })
            },
        )
    }

    // ---- activity and timers ----------------------------------------------------------------

    /// Record user activity (restarts the idle timer). The UI calls this on input events.
    pub fn touch(&mut self) {
        let now = self.clock.now();
        if let State::Unlocked(s) = &mut self.state {
            s.last_activity = now;
        }
    }

    /// Time left before auto-lock, if unlocked.
    #[must_use]
    pub fn idle_remaining(&self) -> Option<Duration> {
        let s = self.session().ok()?;
        let limit = Duration::from_secs(u64::from(self.settings.auto_lock_minutes) * 60);
        Some(limit.saturating_sub(self.clock.now().saturating_sub(s.last_activity)))
    }

    /// Advance timers: lock after inactivity, autosave after the debounce. The UI calls this
    /// about once a second.
    pub fn tick(&mut self) -> TickOutcome {
        if let Some(ev) = self.poll_sync() {
            return TickOutcome::Sync(ev);
        }
        if self.idle_remaining() == Some(Duration::ZERO) {
            if self.lock_pending() {
                return TickOutcome::Idle;
            }
            return match self.lock(false) {
                Ok(()) if self.phase() == Phase::Unlocked => TickOutcome::LockPending,
                Ok(()) => TickOutcome::Locked,
                Err(e) => TickOutcome::LockBlocked(e.user_message()),
            };
        }
        let (now, debounce) = (self.clock.now(), self.debounce);
        let Ok(s) = self.session_mut() else {
            return TickOutcome::Idle;
        };
        let due = s.vault.is_some()
            && s.open
                .as_ref()
                .is_some_and(|o| o.dirty() && now.saturating_sub(o.last_edit) >= debounce);
        if due {
            return match s.save(now) {
                Ok(()) => TickOutcome::Saved,
                Err(e) => TickOutcome::SaveFailed(e.user_message()),
            };
        }
        if self.maybe_start_sync() {
            return TickOutcome::Sync(SyncEvent::Started);
        }
        TickOutcome::Idle
    }

    // ---- notes ------------------------------------------------------------------------------

    /// All notes as folder and note rows.
    #[must_use]
    pub fn tree(&self) -> Vec<TreeRow> {
        self.session()
            .map_or_else(|_| Vec::new(), |s| build_tree(&s.list()))
    }

    /// All notes, sorted by path.
    #[must_use]
    pub fn notes(&self) -> Vec<NoteRef> {
        self.session()
            .map_or_else(|_| Vec::new(), |s| s.list().iter().map(note_ref).collect())
    }

    /// Create an empty note named "Untitled" (numbered if taken) in `folder` (`""` for the top
    /// level) and open it. Unsaved edits of the current note are saved first.
    ///
    /// # Errors
    /// [`Error::Unsaved`] if the current note cannot be saved; invalid folder; storage errors.
    pub fn create_note(&mut self, folder: &str) -> Result<NoteId> {
        let now = self.clock.now();
        let s = self.session_mut()?;
        s.flush(now)?;
        let folder = folder.trim_matches('/');
        let taken: Vec<String> = s
            .list()
            .iter()
            .map(|n| n.path.as_str().to_lowercase())
            .collect();
        let mut n = 1;
        let (name, path) = loop {
            let name = if n == 1 {
                "Untitled".to_owned()
            } else {
                format!("Untitled {n}")
            };
            let path = join(folder, &name);
            if !taken.contains(&path.to_lowercase()) {
                break (name, path);
            }
            n += 1;
        };
        let note = Note::new(&name, &path, Vec::new(), "")?;
        s.vault_mut()?.create_note(&note)?;
        s.sync.changed_at = Some(now);
        s.index.upsert(&note);
        let id = note.id;
        s.open = Some(OpenNote::new(note, now));
        s.last_activity = now;
        Ok(id)
    }

    /// Open a note (saving the current one first).
    ///
    /// # Errors
    /// [`Error::Unsaved`] if the current note cannot be saved (it stays open); the note cannot
    /// be read.
    pub fn open_note(&mut self, id: NoteId) -> Result<()> {
        let now = self.clock.now();
        let s = self.session_mut()?;
        s.last_activity = now;
        if s.open.as_ref().is_some_and(|o| o.note.id == id) {
            return Ok(());
        }
        s.flush(now)?;
        let note = s.vault()?.read_note(&id)?;
        s.open = Some(OpenNote::new(note, now));
        Ok(())
    }

    /// Close the open note (saving it first).
    ///
    /// # Errors
    /// [`Error::Unsaved`] if it cannot be saved (it stays open).
    pub fn close_note(&mut self) -> Result<()> {
        let now = self.clock.now();
        let s = self.session_mut()?;
        s.flush(now)?;
        s.open = None;
        Ok(())
    }

    /// Metadata and state of the open note.
    #[must_use]
    pub fn open_info(&self) -> Option<NoteInfo> {
        let o = self.session().ok()?.open.as_ref()?;
        Some(NoteInfo {
            id: o.note.id,
            title: o.note.title.clone(),
            path: o.note.path.as_str().to_owned(),
            tags: o.note.tags.clone(),
            dirty: o.dirty(),
            save_error: o.save_error.clone(),
            changed_by_sync: o.changed_by_sync,
            generation: o.generation,
        })
    }

    /// The editor content of the open note.
    #[must_use]
    pub fn buffer(&self) -> Option<&str> {
        self.session()
            .ok()?
            .open
            .as_ref()
            .map(|o| o.buffer.as_str())
    }

    /// The editor reports new content for note `id`. Ignored (with [`Error::NotOpen`]) if `id`
    /// is not the open note, so a late message from a previous note cannot overwrite the current
    /// one.
    ///
    /// # Errors
    /// [`Error::NotOpen`].
    pub fn edit(&mut self, id: NoteId, text: &str) -> Result<()> {
        let now = self.clock.now();
        let s = self.session_mut()?;
        s.last_activity = now;
        let o = s
            .open
            .as_mut()
            .filter(|o| o.note.id == id)
            .ok_or(Error::NotOpen)?;
        if *o.buffer != text {
            o.buffer = Zeroizing::new(text.to_owned());
            o.last_edit = now;
        }
        Ok(())
    }

    /// Save the open note now.
    ///
    /// # Errors
    /// The write failed; the buffer is kept and the error is also in [`NoteInfo::save_error`].
    pub fn save_now(&mut self) -> Result<()> {
        let now = self.clock.now();
        self.session_mut()?.save(now)
    }

    /// Rename a note: sets its title and the last segment of its path to `name`.
    ///
    /// # Errors
    /// Empty name or a name with `/`; invalid path; storage errors. The open buffer is kept.
    pub fn rename_note(&mut self, id: NoteId, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.contains('/') {
            return Err(Error::Invalid("a name must not be empty or contain '/'"));
        }
        let name = name.to_owned();
        self.update_meta(id, move |n| {
            let folder = parent(n.path.as_str()).to_owned();
            n.path = NotePath::parse(&join(&folder, &name))?;
            n.title = name;
            Ok(())
        })
    }

    /// Move a note to `folder` (`""` for the top level), keeping its last path segment.
    ///
    /// # Errors
    /// Invalid folder; storage errors.
    pub fn move_note(&mut self, id: NoteId, folder: &str) -> Result<()> {
        let folder = folder.trim_matches('/').to_owned();
        self.update_meta(id, move |n| {
            let leaf = n.path.segments().last().unwrap_or_default().to_owned();
            n.path = NotePath::parse(&join(&folder, &leaf))?;
            Ok(())
        })
    }

    /// Replace a note's tags.
    ///
    /// # Errors
    /// Too many tags; storage errors.
    pub fn set_tags(&mut self, id: NoteId, tags: Vec<String>) -> Result<()> {
        self.update_meta(id, move |n| {
            n.tags = tags;
            Ok(())
        })
    }

    fn update_meta(
        &mut self,
        id: NoteId,
        f: impl FnOnce(&mut Note) -> nuvoletta_core::Result<()>,
    ) -> Result<()> {
        let now = self.clock.now();
        let s = self.session_mut()?;
        s.last_activity = now;
        let is_open = s.open.as_ref().is_some_and(|o| o.note.id == id);
        let mut n = match &s.open {
            Some(o) if is_open => {
                let mut n = o.note.clone();
                n.body = (*o.buffer).clone();
                n
            }
            _ => s.vault()?.read_note(&id)?,
        };
        f(&mut n)?;
        n.modified_at = now_ms();
        n.validate()?;
        s.vault_mut()?.update_note(&n)?;
        s.sync.changed_at = Some(now);
        s.index.upsert(&n);
        if let Some(o) = s.open.as_mut().filter(|_| is_open) {
            o.note = n;
            o.save_error = None;
        }
        Ok(())
    }

    /// Delete a note. If it is open, its buffer is discarded (the UI must have confirmed).
    ///
    /// # Errors
    /// Unknown note; storage errors.
    pub fn delete_note(&mut self, id: NoteId) -> Result<()> {
        let now = self.clock.now();
        let s = self.session_mut()?;
        s.last_activity = now;
        s.vault_mut()?.delete_note(&id)?;
        s.sync.changed_at = Some(now);
        s.index.remove(id);
        if s.open.as_ref().is_some_and(|o| o.note.id == id) {
            s.open = None;
        }
        Ok(())
    }

    // ---- queries ----------------------------------------------------------------------------

    /// Full-text search.
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let Ok(s) = self.session() else {
            return Vec::new();
        };
        s.index
            .search(query, limit)
            .into_iter()
            .filter_map(|h| {
                Some(SearchResult {
                    note: note_ref(&s.summary(&h.id)?),
                    snippet: h.snippet.clone(),
                })
            })
            .collect()
    }

    /// All tags with their note counts.
    #[must_use]
    pub fn tags(&self) -> Vec<TagCount> {
        self.session()
            .map_or_else(|_| Vec::new(), |s| s.index.tags())
    }

    /// Notes carrying `tag`.
    #[must_use]
    pub fn notes_with_tag(&self, tag: &str) -> Vec<NoteRef> {
        self.refs(|s| s.index.notes_with_tag(tag))
    }

    /// Notes that link to `id`.
    #[must_use]
    pub fn backlinks(&self, id: NoteId) -> Vec<NoteRef> {
        self.refs(|s| s.index.backlinks(id))
    }

    fn refs(&self, f: impl FnOnce(&Session) -> Vec<NoteId>) -> Vec<NoteRef> {
        let Ok(s) = self.session() else {
            return Vec::new();
        };
        f(s).iter()
            .filter_map(|id| s.summary(id).map(|n| note_ref(&n)))
            .collect()
    }

    /// Wikilinks that match no note, across the vault.
    #[must_use]
    pub fn unresolved(&self) -> Vec<UnresolvedRef> {
        let Ok(s) = self.session() else {
            return Vec::new();
        };
        s.index
            .unresolved()
            .into_iter()
            .filter_map(|u| {
                Some(UnresolvedRef {
                    source: note_ref(&s.summary(&u.source)?),
                    target: u.target,
                })
            })
            .collect()
    }

    /// Wikilinks of the open note that match no note.
    #[must_use]
    pub fn unresolved_in_open(&self) -> Vec<String> {
        let Ok(s) = self.session() else {
            return Vec::new();
        };
        let Some(o) = &s.open else {
            return Vec::new();
        };
        s.index
            .unresolved()
            .into_iter()
            .filter(|u| u.source == o.note.id)
            .map(|u| u.target)
            .collect()
    }

    /// The preview of the open buffer as safe HTML (raw HTML escaped, remote images off,
    /// wikilinks resolved through the index).
    #[must_use]
    pub fn preview_html(&self) -> String {
        let Ok(s) = self.session() else {
            return String::new();
        };
        let Some(o) = &s.open else {
            return String::new();
        };
        let resolver = |t: &str| s.index.resolve_target(t);
        render_html(&o.buffer, &RenderOptions::default(), &resolver)
    }

    // ---- export and import ------------------------------------------------------------------

    /// Export every note as plaintext Markdown into `dest`. **This writes plaintext**; the UI
    /// must have warned the user. Unsaved edits are saved first.
    ///
    /// # Errors
    /// [`Error::Unsaved`]; destination errors from storage.
    pub fn export(&mut self, dest: &Path) -> Result<ExportReport> {
        let now = self.clock.now();
        let synced = self.sync_cfg.folder.clone();
        let s = self.session_mut()?;
        s.flush(now)?;
        s.last_activity = now;
        // The synced folder is refused even while sync is switched off: it is still synced by
        // the external tool, and plaintext must never be written into it.
        Ok(export_markdown_avoiding(
            s.vault()?,
            dest,
            synced.as_deref().as_slice(),
        )?)
    }

    /// Import a folder of `.md` files, then rebuild the index.
    ///
    /// # Errors
    /// [`Error::Unsaved`]; source errors from storage.
    pub fn import(&mut self, src: &Path) -> Result<ImportReport> {
        let now = self.clock.now();
        let s = self.session_mut()?;
        s.flush(now)?;
        s.last_activity = now;
        let report = import_markdown(s.vault_mut()?, src);
        s.sync.changed_at = Some(now);
        // Rebuild even on error: part of the folder may have been imported.
        s.rebuild_index();
        Ok(report?)
    }
}

impl OpenNote {
    fn new(note: Note, now: Duration) -> Self {
        let buffer = Zeroizing::new(note.body.clone());
        Self {
            note,
            buffer,
            last_edit: now,
            save_error: None,
            changed_by_sync: false,
            sync_base: None,
            generation: 0,
        }
    }
}

impl Session {
    pub(crate) fn vault(&self) -> Result<&Unlocked> {
        self.vault.as_ref().ok_or(Error::SyncBusy)
    }

    pub(crate) fn vault_mut(&mut self) -> Result<&mut Unlocked> {
        self.vault.as_mut().ok_or(Error::SyncBusy)
    }

    /// The notes, from the vault or (while a sync cycle has it) from the snapshot taken when
    /// the cycle started.
    pub(crate) fn list(&self) -> Vec<NoteSummary> {
        self.vault
            .as_ref()
            .map_or_else(|| self.listing.clone(), Unlocked::list)
    }

    pub(crate) fn summary(&self, id: &NoteId) -> Option<NoteSummary> {
        match &self.vault {
            Some(v) => v.summary(id).cloned(),
            None => self.listing.iter().find(|n| n.id == *id).cloned(),
        }
    }

    pub(crate) fn rebuild_index(&mut self) {
        let Some(vault) = self.vault.as_ref() else {
            return;
        };
        // Stream the notes: only one decrypted note is held at a time besides the index itself.
        let mut index = Index::new();
        for s in vault.list() {
            if let Ok(note) = vault.read_note(&s.id) {
                index.upsert(&note);
            }
        }
        self.index = index;
    }

    /// Write the buffer if it differs from the stored body. On failure the buffer is kept.
    pub(crate) fn save(&mut self, now: Duration) -> Result<()> {
        let Some(o) = self.open.as_mut() else {
            return Ok(());
        };
        if !o.dirty() {
            o.save_error = None;
            return Ok(());
        }
        let Some(vault) = self.vault.as_mut() else {
            // A sync cycle has the vault: not a failure, the buffer is saved after it.
            return Err(Error::SyncBusy);
        };
        let mut n = o.note.clone();
        n.body = (*o.buffer).clone();
        n.modified_at = now_ms();
        match n
            .validate()
            .map_err(Error::from)
            .and_then(|()| vault.update_note(&n).map_err(Error::from))
        {
            Ok(()) => {
                self.sync.changed_at = Some(now);
                self.index.upsert(&n);
                if let Some(base) = o.sync_base
                    && let (Some(syncer), Some(vault)) =
                        (self.sync.syncer.as_deref_mut(), self.vault.as_ref())
                    && let Err(e) = syncer.note_edited_on(vault, &n.id, base)
                {
                    let e = Error::Sync(e.to_string());
                    o.note = n;
                    o.save_error = Some(e.user_message());
                    return Err(e);
                }
                o.sync_base = None;
                o.note = n;
                o.save_error = None;
                o.changed_by_sync = false;
                Ok(())
            }
            Err(e) => {
                o.save_error = Some(e.user_message());
                o.last_edit = now; // retry after another debounce period
                Err(e)
            }
        }
    }

    fn flush(&mut self, now: Duration) -> Result<()> {
        self.save(now).map_err(|e| Error::Unsaved(Box::new(e)))
    }
}

fn note_ref(s: &NoteSummary) -> NoteRef {
    NoteRef {
        id: s.id,
        title: s.title.clone(),
        path: s.path.as_str().to_owned(),
    }
}

fn join(folder: &str, leaf: &str) -> String {
    if folder.is_empty() {
        leaf.to_owned()
    } else {
        format!("{folder}/{leaf}")
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(p, _)| p)
}
