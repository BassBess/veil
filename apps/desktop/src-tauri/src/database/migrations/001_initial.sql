CREATE TABLE projects (
    project_id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(CAST(name AS BLOB)) BETWEEN 1 AND 320),
    name_key TEXT NOT NULL UNIQUE CHECK (length(CAST(name_key AS BLOB)) > 0),
    position INTEGER NOT NULL UNIQUE CHECK (position >= 0),
    CHECK (
        (project_id = 'default' AND name = 'General' AND name_key = 'general' AND position = 0)
        OR (project_id <> 'default' AND name_key <> 'general' AND position > 0)
    )
) STRICT;

CREATE TRIGGER prevent_general_delete
BEFORE DELETE ON projects WHEN OLD.project_id = 'default'
BEGIN
    SELECT RAISE(ABORT, 'General cannot be deleted');
END;

CREATE TRIGGER prevent_general_update
BEFORE UPDATE ON projects WHEN OLD.project_id = 'default'
BEGIN
    SELECT RAISE(ABORT, 'General cannot be updated');
END;

CREATE TRIGGER limit_projects
BEFORE INSERT ON projects WHEN NEW.project_id <> 'default'
    AND (SELECT count(*) FROM projects WHERE position > 0) >= 100
BEGIN
    SELECT RAISE(ABORT, 'Project limit reached');
END;

CREATE TABLE pii_categories (
    code TEXT PRIMARY KEY,
    token_prefix TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE pseudonym_mappings (
    mapping_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    category_code TEXT NOT NULL REFERENCES pii_categories(code),
    original_value TEXT NOT NULL CHECK (length(CAST(original_value AS BLOB)) BETWEEN 1 AND 10000),
    normalized_value TEXT NOT NULL CHECK (length(CAST(normalized_value AS BLOB)) > 0),
    pseudonym TEXT NOT NULL UNIQUE,
    position INTEGER NOT NULL CHECK (position >= 0),
    UNIQUE (project_id, category_code, normalized_value),
    UNIQUE (project_id, position)
) STRICT;

CREATE TABLE processing_history (
    record_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    mode TEXT NOT NULL CHECK (mode IN ('pseudonymize', 'anonymize')),
    processed_text TEXT NOT NULL,
    original_text TEXT CHECK (original_text IS NULL OR (mode = 'pseudonymize'
        AND length(CAST(original_text AS BLOB)) <= 500000)),
    replacement_count INTEGER NOT NULL CHECK (replacement_count BETWEEN 1 AND 500000),
    position INTEGER NOT NULL CHECK (position >= 0),
    UNIQUE (project_id, position)
) STRICT;

CREATE TRIGGER limit_history
BEFORE INSERT ON processing_history
WHEN (SELECT count(*) FROM processing_history WHERE project_id = NEW.project_id) >= 100
BEGIN
    SELECT RAISE(ABORT, 'History limit reached');
END;

CREATE TABLE detection_rules (
    project_id TEXT NOT NULL REFERENCES projects(project_id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position BETWEEN 0 AND 199),
    phrase TEXT NOT NULL CHECK (length(CAST(phrase AS BLOB)) BETWEEN 1 AND 500),
    category_code TEXT NOT NULL REFERENCES pii_categories(code),
    PRIMARY KEY (project_id, position)
) STRICT;

CREATE TABLE pseudonym_counters (
    category_code TEXT PRIMARY KEY REFERENCES pii_categories(code),
    high_water INTEGER NOT NULL CHECK (high_water >= 0)
) STRICT;

INSERT INTO projects VALUES ('default', 'General', 'general', 0);
INSERT INTO pii_categories VALUES
    ('PERSON', 'PERSON'), ('EMAIL_ADDRESS', 'EMAIL'), ('PHONE_NUMBER', 'PHONE'),
    ('LOCATION', 'LOCATION'), ('ORGANIZATION', 'ORG'), ('STREET_ADDRESS', 'ADDRESS'),
    ('POSTAL_CODE', 'POSTCODE'), ('DATE_TIME', 'DATE'), ('CREDIT_CARD', 'CARD'),
    ('IBAN_CODE', 'IBAN'), ('IP_ADDRESS', 'IP'), ('URL', 'URL'),
    ('ID_NUMBER', 'ID'), ('INDIRECT_IDENTIFIER', 'DETAIL');
