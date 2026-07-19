#![allow(missing_docs)]

use calibre::{BookQuery, Library, NewBook, OpenOptions};
use std::process::Command;

#[test]
#[ignore = "requires CALIBREDB=/path/to/calibredb; run through scripts/compatibility-test.sh"]
fn calibre_reopens_rust_changes_and_rust_reads_calibre_changes() {
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
            ..NewBook::default()
        })
        .expect("Rust add");
    drop(library);

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
    let library = Library::open(fixture.path()).expect("Rust reopens");
    assert_eq!(
        library
            .books()
            .get(calibre::BookId::new(1))
            .expect("book")
            .tags[0]
            .name,
        "changed-by-calibre"
    );
}
