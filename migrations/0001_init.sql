-- Grundschema der Medienliste.
-- Zeitstempel werden als RFC-3339-Text in UTC gespeichert.

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    sub           TEXT    NOT NULL UNIQUE,
    name          TEXT    NOT NULL DEFAULT '',
    email         TEXT    NOT NULL DEFAULT '',
    is_admin      INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT    NOT NULL,
    last_login_at TEXT    NOT NULL
);

-- Sitzungen: gespeichert wird nur der SHA-256-Hash des Cookie-Tokens.
CREATE TABLE sessions (
    token_hash TEXT    PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    csrf_token TEXT    NOT NULL,
    created_at TEXT    NOT NULL,
    expires_at TEXT    NOT NULL
);
CREATE INDEX sessions_user_id ON sessions(user_id);
CREATE INDEX sessions_expires_at ON sessions(expires_at);

-- Kurzlebiger Zustand zwischen Login-Start und OIDC-Callback.
CREATE TABLE login_states (
    state         TEXT PRIMARY KEY,
    pkce_verifier TEXT NOT NULL,
    nonce         TEXT NOT NULL,
    return_to     TEXT NOT NULL,
    created_at    TEXT NOT NULL
);

-- Medien sind global und werden zwischen Benutzern geteilt.
CREATE TABLE media (
    id               INTEGER PRIMARY KEY,
    media_type       TEXT    NOT NULL CHECK (media_type IN ('book', 'audiobook')),
    title            TEXT    NOT NULL,
    subtitle         TEXT,
    authors          TEXT    NOT NULL DEFAULT '[]',  -- JSON-Array
    narrators        TEXT    NOT NULL DEFAULT '[]',  -- JSON-Array, nur Hörbuch
    description      TEXT,
    isbn10           TEXT,
    isbn13           TEXT,
    asin             TEXT,
    published_year   INTEGER,
    series           TEXT,
    series_index     TEXT,
    language         TEXT,
    publisher        TEXT,
    cover_file       TEXT,                           -- relativ zu <data>/covers
    cover_source_url TEXT,
    source           TEXT    NOT NULL,               -- z. B. googlebooks, openlibrary, audible, manual
    external_id      TEXT,
    search_text      TEXT    NOT NULL DEFAULT '',    -- normalisiert: Titel, Autoren, Serie
    created_by       INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at       TEXT    NOT NULL,
    updated_at       TEXT    NOT NULL
);
CREATE UNIQUE INDEX media_source_external_id ON media(source, external_id) WHERE external_id IS NOT NULL;
CREATE INDEX media_isbn13 ON media(isbn13) WHERE isbn13 IS NOT NULL;
CREATE INDEX media_asin ON media(asin) WHERE asin IS NOT NULL;

-- Persönlicher Status je Benutzer und Medium (genau einer).
CREATE TABLE user_media (
    user_id           INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    media_id          INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    status            TEXT    NOT NULL CHECK (status IN ('done', 'wanted')),
    status_changed_at TEXT    NOT NULL,
    added_at          TEXT    NOT NULL,
    PRIMARY KEY (user_id, media_id)
) WITHOUT ROWID;
CREATE INDEX user_media_user_status ON user_media(user_id, status, status_changed_at);
CREATE INDEX user_media_media_id ON user_media(media_id);
