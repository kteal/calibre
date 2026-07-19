#![allow(missing_docs)]

mod support;

use calibre::{
    AuditIssueKind, BookFilter, BookOrder, BookQuery, BookSort, CustomColumnKind,
    CustomColumnValue, DeletionMode, Error, FormatFile, Library, NewBook, OpenOptions, Rating,
    SortDirection, UpdateBook,
};
use std::collections::BTreeMap;
use std::path::Path;

fn populated_input() -> NewBook {
    NewBook {
        title: "The Odyssey λ".into(),
        authors: vec!["Homer".into(), "Translator".into()],
        tags: vec!["classic".into(), "unicode-λ".into()],
        series: Some("Epics".into()),
        series_index: 2.5,
        publisher: Some("Test Press".into()),
        languages: vec!["eng".into(), "grc".into()],
        identifiers: BTreeMap::from([
            ("isbn".into(), "123".into()),
            ("doi".into(), "10.1/test".into()),
        ]),
        comments: Some("<p>Hello</p>".into()),
        rating: Some(Rating::new(8).expect("valid rating")),
        formats: vec![
            FormatFile::new("tests/fixtures/sample.txt"),
            FormatFile::new("tests/fixtures/sample.epub"),
        ],
        cover: Some("tests/fixtures/cover.jpg".into()),
    }
}

#[test]
fn read_only_open_does_not_create_sqlite_sidecars() {
    let fixture = support::TestLibrary::new();
    let before = std::fs::metadata(fixture.database())
        .expect("metadata")
        .modified()
        .expect("mtime");
    let library = Library::open(fixture.path()).expect("open");
    assert_eq!(library.compatibility().schema_version, 27);
    assert!(library.capabilities().read_books);
    assert!(!library.capabilities().write_books);
    assert_eq!(
        library
            .books()
            .query(BookQuery::default())
            .expect("query")
            .total,
        0
    );
    assert!(!fixture.path().join("metadata.db-wal").exists());
    assert!(!fixture.path().join("metadata.db-shm").exists());
    assert_eq!(
        before,
        std::fs::metadata(fixture.database())
            .expect("metadata")
            .modified()
            .expect("mtime")
    );
}

#[test]
fn rejects_unknown_schema_before_writing() {
    let fixture = support::TestLibrary::with_version(999);
    let error = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect_err("newer schema must be rejected");
    assert!(matches!(
        error,
        Error::UnsupportedSchema { detected: 999, .. }
    ));
}

#[test]
fn complete_book_format_cover_lifecycle() {
    let fixture = support::TestLibrary::new();
    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open writable");
    let book = library.books().add(populated_input()).expect("add book");
    assert_eq!(book.authors.len(), 2);
    assert_eq!(book.tags.len(), 2);
    assert_eq!(book.languages.len(), 2);
    assert_eq!(book.formats.len(), 2);
    assert!(book.cover_path.as_ref().is_some_and(|path| path.is_file()));
    assert_eq!(book.rating.expect("rating").get(), 8);

    let old_directory = fixture.path().join(&book.relative_path);
    let updated = library
        .books()
        .update(
            book.id,
            UpdateBook {
                title: Some("A Changed / Title".into()),
                authors: Some(vec!["Zoë Smith".into(), "二番".into()]),
                tags: Some(vec!["changed".into()]),
                series: Some(None),
                publisher: Some(None),
                languages: Some(vec!["fra".into()]),
                identifiers: Some(BTreeMap::from([("asin".into(), "X".into())])),
                comments: Some(None),
                rating: Some(None),
                ..UpdateBook::default()
            },
        )
        .expect("update");
    assert!(!old_directory.exists());
    assert!(fixture.path().join(&updated.relative_path).is_dir());
    assert_eq!(updated.authors[0].name, "Zoë Smith");
    assert_eq!(updated.tags[0].name, "changed");
    assert!(updated.series.is_none());
    assert!(updated.publisher.is_none());

    let replacement = library
        .formats()
        .replace(updated.id, "tests/fixtures/replacement.txt")
        .expect("replace txt");
    assert_eq!(replacement.format, "TXT");
    assert_eq!(
        library.formats().read(updated.id, "txt").expect("read"),
        include_bytes!("fixtures/replacement.txt")
    );
    library
        .formats()
        .remove(updated.id, "EPUB")
        .expect("remove epub");
    library
        .formats()
        .remove(updated.id, "TXT")
        .expect("remove txt");
    assert!(
        library
            .books()
            .get(updated.id)
            .expect("book survives")
            .formats
            .is_empty()
    );

    assert!(library.covers().read(updated.id).expect("cover").is_some());
    assert!(library.covers().remove(updated.id).expect("remove cover"));
    assert!(
        library
            .covers()
            .read(updated.id)
            .expect("no cover")
            .is_none()
    );

    library
        .books()
        .remove(updated.id, DeletionMode::Permanent)
        .expect("permanent removal");
    assert!(matches!(
        library.books().get(updated.id),
        Err(Error::NotFound { .. })
    ));
}

