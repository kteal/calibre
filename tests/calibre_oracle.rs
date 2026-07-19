#![allow(missing_docs)]

use calibre::{BookQuery, DeletionMode, Library, NewBook, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

static ORACLE_LOCK: Mutex<()> = Mutex::new(());

#[test]
#[ignore = "requires CALIBREDB=/path/to/calibredb; run through scripts/compatibility-test.sh"]
#[allow(clippy::too_many_lines)] // One paired cycle makes both date directions explicit.
fn calibre_reopens_rust_changes_and_rust_reads_calibre_changes() {
    let _guard = ORACLE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let calibredb = std::env::var_os("CALIBREDB").expect("CALIBREDB");
    let fixture = tempfile::tempdir().expect("library");
    let status = Command::new(&calibredb)
        .args([
            "add",
            "--empty",
            "--library-path",
            fixture.path().to_str().expect("UTF-8 test path"),
            "--title",
            "Created by Calibre",
            "--authors",
            "Oracle Author",
        ])
        .status()
        .expect("run Calibre");
    assert!(status.success());

    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("Rust opens Calibre library");
    assert_eq!(
        library
            .books()
            .query(BookQuery::default())
            .expect("query")
            .total,
        1
    );
    library
        .books()
        .add(NewBook {
            title: "Created by Rust".into(),
            authors: vec!["Rust Author".into()],
            publication_date: Some("2024-02-29".into()),
            ..NewBook::default()
        })
        .expect("Rust add");
    drop(library);

    let output = Command::new(&calibredb)
        .args([
            "list",
            "--for-machine",
            "--fields",
            "id,pubdate",
            "--library-path",
            fixture.path().to_str().expect("UTF-8 test path"),
        ])
        .output()
        .expect("Calibre reads Rust publication date");
    assert!(output.status.success());
    let rows: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("Calibre list JSON");
    let rust_date = rows
        .as_array()
        .expect("Calibre rows")
        .iter()
        .find(|row| row["id"] == 2)
        .and_then(|row| row["pubdate"].as_str())
        .expect("Rust book publication date in Calibre");
    assert!(rust_date.starts_with("2024-02-29"));

    let status = Command::new(&calibredb)
        .args([
            "check_library",
            "--library-path",
            fixture.path().to_str().expect("UTF-8 test path"),
        ])
        .status()
        .expect("Calibre check");
    assert!(status.success());
    let status = Command::new(&calibredb)
        .args([
            "set_metadata",
            "--library-path",
            fixture.path().to_str().expect("UTF-8 test path"),
            "1",
            "--field",
            "tags:changed-by-calibre",
        ])
        .status()
        .expect("Calibre update");
    assert!(status.success());
    let status = Command::new(&calibredb)
        .args([
            "set_metadata",
            "--library-path",
            fixture.path().to_str().expect("UTF-8 test path"),
            "1",
            "--field",
            "pubdate:2025-03-04",
        ])
        .status()
        .expect("Calibre date update");
    assert!(status.success());
    let library = Library::open(fixture.path()).expect("Rust reopens");
    let book = library.books().get(calibre::BookId::new(1)).expect("book");
    assert_eq!(book.tags[0].name, "changed-by-calibre");
    assert_eq!(
        book.publication_date.as_deref(),
        Some("2025-03-04 00:00:00+00:00")
    );
}

#[test]
#[ignore = "requires CALIBREDB=/path/to/calibredb; run through scripts/compatibility-test.sh"]
fn calibre_checks_and_mutates_a_rust_created_library() {
    let _guard = ORACLE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let calibredb = PathBuf::from(std::env::var_os("CALIBREDB").expect("CALIBREDB"));
    let fixture = tempfile::tempdir().expect("library");
    let library = Library::create(fixture.path()).expect("Rust creates library");
    let book = library
        .books()
        .add(NewBook {
            title: "Native creation oracle".into(),
            authors: vec!["Rust Author".into()],
            publication_date: Some("2024-04-05".into()),
            formats: vec![calibre::FormatFile::new("tests/fixtures/sample.txt")],
            cover: Some("tests/fixtures/cover.jpg".into()),
            ..NewBook::default()
        })
        .expect("Rust adds book");
    assert!(library.auditor().run().expect("Rust audit").is_clean());
    drop(library);

    assert!(calibre_check(&calibredb, fixture.path()));
    assert!(
        Command::new(&calibredb)
            .args([
                "set_metadata",
                "--library-path",
                fixture.path().to_str().expect("UTF-8 test path"),
                &book.id.get().to_string(),
                "--field",
                "tags:calibre-mutated-rust-library",
            ])
            .status()
            .expect("Calibre mutates Rust-created library")
            .success()
    );
    assert!(
        Command::new(&calibredb)
            .args([
                "add",
                "--empty",
                "--library-path",
                fixture.path().to_str().expect("UTF-8 test path"),
                "--title",
                "Added to Rust library by Calibre",
                "--authors",
                "Calibre Author",
            ])
            .status()
            .expect("Calibre adds to Rust-created library")
            .success()
    );
    assert!(calibre_check(&calibredb, fixture.path()));

    let library = Library::open(fixture.path()).expect("Rust reopens Calibre mutation");
    let reopened = library.books().get(book.id).expect("mutated book");
    assert_eq!(reopened.tags[0].name, "calibre-mutated-rust-library");
    assert_eq!(
        reopened.publication_date.as_deref(),
        Some("2024-04-05 00:00:00+00:00")
    );
    assert_eq!(
        library.formats().read(book.id, "TXT").expect("format"),
        include_bytes!("fixtures/sample.txt")
    );
    assert_eq!(
        library
            .books()
            .get(calibre::BookId::new(2))
            .expect("Calibre-added book")
            .title,
        "Added to Rust library by Calibre"
    );
}

#[test]
#[ignore = "requires CALIBREDB=/path/to/calibredb; run through scripts/compatibility-test.sh"]
fn calibre_and_rust_restore_each_others_trash() {
    let _guard = ORACLE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let calibredb = PathBuf::from(std::env::var_os("CALIBREDB").expect("CALIBREDB"));
    let calibre_debug = std::env::var_os("CALIBRE_DEBUG").map_or_else(
        || calibredb.with_file_name(calibre_debug_name()),
        PathBuf::from,
    );
    let fixture = tempfile::tempdir().expect("library");
    assert!(
        Command::new(&calibredb)
            .args([
                "add",
                "--library-path",
                fixture.path().to_str().expect("UTF-8 test path"),
                "--title",
                "Trash oracle Ω",
                "--authors",
                "First Author & Second Author",
                "tests/fixtures/sample.txt",
            ])
            .status()
            .expect("Calibre add")
            .success()
    );

    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("Rust opens Calibre library");
    let book = library
        .books()
        .query(BookQuery::default())
        .expect("query")
        .items
        .remove(0);
    library
        .formats()
        .remove(book.id, "TXT")
        .expect("Rust trashes format");
    drop(library);
    calibre_debug_call(
        &calibre_debug,
        fixture.path(),
        "db.new_api.move_format_from_trash(1, 'TXT')",
    );
    assert!(calibre_check(&calibredb, fixture.path()));

    assert!(
        Command::new(&calibredb)
            .args([
                "remove",
                "--library-path",
                fixture.path().to_str().expect("UTF-8 test path"),
                "1",
            ])
            .status()
            .expect("Calibre trashes book")
            .success()
    );
    let library = Library::open_with(fixture.path(), OpenOptions::new().read_write(true))
        .expect("Rust reopens");
    let restored = library
        .trash()
        .restore_book(book.id)
        .expect("Rust restores Calibre book trash");
    assert_eq!(restored.title, "Trash oracle Ω");
    assert_eq!(restored.authors.len(), 2);
    library
        .books()
        .remove(book.id, DeletionMode::Trash)
        .expect("Rust trashes whole book");
    drop(library);

    calibre_debug_call(
        &calibre_debug,
        fixture.path(),
        "db.new_api.move_book_from_trash(1)",
    );
    assert!(calibre_check(&calibredb, fixture.path()));
    let library = Library::open(fixture.path()).expect("Rust opens final Calibre state");
    let restored = library.books().get(book.id).expect("restored book");
    assert_eq!(restored.title, "Trash oracle Ω");
    assert_eq!(
        library.formats().read(book.id, "TXT").expect("format"),
        include_bytes!("fixtures/sample.txt")
    );
}

fn calibre_check(calibredb: &Path, library: &Path) -> bool {
    Command::new(calibredb)
        .args([
            "check_library",
            "--library-path",
            library.to_str().expect("UTF-8 test path"),
        ])
        .status()
        .expect("Calibre check")
        .success()
}

fn calibre_debug_call(calibre_debug: &Path, library: &Path, operation: &str) {
    let code = format!(
        "import os; from calibre.db.legacy import LibraryDatabase; \
         db = LibraryDatabase(os.environ['CALIBRE_ORACLE_LIBRARY']); \
         {operation}; db.close()"
    );
    assert!(
        Command::new(calibre_debug)
            .args(["-c", &code])
            .env("CALIBRE_ORACLE_LIBRARY", library)
            .status()
            .expect("run calibre-debug")
            .success()
    );
}

const fn calibre_debug_name() -> &'static str {
    if cfg!(windows) {
        "calibre-debug.exe"
    } else {
        "calibre-debug"
    }
}
