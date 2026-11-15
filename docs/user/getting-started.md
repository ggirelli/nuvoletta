# Getting started

## Install and open

Build or download nuvoletta for your system (see the [app guide](../APP.md) for requirements and the
Android build). There is no installer-time account, sign-up or server: the app is a program that
works on a folder on your disk.

## Create a vault

1. Choose *Create a new vault* and pick an **empty folder** (typed or with *Browse...*). This folder is
   your vault; every note is stored in it as an encrypted file.
2. Choose a **passphrase**. The minimum is 12 characters. Use a long phrase of your own that you do
   not use anywhere else. Nobody, including the author of the app, can reset it.
3. **Write down the recovery key** that is shown once. Put it on paper (or in a password manager)
   somewhere other than the device. The app does not continue until you confirm. See
   [Recovery key](recovery-key.md).

## Use it

- Notes are Markdown, organised by a `/`-separated path (folders are part of the path). The editor is
  on the left and the preview on the right. Notes save automatically a second after you stop typing.
- `[[Another note]]` links notes. Search covers titles, paths, tags and text.
- The vault **locks** after inactivity (default 5 minutes) and with *Lock* (Ctrl+L). Locking drops the
  keys from the app's memory; you unlock with the passphrase.

## Before you rely on it

1. Make a [backup](backup-and-restore.md) of the vault folder, and keep the recovery key separately.
2. Decide whether you want [sync](sync-setup.md). It is off until you set it up.
3. Read [Security and privacy](security-and-privacy.md) for what nuvoletta does and does not protect.
