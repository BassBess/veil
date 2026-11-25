# Legacy migration and recovery

A supported existing Vault migrates on the first successful unlock. This may be automatic for a Windows or macOS v2 account Vault, as account Vaults already auto-unlocked. Authentication-required Vaults still prompt. A v1 Vault retains passphrase mode. Change modes later using Settings. Locked Linux/other platforms can use passphrase mode.

## Supported input

- Version 1 AES-256-GCM envelope with the historical Argon2id 0x13 defaults of 19,456 KiB, 2 iterations, 1 lane, 16-byte salt and a 32-byte result. New credentials use separate parameters.
- Version 2 account-protected envelope with Windows DPAPI or macOS Keychain. Accepts `windows_key` alias and defaults a missing platform to Windows.
- Version 3 account envelope using the v3-specific AAD. The app verifies native sign-in before it acquires key material.
- Legacy optional original values, structures with omitted defaulted fields, historical null-original anonymous rows, implicit General, and named projects. Exact rule order and duplicates, UUIDs, original and rendered text, record counts and category high-water values transfer. Child counter copies are non-authoritative.

Legacy anomalies are rejected without a plaintext dump or a partial import. The importer rejects invalid or duplicate IDs, bad categories, tokens inconsistent with root counters, duplicate business keys/tokens, nested child projects, unsupported modes, exceeded application limits and integers outside the supported range. Vector positions are ordered by source and checked after transfer. An inconsistency requiring repair must be handled with an explicit decision about data and token preservation.

## Publication protocol

1. Read a bootstrap or legacy descriptor. Complete required native authentication, acquire the sibling interprocess lock and recheck the descriptor under the lock.
2. Authenticate and decrypt the legacy file in memory. Create a private `identities.sqlcipher/<generation>/vault.db` using a random key. Set the initial schema and import in a transaction.
3. Verify database and foreign-key integrity and SQLCipher page authentication. Close, unwrap the stored credential and independently reopen the staged database. Verify rows and exact source/preview snapshots.
4. Save and sync an exact encrypted recovery copy at `identities.vault.legacy-recovery`. If a copy already exists, its bytes must match the source. The original source remains authoritative through this step.
5. Sync the database and generation directories as supported. Atomically replace `identities.vault` with the new bootstrap on the same filesystem. That replacement is the authority switch. Older Veil versions see an unrecognized format, and the new app uses only the named SQLCipher generation.

Wrong credentials and corruption leave source data in place. Validation or ordinary I/O failures discard the generation created by that attempt and never overwrite the legacy source. A hard termination can leave an unreferenced encrypted generation and a matching recovery copy. If the original still contains the legacy envelope, retrying the unlock stages a fresh generation. It does not import twice into the authoritative database. A missing marker with generations present is treated as damage, including interrupted first creation with no surviving marker. The app does not guess which generation should unlock. Preserve all files and recover the matching bootstrap or confirmed backup.

## Recovery and compatibility

Keep the encrypted recovery file until the team adopts a retention policy. Successful migration leaves it unchanged. Complete Vault reset changes active logical data without deleting separate backups. Account recovery files retain the legacy native dependency. A DPAPI blob or macOS reference needs the original account and accessible OS key material. No recovery password or cloud account exists.

Treat rollback as an offline, explicit operation. Close all versions of Veil, preserve the new marker and its database generation, and only restore the exact legacy recovery file to an isolated data directory with the matching older build for recovery. The older build has no model for the new database or subsequent writes. Never use old reset with the active path and never run two writable histories. No automatic downgrade, recovery assistant or merge tool is shipped.

The single-file authority replacement avoids a marker pointing at an incomplete staged database in ordinary interruption cases. On Unix, the implementation explicitly syncs generation and parent directories before the replacement and attempts parent directory sync afterward. Windows uses file sync and same-directory replacement. Durable directory metadata across Windows power loss and filesystem-specific atomicity still require native testing. Filesystems with nonstandard rename/lock semantics are outside the local-disk design.
