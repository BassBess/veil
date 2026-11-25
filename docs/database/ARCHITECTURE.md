# Veil Vault architecture

React calls typed Tauri commands. Rust validates spans, manages a single Vault session, and performs protection and restoration. A Rust repository issues parameterized SQL through rusqlite and SQLCipher. Python remains a subprocess for detection, import and export. Its JSON inputs can carry source text and detection rules, so the Rust/Python process boundary is inside the trusted application. Python gains no database handle, password or key. WebView import and detection-category preferences retain their existing localStorage names.

## Storage decision

The previous format encrypted one JSON snapshot using AES-256-GCM. Every mutation cloned, encoded, encrypted and replaced the whole Vault. This format is supported as a read-only migration source in production. Its writer is compiled only for synthetic fixtures and benchmarks.

| Storage | Design implications |
| --- | --- |
| Encrypted JSON | Single authenticated file. Whole-document save and in-memory scans. No relational constraints. |
| Ordinary SQLite | Transactions and indexes. Original values, indexes and ordinary journal pages would be readable at rest. |
| Ordinary SQLite with field AEAD | Could hide selected values. Equality indexes, relation metadata, nonce management and rotation would need separate design. |
| SQLCipher | SQLite transactions and constraints with encrypted database pages and encrypted page content in rollback journals. Native build and credential bootstrap add complexity. |

[SQLCipher design](https://www.zetetic.net/sqlcipher/design/) · [rusqlite build features](https://github.com/rusqlite/rusqlite/blob/master/README.md)

## Module boundaries

- `database/connection.rs` configures a keyed SQLCipher connection, applies supported schema migrations and validates integrity.
- `database/bootstrap.rs` holds a versioned credential wrapper. `database/mod.rs` manages the current generation, exclusive lock, staging and publication.
- `database/repository.rs` scopes all reads and writes by project and uses bound parameters. `database/legacy_migration.rs` validates and transfers legacy rows. `vault_storage.rs` retains the authenticated legacy reader.
- `protection.rs` holds application rules, session commands, token formatting, review-to-output transformation, and a temporary mapping set for anonymization. There is no database transaction in anonymization, Python detection, or document export.

## Session and transactions

The app state owns a mutex around the unlocked Vault and its connection. A sibling `identities.lock` is held across the connection's lifetime and shares the legacy writer's lock protocol. No connection or statement is returned through IPC. Lock drops the keyed connection and owned key before releasing the file lock. Session epochs invalidate pending native-auth transitions and rule-backed detection responses after a lock, reset, or change of credentials. No transaction spans an OS dialog or Python request. Frontend epochs discard late Vault-derived UI results and clear cached history, identities, rules and restored text on lock. The open working document stays available for independent anonymization.

Pseudonymization validates request spans and supported categories, then opens a serialized immediate transaction. It checks scope, request identity and capacity, looks up or allocates mappings, advances high-water values and stores a history snapshot. The UI sends an operation UUID. A repeated UUID targeting an existing history row is rejected before allocation. Separate user submissions have separate UUIDs and may legitimately reproduce the same output. The history row's identifier fulfills this purpose without tracking submissions in a separate table. No successful result is returned before commit. Rule replacement, named project creation/deletion, reset and mapping creation use explicit transactions or one atomic SQL statement.

These six tables preserve current data relationships. Projects share an unlock boundary and token namespace. A project is a logical scope within one local Vault. The application keeps no account, document catalog, retained review queue, or history-to-mapping provenance. See [schema](SCHEMA.md), [migration](MIGRATION.md) and [security](SECURITY.md).