#[test]
fn add_failure_rolls_back_database_and_staged_files() {
    let fixture = support::TestLibrary::new();
    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open writable");
    let input = NewBook {
        title: "Rollback".into(),
        authors: vec!["Tester".into()],
        formats: vec![
            FormatFile::new("tests/fixtures/sample.txt"),
            FormatFile::new("tests/fixtures/does-not-exist.epub"),
        ],
        ..NewBook::default()
    };
    assert!(library.books().add(input).is_err());
    assert_eq!(
        library
            .books()
            .query(BookQuery::default())
            .expect("query")
            .total,
        0
    );
    let entries = std::fs::read_dir(fixture.path())
        .expect("read root")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, vec![std::ffi::OsString::from("metadata.db")]);
}

#[test]
fn format_replacement_failure_restores_old_bytes() {
    let fixture = support::TestLibrary::new();
    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open writable");
    let book = library
        .books()
        .add(NewBook {
            title: "Format rollback".into(),
            authors: vec!["Tester".into()],
            formats: vec![FormatFile::new("tests/fixtures/sample.txt")],
            ..NewBook::default()
        })
        .expect("add");
    let connection = rusqlite::Connection::open(fixture.database()).expect("open db");
    connection
        .execute("DROP TABLE metadata_dirtied", [])
        .expect("inject dirty-state failure");
    drop(connection);

    assert!(
        library
            .formats()
            .replace(book.id, "tests/fixtures/replacement.txt")
            .is_err()
    );
    assert_eq!(
        library.formats().read(book.id, "TXT").expect("old bytes"),
        include_bytes!("fixtures/sample.txt")
    );
}

#[test]
fn cover_replacement_failure_restores_old_bytes() {
    let fixture = support::TestLibrary::new();
    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open writable");
    let book = library
        .books()
        .add(NewBook {
            title: "Cover rollback".into(),
            authors: vec!["Tester".into()],
            cover: Some("tests/fixtures/cover.jpg".into()),
            ..NewBook::default()
        })
        .expect("add");
    let original = library
        .covers()
        .read(book.id)
        .expect("read cover")
        .expect("cover");
    let connection = rusqlite::Connection::open(fixture.database()).expect("open db");
    connection
        .execute("DROP TABLE metadata_dirtied", [])
        .expect("inject dirty-state failure");
    drop(connection);

    assert!(
        library
            .covers()
            .replace(book.id, "tests/fixtures/replacement.txt")
            .is_err()
    );
    assert_eq!(
        library
            .covers()
            .read(book.id)
            .expect("read restored cover")
            .expect("cover"),
        original
    );
}

#[test]
fn streaming_assets_round_trip_without_source_paths() {
    let fixture = support::TestLibrary::new();
    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open writable");
    let book = library
        .books()
        .add(NewBook {
            title: "Streams".into(),
            authors: vec!["Tester".into()],
            ..NewBook::default()
        })
        .expect("add");

    let mut epub_source = std::io::Cursor::new(b"streamed epub".to_vec());
    library
        .formats()
        .add_from_reader(book.id, "epub", &mut epub_source)
        .expect("stream format in");
    let mut epub_output = Vec::new();
    let size = library
        .formats()
        .write_to(book.id, "EPUB", &mut epub_output)
        .expect("stream format out");
    assert_eq!(size, 13);
    assert_eq!(epub_output, b"streamed epub");

    let mut cover_source = std::io::Cursor::new(b"streamed cover".to_vec());
    library
        .covers()
        .replace_from_reader(book.id, &mut cover_source)
        .expect("stream cover in");
    let mut cover_output = Vec::new();
    let size = library
        .covers()
        .write_to(book.id, &mut cover_output)
        .expect("stream cover out")
        .expect("cover");
    assert_eq!(size, 14);
    assert_eq!(cover_output, b"streamed cover");
}

