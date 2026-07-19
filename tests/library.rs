#![allow(missing_docs)]

mod support;

use calibre::{
    BookQuery, DeletionMode, Error, FormatFile, Library, NewBook, OpenOptions, Rating, UpdateBook,
};
use std::collections::BTreeMap;

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
