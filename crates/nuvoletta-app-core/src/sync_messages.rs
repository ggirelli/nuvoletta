//! Words for the sync view: integrity warnings and errors turned into explanations that say what
//! happened, what it means and what to do. Never includes note content or keys.

use crate::sync_settings::SyncBackend;
use crate::sync_types::{InvalidInfo, StrayInfo, SyncNotice, SyncProblem};
use nuvoletta_sync::{Error, InvalidEntry, ProviderError, StrayFile, StrayKind, Warning};

/// First eight characters of an id, enough to tell devices apart in a message.
pub(crate) fn short(id: impl std::fmt::Display) -> String {
    id.to_string().chars().take(8).collect()
}

/// Describe a warning of the sync engine.
pub(crate) fn describe_warning(w: &Warning, now_ms: i64) -> SyncNotice {
    let (code, title, detail) = match w {
        Warning::Rollback {
            device,
            known_seq,
            remote_seq,
        } => (
            "rollback",
            "The sync folder went back in time".to_owned(),
            format!(
                "Device {} had {known_seq} entries before, but the folder now shows only {remote_seq}. \
                 The folder may have been restored from a backup, or files were deleted or withheld \
                 (possibly by the storage provider). Nothing was reverted on this device. Check your \
                 sync tool for restored or missing files; if you restored a backup on purpose, this is expected.",
                short(device)
            ),
        ),
        Warning::DeviceFork { device, seq } => (
            "device-fork",
            "A device ID was used twice".to_owned(),
            format!(
                "Two copies of the same vault (for example a copied vault folder or a restored backup) wrote \
                 under device {} and their histories differ from entry {seq} on. That device's history is \
                 frozen there and nothing after it is applied. Affected notes are listed under Needs attention. \
                 Do not copy vault folders between computers: set up the other computer from the sync folder instead.",
                short(device)
            ),
        ),
        Warning::OwnForkRecovered { old, new } => (
            "own-fork-recovered",
            "This device switched to a new ID".to_owned(),
            format!(
                "Another copy of this vault used this device's ID ({}), so this device continues as {}. \
                 No notes were lost. This usually means the vault folder was copied or restored from a \
                 backup; avoid running two copies of the same vault.",
                short(old),
                short(new)
            ),
        ),
        Warning::PossiblyWithheld {
            device,
            claimed_seq,
            have_seq,
        } => (
            "possibly-withheld",
            "Some changes may be missing from the sync folder".to_owned(),
            format!(
                "Another device reports having seen entries of device {} up to {claimed_seq}, but only {have_seq} \
                 are visible here. Files may still be syncing, or were removed or withheld. Wait for your sync \
                 tool to finish; if this stays, check the folder on the other device.",
                short(device)
            ),
        ),
        Warning::DivergentCheckpoint {
            reporter,
            device,
            seq,
        } => (
            "divergent-checkpoint",
            "Devices disagree about the history".to_owned(),
            format!(
                "Device {} recorded a different entry {seq} for device {} than this device verified. \
                 This can mean a forked device ID or a tampered folder. Compare the vault copies you use.",
                short(reporter),
                short(device)
            ),
        ),
        Warning::StreamGap {
            device,
            expected_seq,
        } => (
            "stream-gap",
            "Files are missing or still syncing".to_owned(),
            format!(
                "The history of device {} stops before entry {expected_seq}: a file has not arrived or is \
                 corrupted. Changes after it are not applied yet. Let your sync tool finish; if it persists, \
                 the file may be corrupted or withheld.",
                short(device)
            ),
        ),
        Warning::ObjectUnavailable { rev } => (
            "object-unavailable",
            "A note version is missing or corrupted".to_owned(),
            format!(
                "The content of version {} did not arrive or failed verification several times. It is retried \
                 every cycle. If your sync tool shows errors for it, resolve them there; if it stays, the \
                 file may be corrupted.",
                short(rev)
            ),
        ),
        Warning::LogRestarted { old, new } => (
            "log-restarted",
            "An interrupted upload blocked this device's history, so it started a new one"
                .to_owned(),
            format!(
                "An upload was cut off and the server kept a damaged file under the name this device needs next. \
                 The account cannot remove or replace files, so this device continues as {} (it was {}). \
                 Nothing was lost: unsent changes were sent again. Other devices may show a warning that the \
                 old history stops at a damaged file; that warning is expected and harmless.",
                short(new),
                short(old)
            ),
        ),
        Warning::ObjectResealed { rev } => (
            "object-resealed",
            "An interrupted upload of a note version was repeated".to_owned(),
            format!(
                "The upload of version {} was cut off and the server kept a damaged file under its name. The note \
                 was saved again under a new name and sent; nothing was lost.",
                short(rev)
            ),
        ),
        Warning::HeaderNewer { generation } => (
            "header-newer",
            "The passphrase or recovery key was changed elsewhere".to_owned(),
            format!(
                "Another device published vault header generation {generation}. Lock this vault and unlock it \
                 again with the new passphrase."
            ),
        ),
        Warning::HeaderRollback { seen, remote } => (
            "header-rollback",
            "The sync folder has an older vault header".to_owned(),
            format!(
                "The newest vault header in the folder is generation {remote}, but generation {seen} was seen \
                 before. The folder may have been rolled back to an older state."
            ),
        ),
        Warning::HeaderInvalid { generation } => (
            "header-invalid",
            "A vault header in the sync folder is invalid".to_owned(),
            format!(
                "The header file for generation {generation} is damaged, belongs to another vault, or differs \
                 from this vault's. Check that the sync folder is the right one."
            ),
        ),
    };
    SyncNotice {
        code: code.to_owned(),
        title,
        detail,
        first_seen_ms: now_ms,
    }
}