#[test]
fn pagination_is_stable_and_concurrent_reads_work() {
    let fixture = support::TestLibrary::new();
    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open writable");
    for title in ["Gamma", "Alpha", "Beta"] {
        library
            .books()
            .add(NewBook {
                title: title.into(),
                authors: vec!["Author".into()],
                ..NewBook::default()
            })
            .expect("add");
    }
    let mut query = BookQuery::default();
    query.page.limit = 2;
    let first = library.books().query(query.clone()).expect("first");
    query.page.offset = 2;
    let second = library.books().query(query).expect("second");
    assert_eq!(first.total, 3);
    assert_eq!(first.items[0].title, "Alpha");
    assert_eq!(second.items[0].title, "Gamma");

    // Collect before joining so all read operations overlap.
    #[allow(clippy::needless_collect)]
    let handles = (0..4)
        .map(|_| {
            let library = library.clone();
            std::thread::spawn(move || {
                library
                    .books()
                    .query(BookQuery::default())
                    .expect("thread query")
                    .total
            })
        })
        .collect::<Vec<_>>();
    assert!(
        handles
            .into_iter()
            .all(|handle| handle.join().expect("join") == 3)
    );
}

#[test]
fn rich_filters_and_multi_sort_compose() {
    let fixture = support::TestLibrary::new();
    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open writable");
    for (title, author, tag, publisher, rating) in [
        ("One", "Ada Alpha", "rust", "Press B", 6),
        ("Two", "Ada Beta", "rust", "Press A", 10),
        ("Three", "Other", "history", "Press C", 8),
    ] {
        library
            .books()
            .add(NewBook {
                title: title.into(),
                authors: vec![author.into()],
                tags: vec![tag.into()],
                publisher: Some(publisher.into()),
                languages: vec!["eng".into()],
                identifiers: BTreeMap::from([("test".into(), title.to_lowercase())]),
                rating: Some(Rating::new(rating).expect("rating")),
                formats: vec![FormatFile::new("tests/fixtures/sample.epub")],
                ..NewBook::default()
            })
            .expect("add");
    }

    let query = BookQuery::default()
        .filter(BookFilter::AuthorContains("ada".into()))
        .filter(BookFilter::Tag("RUST".into()))
        .filter(BookFilter::Language("eng".into()))
        .filter(BookFilter::Format("epub".into()))
        .filter(BookFilter::RatingRange {
            minimum: Rating::new(6).expect("rating"),
            maximum: Rating::new(10).expect("rating"),
        })
        .order_by([
            BookOrder::new(BookSort::Rating, SortDirection::Descending),
            BookOrder::ascending(BookSort::Publisher),
        ]);
    let page = library.books().query(query).expect("query");
    assert_eq!(page.total, 2);
    assert_eq!(
        page.items
            .iter()
            .map(|book| book.title.as_str())
            .collect::<Vec<_>>(),
        ["Two", "One"]
    );

    let identifier_query = BookQuery::default().filter(BookFilter::Identifier {
        kind: Some("TEST".into()),
        value: "three".into(),
    });
    assert_eq!(
        library
            .books()
            .query(identifier_query)
            .expect("identifier query")
            .items[0]
            .title,
        "Three"
    );
}

#[test]
fn audit_reports_database_and_filesystem_disagreement_without_mutating() {
    let fixture = support::TestLibrary::new();
    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open writable");
    let book = library.books().add(populated_input()).expect("add");
    assert!(library.auditor().run().expect("clean audit").is_clean());

    std::fs::write(&book.formats[0].path, b"short").expect("corrupt format bytes");
    std::fs::remove_file(book.cover_path.as_ref().expect("cover")).expect("remove cover");
    let connection = rusqlite::Connection::open(fixture.database()).expect("open db");
    connection
        .execute(
            "INSERT INTO books(title, path, uuid) VALUES ('Unsafe', '../outside', 'unsafe')",
            [],
        )
        .expect("insert unsafe path");
    drop(connection);

    let report = library.auditor().run().expect("audit");
    assert!(report.issues.iter().any(|issue| {
        issue.kind == AuditIssueKind::FormatSizeMismatch && issue.book_id == Some(book.id)
    }));
    assert!(report.issues.iter().any(|issue| {
        issue.kind == AuditIssueKind::MissingCover && issue.book_id == Some(book.id)
    }));
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.kind == AuditIssueKind::UnsafeBookPath)
    );
    assert_eq!(
        std::fs::read(&book.formats[0].path).expect("audit preserved bytes"),
        b"short"
    );
}

