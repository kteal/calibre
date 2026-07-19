use crate::Result;
use crate::library::{CALIBRE_APPLICATION_ID, SUPPORTED_SCHEMA_VERSION, database_error};
use rusqlite::{Connection, params};
use std::path::Path;

pub(crate) fn initialize(connection: &mut Connection, database: &Path) -> Result<()> {
    connection
        .pragma_update(None, "application_id", CALIBRE_APPLICATION_ID)
        .map_err(|error| database_error("set Calibre application ID", database, error))?;
    connection
        .pragma_update(None, "user_version", SUPPORTED_SCHEMA_VERSION)
        .map_err(|error| database_error("set Calibre schema version", database, error))?;
    let transaction = connection
        .transaction()
        .map_err(|error| database_error("begin library initialization", database, error))?;
    transaction
        .execute_batch(SCHEMA)
        .map_err(|error| database_error("create schema-27 library", database, error))?;
    transaction
        .execute(
            "INSERT INTO library_id(uuid) VALUES (?1)",
            [uuid::Uuid::new_v4().to_string()],
        )
        .map_err(|error| database_error("initialize library identity", database, error))?;
    for (key, value) in [
        ("bools_are_tristate", "true"),
        ("user_categories", "{}"),
        ("saved_searches", "{}"),
        ("grouped_search_terms", "{}"),
    ] {
        transaction
            .execute(
                "INSERT INTO preferences(key, val) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(|error| database_error("initialize library preferences", database, error))?;
    }
    transaction
        .commit()
        .map_err(|error| database_error("commit library initialization", database, error))
}

// This schema is independently authored from public format information and
// black-box object/column observations. It intentionally contains no Calibre
// or Citadel schema SQL, view definition, trigger body, or migration.
const SCHEMA: &str = r"
CREATE TABLE books (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL DEFAULT 'Unknown' COLLATE NOCASE,
    sort TEXT COLLATE NOCASE,
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    pubdate TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    series_index REAL NOT NULL DEFAULT 1.0,
    author_sort TEXT COLLATE NOCASE,
    path TEXT NOT NULL DEFAULT '',
    uuid TEXT,
    has_cover BOOL DEFAULT 0,
    last_modified TIMESTAMP NOT NULL DEFAULT '2000-01-01 00:00:00+00:00'
);
CREATE TABLE authors (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    sort TEXT COLLATE NOCASE,
    link TEXT NOT NULL DEFAULT ''
);
CREATE TABLE tags (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    link TEXT NOT NULL DEFAULT ''
);
CREATE TABLE series (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    sort TEXT COLLATE NOCASE,
    link TEXT NOT NULL DEFAULT ''
);
CREATE TABLE publishers (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    sort TEXT COLLATE NOCASE,
    link TEXT NOT NULL DEFAULT ''
);
CREATE TABLE languages (
    id INTEGER PRIMARY KEY,
    lang_code TEXT NOT NULL COLLATE NOCASE UNIQUE,
    link TEXT NOT NULL DEFAULT ''
);
CREATE TABLE ratings (
    id INTEGER PRIMARY KEY,
    rating INTEGER UNIQUE,
    link TEXT NOT NULL DEFAULT ''
);
CREATE TABLE comments (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL UNIQUE,
    text TEXT NOT NULL
);
CREATE TABLE identifiers (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    type TEXT NOT NULL DEFAULT 'isbn' COLLATE NOCASE,
    val TEXT NOT NULL,
    UNIQUE(book, type)
);
CREATE TABLE data (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    format TEXT NOT NULL COLLATE NOCASE,
    uncompressed_size INTEGER NOT NULL,
    name TEXT NOT NULL,
    UNIQUE(book, format)
);
CREATE TABLE books_authors_link (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    author INTEGER NOT NULL,
    UNIQUE(book, author)
);
CREATE TABLE books_tags_link (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    tag INTEGER NOT NULL,
    UNIQUE(book, tag)
);
CREATE TABLE books_series_link (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL UNIQUE,
    series INTEGER NOT NULL
);
CREATE TABLE books_publishers_link (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL UNIQUE,
    publisher INTEGER NOT NULL
);
CREATE TABLE books_languages_link (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    lang_code INTEGER NOT NULL,
    item_order INTEGER NOT NULL DEFAULT 0,
    UNIQUE(book, lang_code)
);
CREATE TABLE books_ratings_link (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    rating INTEGER NOT NULL,
    UNIQUE(book, rating)
);
CREATE TABLE metadata_dirtied (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL UNIQUE
);
CREATE TABLE annotations_dirtied (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL UNIQUE
);
CREATE TABLE books_pages_link (
    book INTEGER PRIMARY KEY,
    pages INTEGER NOT NULL DEFAULT 0,
    algorithm INTEGER NOT NULL DEFAULT 0,
    format TEXT NOT NULL DEFAULT '',
    format_size INTEGER NOT NULL DEFAULT 0,
    timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    needs_scan INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE custom_columns (
    id INTEGER PRIMARY KEY,
    label TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    datatype TEXT NOT NULL,
    mark_for_delete BOOL NOT NULL DEFAULT 0,
    editable BOOL NOT NULL DEFAULT 1,
    display TEXT NOT NULL DEFAULT '{}',
    is_multiple BOOL NOT NULL DEFAULT 0,
    normalized BOOL NOT NULL
);
CREATE TABLE preferences (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL UNIQUE,
    val TEXT NOT NULL
);
CREATE TABLE library_id (
    id INTEGER PRIMARY KEY,
    uuid TEXT NOT NULL UNIQUE
);
CREATE TABLE feeds (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL UNIQUE,
    script TEXT NOT NULL
);
CREATE TABLE conversion_options (
    id INTEGER PRIMARY KEY,
    format TEXT NOT NULL COLLATE NOCASE,
    book INTEGER,
    data BLOB NOT NULL,
    UNIQUE(format, book)
);
CREATE TABLE books_plugin_data (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    name TEXT NOT NULL,
    val TEXT NOT NULL,
    UNIQUE(book, name)
);
CREATE TABLE last_read_positions (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    format TEXT NOT NULL COLLATE NOCASE,
    user TEXT NOT NULL,
    device TEXT NOT NULL,
    cfi TEXT NOT NULL,
    epoch REAL NOT NULL,
    pos_frac REAL NOT NULL DEFAULT 0,
    UNIQUE(user, device, book, format)
);
CREATE TABLE annotations (
    id INTEGER PRIMARY KEY,
    book INTEGER NOT NULL,
    format TEXT NOT NULL COLLATE NOCASE,
    user_type TEXT NOT NULL,
    user TEXT NOT NULL,
    timestamp REAL NOT NULL,
    annot_id TEXT NOT NULL,
    annot_type TEXT NOT NULL,
    annot_data TEXT NOT NULL,
    searchable_text TEXT NOT NULL DEFAULT '',
    UNIQUE(book, user_type, user, format, annot_type, annot_id)
);

CREATE INDEX books_idx ON books(sort);
CREATE INDEX authors_idx ON books(author_sort);
CREATE INDEX publishers_idx ON publishers(name);
CREATE INDEX series_idx ON series(name);
CREATE INDEX tags_idx ON tags(name);
CREATE INDEX languages_idx ON languages(lang_code);
CREATE INDEX comments_idx ON comments(book);
CREATE INDEX data_idx ON data(book);
CREATE INDEX formats_idx ON data(format);
CREATE INDEX custom_columns_idx ON custom_columns(label);
CREATE INDEX conversion_options_idx_a ON conversion_options(format);
CREATE INDEX conversion_options_idx_b ON conversion_options(book);
CREATE INDEX lrp_idx ON last_read_positions(book);
CREATE INDEX annot_idx ON annotations(book);
CREATE INDEX books_pages_link_pidx ON books_pages_link(needs_scan);
CREATE INDEX books_authors_link_aidx ON books_authors_link(author);
CREATE INDEX books_authors_link_bidx ON books_authors_link(book);
CREATE INDEX books_tags_link_aidx ON books_tags_link(tag);
CREATE INDEX books_tags_link_bidx ON books_tags_link(book);
CREATE INDEX books_series_link_aidx ON books_series_link(series);
CREATE INDEX books_series_link_bidx ON books_series_link(book);
CREATE INDEX books_publishers_link_aidx ON books_publishers_link(publisher);
CREATE INDEX books_publishers_link_bidx ON books_publishers_link(book);
CREATE INDEX books_languages_link_aidx ON books_languages_link(lang_code);
CREATE INDEX books_languages_link_bidx ON books_languages_link(book);
CREATE INDEX books_ratings_link_aidx ON books_ratings_link(rating);
CREATE INDEX books_ratings_link_bidx ON books_ratings_link(book);

CREATE TRIGGER books_insert_trg AFTER INSERT ON books
WHEN NEW.uuid IS NULL OR NEW.sort IS NULL
BEGIN
    UPDATE books
    SET uuid = COALESCE(NEW.uuid, uuid4()),
        sort = COALESCE(NEW.sort, title_sort(NEW.title))
    WHERE id = NEW.id;
END;
CREATE TRIGGER books_update_trg AFTER UPDATE OF title ON books
WHEN NEW.title IS NOT OLD.title
BEGIN
    UPDATE books SET sort = title_sort(NEW.title) WHERE id = NEW.id;
END;
CREATE TRIGGER series_insert_trg AFTER INSERT ON series
WHEN NEW.sort IS NULL
BEGIN
    UPDATE series SET sort = title_sort(NEW.name) WHERE id = NEW.id;
END;
CREATE TRIGGER series_update_trg AFTER UPDATE OF name ON series
WHEN NEW.name IS NOT OLD.name
BEGIN
    UPDATE series SET sort = title_sort(NEW.name) WHERE id = NEW.id;
END;
CREATE TRIGGER books_pages_link_create_trigger AFTER INSERT ON books
BEGIN
    INSERT OR IGNORE INTO books_pages_link(book, needs_scan) VALUES (NEW.id, 1);
END;
CREATE TRIGGER books_delete_trg AFTER DELETE ON books
BEGIN
    DELETE FROM books_authors_link WHERE book = OLD.id;
    DELETE FROM books_tags_link WHERE book = OLD.id;
    DELETE FROM books_series_link WHERE book = OLD.id;
    DELETE FROM books_publishers_link WHERE book = OLD.id;
    DELETE FROM books_languages_link WHERE book = OLD.id;
    DELETE FROM books_ratings_link WHERE book = OLD.id;
    DELETE FROM comments WHERE book = OLD.id;
    DELETE FROM identifiers WHERE book = OLD.id;
    DELETE FROM data WHERE book = OLD.id;
    DELETE FROM metadata_dirtied WHERE book = OLD.id;
    DELETE FROM annotations_dirtied WHERE book = OLD.id;
    DELETE FROM books_pages_link WHERE book = OLD.id;
    DELETE FROM conversion_options WHERE book = OLD.id;
    DELETE FROM books_plugin_data WHERE book = OLD.id;
    DELETE FROM last_read_positions WHERE book = OLD.id;
    DELETE FROM annotations WHERE book = OLD.id;
END;
";
