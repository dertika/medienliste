-- Medientyp „series“ (TV-Serien) erlauben.
-- SQLite kann CHECK-Constraints nicht ändern, daher wird `media` neu aufgebaut.
-- Die Migration läuft in einer Transaktion mit aktiven Fremdschlüsseln; ein
-- DROP von `media` würde `user_media` per ON DELETE CASCADE leeren. Deshalb
-- werden die persönlichen Einträge vorher gesichert und danach zurückgeschrieben.

CREATE TEMP TABLE user_media_backup AS SELECT * FROM user_media;
DROP TABLE user_media;

CREATE TABLE media_new (
    id               INTEGER PRIMARY KEY,
    media_type       TEXT    NOT NULL CHECK (media_type IN ('book', 'audiobook', 'movie', 'series')),
    title            TEXT    NOT NULL,
    subtitle         TEXT,
    authors          TEXT    NOT NULL DEFAULT '[]',  -- JSON-Array: Autor(en), Regie bzw. Idee
    narrators        TEXT    NOT NULL DEFAULT '[]',  -- JSON-Array: Sprecher bzw. Darsteller
    description      TEXT,
    isbn10           TEXT,
    isbn13           TEXT,
    asin             TEXT,
    published_year   INTEGER,
    series           TEXT,
    series_index     TEXT,
    language         TEXT,
    publisher        TEXT,
    cover_file       TEXT,
    cover_source_url TEXT,
    source           TEXT    NOT NULL,
    external_id      TEXT,
    search_text      TEXT    NOT NULL DEFAULT '',
    created_by       INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at       TEXT    NOT NULL,
    updated_at       TEXT    NOT NULL
);

INSERT INTO media_new (id, media_type, title, subtitle, authors, narrators, description, isbn10, isbn13,
    asin, published_year, series, series_index, language, publisher, cover_file, cover_source_url, source,
    external_id, search_text, created_by, created_at, updated_at)
SELECT id, media_type, title, subtitle, authors, narrators, description, isbn10, isbn13,
    asin, published_year, series, series_index, language, publisher, cover_file, cover_source_url, source,
    external_id, search_text, created_by, created_at, updated_at
FROM media;

DROP TABLE media;
ALTER TABLE media_new RENAME TO media;

CREATE UNIQUE INDEX media_source_external_id ON media(source, external_id) WHERE external_id IS NOT NULL;
CREATE INDEX media_isbn13 ON media(isbn13) WHERE isbn13 IS NOT NULL;
CREATE INDEX media_asin ON media(asin) WHERE asin IS NOT NULL;

CREATE TABLE user_media (
    user_id           INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    media_id          INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    status            TEXT    NOT NULL CHECK (status IN ('done', 'wanted')),
    status_changed_at TEXT    NOT NULL,
    added_at          TEXT    NOT NULL,
    done_year         INTEGER,
    PRIMARY KEY (user_id, media_id)
) WITHOUT ROWID;
INSERT INTO user_media (user_id, media_id, status, status_changed_at, added_at, done_year)
SELECT user_id, media_id, status, status_changed_at, added_at, done_year FROM user_media_backup;
DROP TABLE user_media_backup;
CREATE INDEX user_media_user_status ON user_media(user_id, status, status_changed_at);
CREATE INDEX user_media_media_id ON user_media(media_id);