/// Describe an invalid entry as a notice too (possibly corrupted segment).
pub(crate) fn describe_invalid(e: &InvalidEntry, now_ms: i64) -> SyncNotice {
    SyncNotice {
        code: "invalid-entry".to_owned(),
        title: "An entry in the sync folder is invalid".to_owned(),
        detail: format!(
            "A log {} of device {} authenticated but is not valid ({}). It was not applied. The file may be \
             corrupted or written by a faulty client.",
            if e.seq == 0 {
                "segment".to_owned()
            } else {
                format!("entry {}", e.seq)
            },
            short(e.device),
            e.reason
        ),
        first_seen_ms: now_ms,
    }
}

pub(crate) fn invalid_info(e: &InvalidEntry) -> InvalidInfo {
    InvalidInfo {
        device: short(e.device),
        seq: e.seq,
        reason: e.reason.to_owned(),
    }
}

pub(crate) fn stray_info(s: &StrayFile) -> StrayInfo {
    let treatment = match s.kind {
        StrayKind::Ignored => "Ignored: not a Nuvoletta file.",
        StrayKind::DuplicateCopy => {
            "A copy of a Nuvoletta file made by your sync tool; the duplicate is ignored."
        }
        StrayKind::CopyOnly => {
            "A copy of a Nuvoletta file whose original is missing; reported, not applied."
        }
        StrayKind::ForkEvidence => {
            "A copy that contradicts the verified history: evidence that a device ID was used twice."
        }
        StrayKind::NotYetArrived => "Incomplete: probably still being synced.",
    };
    StrayInfo {
        name: s.name.clone(),
        size: s.size,
        treatment: treatment.to_owned(),
    }
}

fn problem(message: impl Into<String>, hint: impl Into<String>) -> SyncProblem {
    SyncProblem {
        message: message.into(),
        hint: hint.into(),
    }
}

/// Describe a failed cycle.
pub(crate) fn describe_error(e: &Error, backend: SyncBackend) -> SyncProblem {
    let dav = backend == SyncBackend::WebDav;
    match e {
        Error::Provider(ProviderError::Transient(d)) if dav => problem(
            format!("The sync server could not be reached ({d})."),
            "Sync retries automatically. Check your connection and that the server is up.",
        ),
        Error::Provider(ProviderError::Transient(d)) => problem(
            format!("The sync folder could not be reached ({d})."),
            "Sync retries automatically. If the folder is on a removable or network drive, check that it is connected.",
        ),
        Error::Provider(ProviderError::Auth(_)) => problem(
            "The server rejected the user name or password.",
            "The password may have been changed or revoked. Enter the credentials again in the Sync settings.",
        ),
        Error::Provider(ProviderError::Forbidden(d)) => problem(
            format!("The account is not allowed to do this ({d})."),
            "Check the permissions of the account: it needs to read and create files.",
        ),
        Error::Provider(ProviderError::UnsafeCredentials(d)) => problem(
            "Sync is stopped: this account can change or delete files on the server.",
            d.clone(),
        ),
        Error::Provider(ProviderError::NotFound(d) | ProviderError::Other(d)) if dav => problem(
            format!("The sync server could not be used ({d})."),
            "Check the address and the account in the Sync settings.",
        ),
        Error::Provider(ProviderError::NotFound(d) | ProviderError::Other(d)) => problem(
            format!("The sync folder could not be used ({d})."),
            "Check that the folder still exists and that you may read and write it, or choose another folder in Settings.",
        ),
        Error::Provider(ProviderError::AlreadyExists(d)) => problem(
            format!("A file in the sync location was in the way ({d})."),
            "Nothing was overwritten. Sync will try again; if this repeats, check the location for stray files.",
        ),
        Error::Io(d) => problem(
            format!("Local sync data could not be read or written ({d})."),
            "Check free disk space and permissions of the vault folder. Your notes are not affected.",
        ),
        Error::State(d) => problem(
            format!("The local sync data is damaged ({d})."),
            "Your notes were not changed and nothing was uploaded from the damaged state. Most of it is rebuilt from the sync location; if this persists, keep a copy of the vault folder before turning sync off and on.",
        ),
        Error::Crypto(_) | Error::Storage(_) => problem(
            "A file failed verification or could not be read.",
            "The sync location may belong to a different vault or contain damaged files. Your notes were not changed.",
        ),
        other => problem(
            format!("Sync stopped: {other}."),
            "Nothing was lost. Try again; if it repeats, report it.",
        ),
    }
}

/// Describe a failed resolution action.
pub(crate) fn describe_resolution_error(e: &Error) -> String {
    describe_resolution_error_for(e, SyncBackend::Folder)
}

/// Like [`describe_resolution_error`], with the wording of the backend in use.
pub(crate) fn describe_resolution_error_for(e: &Error, backend: SyncBackend) -> String {
    match e {
        Error::InvalidResolution(_) | Error::NoteNotFound => {
            "The versions of this note changed in the meantime. Review them again.".to_owned()
        }
        Error::RevisionUnavailable => {
            "That version has not finished syncing yet. Try again after the next sync.".to_owned()
        }
        other => describe_error(other, backend).message,
    }
}
