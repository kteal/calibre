# calibre

`calibre` is a native Rust crate for reading and writing existing Calibre
libraries. It reads `metadata.db` with SQLite and works with book directories,
formats, covers, and core metadata. Your program does not need the Calibre
executable, Python, or a Calibre installation at runtime.

The crate is an independent project. It has no affiliation with the Calibre
project or its maintainers.

## Status

The pre-release 0.1.0 code supports Calibre schema version 27. The
compatibility test has passed against Calibre 9.10.0 on Linux.

The first milestone provides:

- read-only and read-write opening with schema and application-ID validation;
- capability reporting;
- composable book filters, multi-column sorting, pagination, and full core
  metadata loading;
- checked format and cover paths with traversal and symlink containment;
- book creation and core metadata updates;
- staged and streaming format and cover add, replace, read, copy, and removal;
- compensated directory moves after title or first-author changes;
- read-only library audits for database, format, cover, and path disagreement;
- read-only custom-column discovery and stored-value access;
- durable recovery for interrupted book creation and permanent removal;
- explicit permanent book deletion for libraries without active custom columns
  or full-text-search state.

Calibre-compatible trash, locale-aware sort generation, exact filename parity,
custom-column writes, preferences, notes, annotations, and FTS maintenance
remain unsupported. Composite custom-column values require Calibre's template
engine and return `CustomColumnValue::Unavailable`. The crate refuses
operations whose deferred state it cannot update. Read
[compatibility.md](docs/compatibility.md) before writing a library.

## Example

```rust,no_run
use calibre::{BookQuery, FormatFile, Library, NewBook, OpenOptions};

fn main() -> Result<(), calibre::Error> {
    let library = Library::open_with(
        "/path/to/library",
        OpenOptions::new().read_write(true),
    )?;

    let page = library.books().query(BookQuery::default())?;
    println!("{} books", page.total);

    let book = library.books().add(NewBook {
        title: "The Odyssey".into(),
        authors: vec!["Homer".into()],
        formats: vec![FormatFile::new("/tmp/odyssey.epub")],
        ..NewBook::default()
    })?;

    library.formats().add(book.id, "/tmp/odyssey.pdf")?;
    Ok(())
}
```

The API is synchronous. Async applications should call it from a
blocking-worker thread.

## Safety and coordination

Do not let Calibre and a Rust process write the same library at the same time.
The crate serializes its own writers and uses SQLite transactions, but it does
not share Calibre's in-process lock.

Book creation and permanent deletion write a recovery record before changing
the filesystem. If `capabilities().recovery_required` is true, inspect
`pending_recovery()`, call `recover_pending()` from a read-write handle, and
reopen the library. Format replacement, cover replacement, and directory moves
have in-process compensation but no durable crash journal yet.

Write tests create a new temporary library and stay inside that directory.

## MSRV

The minimum supported Rust version is 1.85.0. Rust 1.85 introduced stable Rust
2024 support. CI tests this version and stable Rust. Dependencies in
`Cargo.lock` must continue to support 1.85.0.

## Development

```console
nix develop
cargo fmt --check
cargo check --all-targets --all-features
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --all-features --no-deps
```

Run the Calibre oracle test with:

```console
CALIBREDB=/path/to/calibredb scripts/compatibility-test.sh
```

## Scope

The long-term target covers Calibre's library database API and filesystem
behavior. The ebook reader, editor, conversion engine, plugin runtime, content
server, and device drivers fall outside this crate.

## License review

Independently authored code uses `MIT OR Apache-2.0`. Calibre uses GPLv3.
Contributors must follow [provenance.md](docs/provenance.md). Do not publish the
crate until the owner reviews its package contents, licensing, and provenance.
