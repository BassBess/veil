# Baseline and verification

Baseline commit `af89a997707dcdb69a0843d595a18099b7f2713f`, confirmed against origin main. The same x86_64 NixOS shell ran the baseline from a committed-source archive and the implementation from the feature branch. It supplied Rust/Cargo 1.99.0, Node 22.23.3 and Python 3.12.15. Node modules, the Python venv and installed language models were reused. The SQLCipher compatibility experiment obtained the new Cargo dependencies before the production Vault write path changed. No actual user Vault was accessed or migrated.

| Check | Baseline | Implementation |
| --- | --- | --- |
| Rust default | 54 passed, 0 failed, 7 ignored | 66 passed, 0 failed, 8 ignored including the explicitly selected benchmark. Binary and doc-test runners contain zero tests. |
| Rust optional engine tests | 6 passed, 1 failed due to missing local Arabic model | 6 passed, 1 failed for the same missing model. The benchmark is selected separately. |
| Python discovery | 69 tests, 67 passed, 0 skipped, 2 errors | 69 tests, 67 passed, 0 skipped, 2 errors. Both rounds lack local Arabic Stanza model data. |
| Frontend Node | 20 passed, 0 failed or skipped | 20 passed, 0 failed or skipped. |
| TypeScript and Vite | Passed | Passed, Vite 8.3.5 and TypeScript via the build script. |
| ESLint | Passed | Passed. |
| Synthetic persistence benchmark | Standalone comparator | Explicit benchmark passed. See [performance](PERFORMANCE.md). |

The Rust optional multilingual end-to-end test failed at the Arabic model load. Python errors are `test_arabic_diacritics_and_digits` and the Arabic subtest of `test_real_models_offline`. English, French and Spanish ran. Early integration compilation caught direct `usize` binding and a Rust match-guard error, both corrected. The first targeted database run found a SQLCipher PRAGMA result represented as text. The corrected targeted run passed all nine tests then present.

## Reproduce

From the repository root, enter `nix develop --offline --no-write-lock-file` using a prepared checkout with the dependencies cached. The investigation redirected Cargo build outputs using `--target-dir /tmp/veil-investigation-work/rust-target`. Production test and example invocations used the same Cargo registry prepared during the isolated experiment. Commands below omit those optional cache locations and log redirections.

```sh
cd apps/desktop
node --test tests/*.test.mjs
npm run lint
npm run build
cd src-tauri
cargo build --locked --offline
cargo test --locked --offline -- --test-threads=4
cargo test --locked --offline -- --ignored --skip compare_legacy_and_sqlcipher
cd ../../../engines/python
PYTHONDONTWRITEBYTECODE=1 .venv/bin/python -m unittest discover -s tests -v
```

Baseline ran `node --test tests/*.test.mjs`, `npm run build`, `npm run lint`, `cargo test --locked --offline`, `cargo test --locked --offline -- --ignored` and `.venv/bin/python -m unittest discover -s tests -v` from corresponding directories in the isolated archive.

### Cipher experiment and independent SQLite check

Under `apps/desktop/src-tauri`:

```sh
veil_probe_dir=$(mktemp -d)
cargo run --locked --offline --example cipher_probe -- "$veil_probe_dir/probe.db"
python3 - "$veil_probe_dir/probe.db" <<'PY'
import sqlite3
import sys
from pathlib import Path

try:
    connection = sqlite3.connect(Path(sys.argv[1]).as_uri() + "?mode=ro", uri=True)
    connection.execute("SELECT text FROM probe").fetchall()
except sqlite3.DatabaseError as error:
    print(f"Ordinary SQLite {sqlite3.sqlite_version} rejected the file: {error}")
else:
    raise SystemExit("Unexpected plaintext access")
PY
```

The original probe ran against an isolated checkout with the complete Tauri/Rust dependency graph. It compiled rusqlite 0.43.0 with `bundled-sqlcipher-vendored-openssl`. Repeating on the branch reported SQLCipher **4.20.0 community**, SQLite **3.54.1**, crypto provider **OpenSSL 3.6.3**. The separately invoked Python ordinary SQLite **3.51.2** reader failed with `file is not a database`. Tested correct, wrong and absent key access, synthetic canary concealment and transactional rollback. Foreign keys rejected an invalid reference and the experiment verified memory temp, DELETE journal and explicit sync settings. The production tests additionally checked canaries in a live rollback journal, no WAL/SHM, TEMP table materialization without a filesystem filename, explicit schema identity, corruption and missing bootstrap rejection.

## New checks

- Project ordering, reserved General protection including SQL replacement attempts, relational FKs, category contract across Rust/Python/TypeScript/SQL, token and scoped mapping uniqueness, project cascade and counter preservation.
- Unicode, Arabic and case behavior independent of the detector. Exact restore, mapping/history independence, duplicate ordered rules, reset, source-token collision handling, repeat reuse and concurrent mapping submissions.
- Transaction rollback after the first mapping succeeds and a later mapping fails. UUID retry rejection and per-project history capacity.
- Encrypted legacy passphrase migration, exact recovery bytes, scoped IDs, ordered history, non-restorable originals, stale child counters, skipped and reserved global values, interrupted staging retry and repeated open. Negative tests cover duplicate payloads, incorrect legacy credentials and failed marker replacement.
- Credential changes verify failed short passphrase, successful reopen with the new passphrase, old passphrase rejection for the active marker, and generation-binding tamper rejection.
- Windows/macOS-specific account fixtures exercise v2/v3 storage and a mode-binding mismatch when run on those platforms. On Windows, the fixture also rewrites the key field using the legacy alias and omits the platform. Existing Windows verification-result tests remain. These native tests were cfg-excluded on Linux. Native cancellation and installer behavior require actual Windows/macOS runs.

## Linux binary linkage

After the native NixOS debug build, `readelf -d <target>/debug/app` showed no direct dynamic SQLite or OpenSSL dependency. WebKitGTK and related shared libraries still load their ordinary system SQLite transitively. Before symbol isolation, `nm -D <target>/debug/app` showed bundled SQLCipher's `sqlite3_open_v2` and `sqlite3_prepare_v2` exported, creating an interposition risk. The Linux linker map now localizes SQLite/SQLCipher symbols in the app executable. After rebuilding, `nm -D <target>/debug/app | rg ' [TW] (sqlite3|sqlcipher)'` found no exports. `nm <target>/debug/app | rg ' [Tt] sqlite3_(open_v2|key)$'` found both local bundled symbols. The Linux native build and default Rust suite were rerun after this change.

No full GUI interaction, Windows/MSVC compilation, macOS compilation, live Keychain/DPAPI integration or power-loss simulation was performed. File-failure tests use synthetic blocked paths rather than mounting a filesystem to induce disk exhaustion. Recovery artifacts are confined to temporary profiles.
