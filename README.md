# Veil

A local-first, multilingual data anonymization and
pseudonymization desktop application.

## Technology Stack

- React + TypeScript — User Interface
- Tauri + Rust — Desktop Application and Core
- Python — PII Detection and Machine Learning
Download and install these five tools:

Tool

Download

Purpose

Git

https://git-scm.com/downloads/win

Download and update Veil

Python 3.12 (64-bit)

https://www.python.org/downloads/

Run the PII detection engine

Node.js LTS

https://nodejs.org/

Run the React frontend

Rust

https://rustup.rs/

Compile the Tauri backend

Visual Studio Build Tools

https://visualstudio.microsoft.com/visual-cpp-build-tools/

Compile native Windows applications

Important installation notes:

For Python, enable Add Python to PATH if the installer offers it.

For Visual Studio Build Tools, select Desktop development with C++ (including MSVC and the Windows SDK).

For Rust, use the default stable MSVC toolchain.

Windows also needs Microsoft Edge WebView2 Runtime, which is normally already installed.
## Status

Early development — Phase 1

## Multilingual detection

Select English, French, Spanish, or Arabic in **Document language** before
detecting. Choose the data categories to check and review suggestions before
processing. Automatically excluded suggestions remain inspectable. See [multilingual setup and limitations](docs/multilingual-detection.md)
for model installation on each computer and current language coverage.


## Local Vault sign-in

Anonymization works without creating or unlocking a Vault. It replaces reviewed
information with consistent document-local tokens such as `PERSON_001`, `EMAIL_001`, and `LOCATION_001`, keeps the result only in the current workspace,
and supports TXT/PDF export without saving document history to the Vault.
Pseudonymization requires an unlocked Vault to save reusable identity mappings,
processed document history, and originals for restoration. Existing history is
kept until explicitly cleared.

In Settings > Vault sign-in, choose one of three modes:

- **Windows Hello / macOS sign-in:** native identity verification each time the Vault is unlocked. Windows uses Hello PIN, fingerprint or face when configured. macOS uses Touch ID or the Mac login password. Cancelling or failing verification keeps the Vault locked.
- **Separate Vault passphrase:** a passphrase of at least 12 characters, independent of the operating-system login.
- **No extra Vault password:** automatic opening for the signed-in computer account.

Unlock the existing Vault before changing its mode, then click **Apply Vault setting**. Enabling system sign-in verifies it before saving the change. Removing an existing system sign-in requirement also requires verification. Existing no-extra-password Vaults retain their setting until changed.

All modes keep the Vault encrypted locally. Account modes store the encryption key using Windows DPAPI or macOS Keychain; passphrase mode derives it using Argon2. Native sign-in adds an application access check; it does not make account keys hardware-bound or protect against malicious software already running as the same OS user. VEIL never receives the OS password, PIN or biometric data.

System sign-in requires the OS authentication service to be available. If Windows Hello is not configured, set up a PIN in Windows Settings > Accounts > Sign-in options, or use a Vault passphrase. The macOS implementation requires validation on a Mac before release.

Reset and clear-data controls are in Settings. Resetting the entire Vault requires typing **RESET VAULT** and permanently removes its identities, mappings, rules and history. Exported documents and separate backups are unaffected.
