# SQLCipher schema version 1

The executable migration is [001_initial.sql](../../apps/desktop/src-tauri/src/database/migrations/001_initial.sql). A fresh keyed connection creates and seeds all six STRICT tables in one transaction. It writes `application_id=0x5645494c` and `user_version=1` in that transaction. Opening requires the current application ID, supported version, known tables and the General row. A future upgrade must register a version transition with its own tested transaction. An existing unsupported or unidentifiable database is rejected. The SQL schema version is independent of the bootstrap and cipher compatibility formats.

All columns are non-null, except `processing_history.original_text`. Uniqueness is case-sensitive on stored SQL strings. Rust computes the normalized keys for comparison.

| Table | Column | SQLite type and constraint |
| --- | --- | --- |
| projects | project_id | TEXT PK. General uses `default`. Named projects use UUIDs. |
| | name | TEXT. Rust enforces 1–80 Unicode scalar characters after trimming and SQL enforces 1–320 UTF-8 bytes. |
| | name_key | TEXT unique and nonempty. Lowercase display name per Rust normalization. General case-insensitive name reserved. |
| | position | INTEGER unique and nonnegative. General has position zero. Named projects have positive positions. |
| pii_categories | code | TEXT PK. Entries seeded by the versioned SQL migration. |
| | token_prefix | TEXT unique. |
| pseudonym_mappings | mapping_id | TEXT PK, UUID. |
| | project_id | TEXT FK → projects, ON DELETE CASCADE. |
| | category_code | TEXT FK → pii_categories. |
| | original_value | TEXT, 1–10,000 UTF-8 bytes. First trimmed representation retained. |
| | normalized_value | TEXT nonempty. A derived index key, never exposed for independent editing. |
| | pseudonym | TEXT globally unique. |
| | position | INTEGER nonnegative. Unique with project_id. |
| processing_history | record_id | TEXT PK, UUID. |
| | project_id | TEXT FK → projects, ON DELETE CASCADE. |
| | mode | TEXT restricted to `pseudonymize` or historical `anonymize`. |
| | processed_text | TEXT, exact historical output. |
| | original_text | Nullable TEXT. Non-null is limited to pseudonymize and at most 500,000 UTF-8 bytes. |
| | replacement_count | INTEGER, 1–500,000. Counts approved spans. |
| | position | INTEGER nonnegative, unique with project_id. Lists return descending positions. |
| detection_rules | project_id | TEXT FK → projects, ON DELETE CASCADE. Part of composite PK. |
| | position | INTEGER 0–199. Part of composite PK. |
| | phrase | TEXT 1–500 UTF-8 bytes. Rust rejects whitespace-only phrases. |
| | category_code | TEXT FK → pii_categories. |
| pseudonym_counters | category_code | TEXT PK and FK → pii_categories. |
| | high_water | INTEGER nonnegative. Preserves assigned and skipped numbers even after deletes. |

Triggers protect General and cap the Vault at 100 named projects and 100 history rows per project. Every connection enables foreign keys. Unique keys and PKs already provide indexes for scoped positions, category/value lookup, record IDs and token uniqueness. No separate index duplicates those access paths. Categories form a tiny reference catalog and current operations do not search rules by category alone. The catalog and Rust, TypeScript and Python category contracts are compared by a regression test.

`(project_id, category_code, normalized_value)` is unique. PERSON, LOCATION and ORGANIZATION fold Unicode whitespace to single spaces and use Rust lowercase. Other categories trim outer whitespace and keep internal whitespace/case. Unicode canonical normalization and email-specific rules would change existing behavior. Global counter increment is checked. A fresh candidate is skipped if its exact token occurs in source text. An existing candidate is reused even if it already appears literally in the document, as before.

### Synthetic examples

- Project `default`, name `General`, key `general`, position 0.
- Category `PERSON`, prefix `PERSON`. A PERSON counter of 7 holds even if mapping 7 has been deleted.
- Mapping with UUID, project `default`, category `PERSON`, original `Avery  Quill`, normalized `avery quill`, token `PERSON_007`, position 0.
- Pseudonymized history with UUID, project `default`, output `Welcome PERSON_007`, original `Welcome Avery  Quill`, count 1, position 0.
- Rule `(default, 0, "internal project phrase", INDIRECT_IDENTIFIER)`. An identical rule at position 1 is allowed.

Explicit positions preserve relative ordering. After deletion the next position grows from the last remaining one. Position values have no external meaning and can be reused at the end. Token suffixes have separate high-water state and are not recycled by these operations.

The legacy serialized-envelope size limit remains a guard on import. SQLCipher has no whole-Vault JSON envelope limit. Existing per-project, history, phrase, span and mapping bounds still apply, with storage additionally bounded by local disk.
