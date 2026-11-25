# Initial relational model

```mermaid
erDiagram
    PROJECTS ||--o{ PSEUDONYM_MAPPINGS : scopes
    PROJECTS ||--o{ PROCESSING_HISTORY : scopes
    PROJECTS ||--o{ DETECTION_RULES : scopes
    PII_CATEGORIES ||--o{ PSEUDONYM_MAPPINGS : classifies
    PII_CATEGORIES ||--o{ DETECTION_RULES : classifies
    PII_CATEGORIES ||--o| PSEUDONYM_COUNTERS : reserves
    PROJECTS {
        TEXT project_id PK
        TEXT name
        TEXT name_key UK
        INTEGER position UK
    }
    PII_CATEGORIES {
        TEXT code PK
        TEXT token_prefix UK
    }
    PSEUDONYM_MAPPINGS {
        TEXT mapping_id PK
        TEXT project_id FK "part of two scoped unique keys"
        TEXT category_code FK "unique with project_id and normalized_value"
        TEXT original_value
        TEXT normalized_value "unique with project_id and category_code"
        TEXT pseudonym UK "Vault-wide"
        INTEGER position "unique with project_id"
    }
    PROCESSING_HISTORY {
        TEXT record_id PK
        TEXT project_id FK "unique with position"
        TEXT mode
        TEXT processed_text
        TEXT original_text "nullable"
        INTEGER replacement_count
        INTEGER position "unique with project_id"
    }
    DETECTION_RULES {
        TEXT project_id PK,FK "composite PK with position"
        INTEGER position PK "composite PK with project_id"
        TEXT phrase
        TEXT category_code FK
    }
    PSEUDONYM_COUNTERS {
        TEXT category_code PK,FK
        INTEGER high_water
    }
```

Project foreign keys cascade on deletion. A scoped mapping can recur across many documents, but history has no recorded occurrence association with mappings. See [schema](SCHEMA.md) for composite constraints and validation.
