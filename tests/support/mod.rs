#![allow(dead_code)]

use rusqlite::Connection;
use std::path::Path;

pub struct TestLibrary {
    directory: tempfile::TempDir,
}

impl TestLibrary {
    pub fn new() -> Self {
        Self::with_version(27)
    }

    pub fn with_version(version: u32) -> Self {
        let directory = tempfile::tempdir().expect("create disposable library");
        create_schema(directory.path(), version);
        Self { directory }
    }

    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    pub fn database(&self) -> std::path::PathBuf {
        self.path().join("metadata.db")
    }
}

#[allow(clippy::too_many_lines)] // Keeping the synthetic schema in one batch makes it auditable.
fn create_schema(root: &Path, version: u32) {
    let connection = Connection::open(root.join("metadata.db")).expect("create metadata.db");
    connection
        .execute_batch(&format!(
            "
            PRAGMA application_id = 1667329129;
            PRAGMA user_version = {version};

            CREATE TABLE books (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL DEFAULT 'Unknown' COLLATE NOCASE,
                sort TEXT COLLATE NOCASE,
                timestamp TEXT DEFAULT CURRENT_TIMESTAMP,
                pubdate TEXT DEFAULT CURRENT_TIMESTAMP,
                series_index REAL NOT NULL DEFAULT 1.0,
                author_sort TEXT COLLATE NOCASE,
                path TEXT NOT NULL DEFAULT '',
                uuid TEXT,
                has_cover INTEGER DEFAULT 0,
                last_modified TEXT NOT NULL DEFAULT '2000-01-01 00:00:00+00:00'
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
                type TEXT NOT NULL COLLATE NOCASE,
                val TEXT NOT NULL COLLATE NOCASE,
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
            CREATE TABLE books_pages_link (
                book INTEGER PRIMARY KEY,
                pages INTEGER,
                needs_scan INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE custom_columns (
                id INTEGER PRIMARY KEY,
                label TEXT NOT NULL,
                name TEXT NOT NULL,
                datatype TEXT NOT NULL,
                is_multiple INTEGER NOT NULL DEFAULT 0,
                editable INTEGER NOT NULL DEFAULT 1,
                display TEXT NOT NULL DEFAULT '{{}}',
                normalized INTEGER NOT NULL DEFAULT 0,
                mark_for_delete INTEGER NOT NULL DEFAULT 0
            );
            "
        ))
        .expect("create independently authored minimal test schema");
}