#[test]
fn reads_normalized_and_scalar_custom_columns() {
    let fixture = support::TestLibrary::new();
    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open writable");
    let book = library
        .books()
        .add(NewBook {
            title: "Custom values".into(),
            authors: vec!["Tester".into()],
            ..NewBook::default()
        })
        .expect("add");
    let connection = rusqlite::Connection::open(fixture.database()).expect("open db");
    connection
        .execute_batch(
            "
            INSERT INTO custom_columns
                (id, label, name, datatype, is_multiple, normalized)
                VALUES
                (1, 'topics', 'Topics', 'text', 1, 1),
                (2, 'cycle', 'Cycle', 'series', 0, 1),
                (3, 'reviewed', 'Reviewed', 'bool', 0, 0),
                (4, 'derived', 'Derived', 'composite', 0, 0);
            CREATE TABLE custom_column_1 (
                id INTEGER PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE books_custom_column_1_link (
                id INTEGER PRIMARY KEY,
                book INTEGER NOT NULL,
                value INTEGER NOT NULL
            );
            INSERT INTO custom_column_1(id, value) VALUES (1, 'one'), (2, 'two');
            CREATE TABLE custom_column_2 (
                id INTEGER PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE books_custom_column_2_link (
                id INTEGER PRIMARY KEY,
                book INTEGER NOT NULL,
                value INTEGER NOT NULL,
                extra REAL
            );
            INSERT INTO custom_column_2(id, value) VALUES (1, 'A Cycle');
            CREATE TABLE custom_column_3 (
                id INTEGER PRIMARY KEY,
                book INTEGER NOT NULL,
                value INTEGER
            );
            ",
        )
        .expect("create custom columns");
    connection
        .execute(
            "INSERT INTO books_custom_column_1_link(book, value) VALUES (?1, 1), (?1, 2)",
            [book.id.get()],
        )
        .expect("link text values");
    connection
        .execute(
            "INSERT INTO books_custom_column_2_link(book, value, extra) VALUES (?1, 1, 3.5)",
            [book.id.get()],
        )
        .expect("link series");
    connection
        .execute(
            "INSERT INTO custom_column_3(book, value) VALUES (?1, 1)",
            [book.id.get()],
        )
        .expect("set boolean");
    drop(connection);

    let columns = library.custom_columns();
    let definitions = columns.definitions().expect("definitions");
    assert_eq!(definitions.len(), 4);
    assert_eq!(definitions[0].kind, CustomColumnKind::Text);
    assert_eq!(
        columns.value(book.id, "#topics").expect("topics"),
        Some(CustomColumnValue::TextList(vec![
            "one".into(),
            "two".into()
        ]))
    );
    assert_eq!(
        columns.value(book.id, "cycle").expect("series"),
        Some(CustomColumnValue::Series {
            name: "A Cycle".into(),
            index: Some(3.5),
        })
    );
    let values = columns.values(book.id).expect("all values");
    assert_eq!(
        values.get("reviewed"),
        Some(&CustomColumnValue::Boolean(Some(true)))
    );
    assert_eq!(values.get("derived"), Some(&CustomColumnValue::Unavailable));
    assert!(library.capabilities().read_custom_columns);
    assert!(!library.capabilities().write_custom_columns);
}

#[test]
fn pending_book_add_recovery_blocks_writes_and_removes_orphan() {
    let fixture = support::TestLibrary::new();
    let relative = Path::new("Tester").join("Interrupted (4242)");
    let orphan = fixture.path().join(&relative);
    std::fs::create_dir_all(&orphan).expect("create interrupted directory");
    std::fs::write(orphan.join("partial.epub"), b"partial").expect("write partial asset");
    let recovery = fixture.path().join(".calibre-rs/recovery");
    std::fs::create_dir_all(&recovery).expect("create recovery directory");
    std::fs::write(
        recovery.join("interrupted.journal"),
        format!(
            "calibre-rs-recovery-v1\nbook-add\n4242\n{}\n-\n",
            recovery_path_hex(&relative)
        ),
    )
    .expect("write interrupted journal");

    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open with pending recovery");
    assert!(library.capabilities().recovery_required);
    assert!(!library.capabilities().write_books);
    assert_eq!(library.pending_recovery().expect("pending").len(), 1);
    assert!(matches!(
        library.books().add(NewBook::default()),
        Err(Error::UnsupportedOperation { .. })
    ));
    assert_eq!(library.recover_pending().expect("recover").resolved, 1);
    assert!(!orphan.exists());
    assert!(library.pending_recovery().expect("clear").is_empty());

    let reopened = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("reopen after recovery");
    assert!(!reopened.capabilities().recovery_required);
    reopened
        .books()
        .add(NewBook {
            title: "After recovery".into(),
            authors: vec!["Tester".into()],
            ..NewBook::default()
        })
        .expect("write after recovery");
}

#[test]
fn interrupted_book_removal_restores_a_book_still_present_in_database() {
    let fixture = support::TestLibrary::new();
    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open writable");
    let book = library
        .books()
        .add(NewBook {
            title: "Restore me".into(),
            authors: vec!["Tester".into()],
            formats: vec![FormatFile::new("tests/fixtures/sample.epub")],
            ..NewBook::default()
        })
        .expect("add");
    let original = fixture.path().join(&book.relative_path);
    let staged_relative = Path::new(".calibre-rs-delete-interrupted");
    let staged = fixture.path().join(staged_relative);
    let recovery = fixture.path().join(".calibre-rs/recovery");
    std::fs::create_dir_all(&recovery).expect("create recovery directory");
    std::fs::write(
        recovery.join("removal.journal"),
        format!(
            "calibre-rs-recovery-v1\nbook-remove\n{}\n{}\n{}\n",
            book.id.get(),
            recovery_path_hex(&book.relative_path),
            recovery_path_hex(staged_relative)
        ),
    )
    .expect("write removal journal");
    std::fs::rename(&original, &staged).expect("simulate staged removal");

    assert_eq!(library.recover_pending().expect("recover").resolved, 1);
    assert!(original.is_dir());
    assert!(!staged.exists());
    assert_eq!(
        library.books().get(book.id).expect("book restored").title,
        "Restore me"
    );
}

#[test]
fn ambiguous_recovery_state_keeps_the_journal_and_write_block() {
    let fixture = support::TestLibrary::new();
    let relative = Path::new("Tester").join("Missing (4242)");
    let connection = rusqlite::Connection::open(fixture.database()).expect("open db");
    connection
        .execute(
            "INSERT INTO books(id, title, path, uuid) VALUES (4242, 'Missing', ?1, 'missing')",
            [relative.to_string_lossy().as_ref()],
        )
        .expect("insert committed row without directory");
    drop(connection);
    let recovery = fixture.path().join(".calibre-rs/recovery");
    std::fs::create_dir_all(&recovery).expect("create recovery directory");
    std::fs::write(
        recovery.join("ambiguous.journal"),
        format!(
            "calibre-rs-recovery-v1\nbook-add\n4242\n{}\n-\n",
            recovery_path_hex(&relative)
        ),
    )
    .expect("write interrupted journal");

    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open with recovery");
    assert!(matches!(
        library.recover_pending(),
        Err(Error::UnsupportedOperation { .. })
    ));
    assert_eq!(
        library.pending_recovery().expect("journal retained").len(),
        1
    );
    assert!(matches!(
        library.formats().remove(calibre::BookId::new(4242), "EPUB"),
        Err(Error::UnsupportedOperation { .. })
    ));
}

fn recovery_path_hex(path: &Path) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(windows)]
    let bytes = {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    };
    #[cfg(not(any(unix, windows)))]
    let bytes = path.as_os_str().as_encoded_bytes().to_vec();

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(unix)]
#[test]
fn audit_reports_cover_symlink_escape() {
    use std::os::unix::fs::symlink;

    let fixture = support::TestLibrary::new();
    let outside = tempfile::tempdir().expect("outside");
    let outside_cover = outside.path().join("cover.jpg");
    std::fs::write(&outside_cover, b"outside").expect("outside cover");
    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("open writable");
    let book = library
        .books()
        .add(NewBook {
            title: "Symlink cover".into(),
            authors: vec!["Tester".into()],
            ..NewBook::default()
        })
        .expect("add");
    symlink(
        &outside_cover,
        fixture.path().join(&book.relative_path).join("cover.jpg"),
    )
    .expect("symlink cover");
    let connection = rusqlite::Connection::open(fixture.database()).expect("open db");
    connection
        .execute(
            "UPDATE books SET has_cover = 1 WHERE id = ?1",
            [book.id.get()],
        )
        .expect("set cover flag");
    drop(connection);

    let report = library.auditor().run().expect("audit");
    assert!(report.issues.iter().any(|issue| {
        issue.kind == AuditIssueKind::UnsafeCoverPath && issue.book_id == Some(book.id)
    }));
}

#[cfg(unix)]
#[test]
fn rejects_symlink_escape_from_database_path() {
    use std::os::unix::fs::symlink;

    let fixture = support::TestLibrary::new();
    let outside = tempfile::tempdir().expect("outside");
    symlink(outside.path(), fixture.path().join("escape")).expect("symlink");
    let connection = rusqlite::Connection::open(fixture.database()).expect("open db");
    connection
        .execute(
            "INSERT INTO books(id, title, path, last_modified) VALUES (1, 'Bad', 'escape', 'now')",
            [],
        )
        .expect("insert");
    drop(connection);
    let library = Library::open(fixture.path()).expect("open");
    assert!(matches!(
        library.books().get(calibre::BookId::new(1)),
        Err(Error::PathEscape { .. })
    ));
}
