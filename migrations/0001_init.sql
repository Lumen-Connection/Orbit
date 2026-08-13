CREATE TABLE IF NOT EXISTS schema_migrations (
    version     INTEGER PRIMARY KEY,
    applied_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS project (
    id                TEXT PRIMARY KEY,
    name              TEXT NOT NULL,
    canonical_root    TEXT NOT NULL UNIQUE,
    created_at        TEXT NOT NULL,
    last_opened_at    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS session (
    id                TEXT PRIMARY KEY,
    project_id        TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    label             TEXT NOT NULL,
    model_id          TEXT NOT NULL,
    role              TEXT,
    created_at        TEXT NOT NULL,
    last_active_at    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS message (
    id                TEXT PRIMARY KEY,
    session_id        TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    seq               INTEGER NOT NULL,
    role              TEXT NOT NULL,
    content           TEXT NOT NULL,
    tool_calls_json   TEXT,
    created_at        TEXT NOT NULL,
    UNIQUE (session_id, seq)
);

CREATE TABLE IF NOT EXISTS tool_call (
    id                TEXT PRIMARY KEY,
    session_id        TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    message_id        TEXT NOT NULL REFERENCES message(id) ON DELETE CASCADE,
    tool_name         TEXT NOT NULL,
    arguments_json    TEXT NOT NULL,
    status            TEXT NOT NULL,
    output            TEXT,
    duration_ms       INTEGER,
    created_at        TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS file_change (
    id                TEXT PRIMARY KEY,
    project_id        TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    session_id        TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    relative_path     TEXT NOT NULL,
    original_hash     TEXT NOT NULL,
    unified_diff      TEXT NOT NULL,
    original_content  TEXT NOT NULL DEFAULT '',
    proposed_content  TEXT NOT NULL DEFAULT '',
    status            TEXT NOT NULL,
    created_at        TEXT NOT NULL,
    applied_at        TEXT
);

CREATE TABLE IF NOT EXISTS usage_record (
    id                TEXT PRIMARY KEY,
    session_id        TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    model_id          TEXT NOT NULL,
    input_tokens      INTEGER NOT NULL,
    output_tokens     INTEGER NOT NULL,
    cached_tokens     INTEGER NOT NULL DEFAULT 0,
    estimated_cost    REAL NOT NULL,
    latency_ms        INTEGER,
    created_at        TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_message_session_seq ON message (session_id, seq);
CREATE INDEX IF NOT EXISTS idx_usage_session ON usage_record (session_id);
CREATE INDEX IF NOT EXISTS idx_change_project ON file_change (project_id, status);
CREATE INDEX IF NOT EXISTS idx_session_project ON session (project_id);
