# Implementation summary

Branch `feat/sqlcipher-database`. Starting commit and checked remote main `af89a997707dcdb69a0843d595a18099b7f2713f`. Implementation is left uncommitted for review.

- Rust/Tauri now opens SQLCipher for the persistent Vault. The six-table schema preserves General, project order and scope, normalized mappings, global counters, exact restoration snapshots and ordered custom phrases. Anonymization uses an isolated in-memory mapping set.
- Authenticated legacy data moves to a verified private generation. The old marker is replaced atomically only after reopening the new database and saving byte-for-byte encrypted recovery data.
- New code is under `apps/desktop/src-tauri/src/database/`, with the SQL cipher probe under `apps/desktop/src-tauri/examples/`. The Tauri commands, session code, review category contract and test-only legacy writer/production legacy reader are integrated in `lib.rs`, `protection.rs`, `review.rs` and `vault_storage.rs`. `App.tsx` adds cache clearing, stale response checks and submission IDs while keeping the existing UI.
- Cargo pins rusqlite 0.43.0 and selects `bundled-sqlcipher-vendored-openssl`. The lockfile includes the SQLCipher amalgamation crate 420.0.0, libsqlite3-sys 0.40.0 and vendored OpenSSL dependencies. The running probe identified SQLCipher 4.20.0 Community, SQLite 3.54.1 and OpenSSL 3.6.3. Nix adds Perl for OpenSSL configure. The existing C toolchain, GNU make, pkg-config and Tauri/WebKitGTK stack continue to apply. A Linux executable linker map in `build.rs` keeps the bundled SQLite symbols local because the WebKitGTK native runtime loads ordinary SQLite for its own purposes.

## Launch in an isolated NixOS profile

Use the existing Node and Python dependencies or install them with the project's standard setup. The model requirements and optional separate Arabic installation are described in [multilingual detection](../multilingual-detection.md).

```sh
veil_test_profile=$(mktemp -d)
XDG_DATA_HOME="$veil_test_profile/data" XDG_CONFIG_HOME="$veil_test_profile/config" \
  nix develop --command bash -c 'cd apps/desktop && ./node_modules/.bin/tauri dev'
```

Use a synthetic passphrase and synthetic text. Create two projects. Accept PII spans and pseudonymize the same name in both projects. Verify a stable token on repetition within each project and distinct global allocations across projects. Lock and unlock. Restore a saved entry and compare original whitespace. Delete a mapping and observe history remains. Clear history, delete a named project, change the passphrase, then verify explicit reset with only synthetic data. Test anonymization with Vault locked. Inspect only the temporary profile. Before first unlock in a real profile, users should preserve an encrypted backup if they need to retain access from a legacy build.

## Packaging and platforms

- **NixOS x86_64 tested** with the pinned dev shell. Linux Rust, Python and frontend tests were exercised. The developer's real GUI Vault was not opened.
- **Windows and macOS expected paths** use the existing platform-specific Rust dependencies and compile-time cfgs. No Windows/MSVC or macOS target build/native prompt/installer was run in this environment. Test clean Windows WebView2/MSVC and macOS arm64 and x86_64 if both are released. Validate DPAPI, Keychain permissions/signing, platform file replacement and installer updates.
- **Other Linux packaging** also requires target WebKitGTK, desktop libraries and compatible runtime loader dependencies. NixOS installers should use a native closure and appropriate runtime paths. Verify Tauri bundles on clean targets before release. SQLCipher and its cryptographic backend are built via Cargo, avoiding a separate end-user SQLCipher installation. Windows vendored builds require a suitable MSVC native toolchain and OpenSSL configure prerequisites such as Perl. macOS needs Xcode Command Line Tools and normal signing/notarization.
- Preserve relevant Rust package notices, SQLCipher Community's BSD-style license and OpenSSL Apache 2.0 notices where distributed. Validate notices and actual linked artifacts per platform. This is Community, without FIPS certification claims. The unrelated repository-relative Python/model path remains a standalone installer blocker until packaged.

Before a release, run native credential and installer smoke tests, decide rollback/backup and obsolete-generation/Keychain retention, and review the DOCX issues separately. See [architecture](ARCHITECTURE.md), [migration](MIGRATION.md), [security](SECURITY.md), [test results](TEST_RESULTS.md) and [performance](PERFORMANCE.md). [Tauri distribution](https://v2.tauri.app/distribute/) · [SQLCipher source and license](https://github.com/sqlcipher/sqlcipher) · [OpenSSL license](https://www.openssl.org/source/license.html)
