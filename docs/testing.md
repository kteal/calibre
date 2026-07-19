# Testing

## Fast checks

Run:

```console
cargo fmt --check
cargo check --all-targets --all-features
cargo test --all-features
cargo clippy --all-targets --all-features -- \
  -D warnings \
  -W clippy::pedantic \
  -W clippy::nursery
cargo doc --all-features --no-deps
```

Unit tests cover path validation, sanitization properties, and injected native
creation failures before and after the schema transaction. Integration tests
build a fresh schema-27 library for each case. They cover native creation from
missing and empty directories, refusal of existing contents, reopen, the full
created-library lifecycle, open modes, schema rejection, publication-date
validation and round trips, full metadata, rich queries, concurrent reads, streaming assets,
read-only audits, custom-column reads, staged-write rollback, directory moves,
permanent deletion, Calibre trash, recovery journals, and Unix symlink escape.
Trash tests cover Unicode listing metadata, format collisions, whole-book core
metadata, copy and restore, explicit deletion, expiry, deferred-state refusal,
and transaction failure. Recovery tests construct interrupted format, cover,
trash, and directory-move states on both sides of the database commit, then
verify rollback or roll-forward behavior.

Property tests generate Unicode titles and metadata updates, then verify
database and filesystem round trips.

## Calibre oracle

Production code has no Calibre dependency. Run the ignored compatibility test
with a development Calibre installation:

```console
CALIBREDB=/path/to/calibredb scripts/compatibility-test.sh
```

The test performs this cycle in a temporary directory:

1. Calibre creates and populates a library.
2. Rust reads it and adds a book.
3. Calibre runs `check_library` and changes metadata.
4. Rust reopens the library and reads Calibre's change.
5. Rust trashes a format and Calibre restores it.
6. Calibre trashes a book and Rust restores it.
7. Rust trashes that book and Calibre restores it.
8. Rust creates a new library, Calibre checks and mutates it, and Rust reopens
   the result.

The paired metadata cycle also proves that Calibre reads a Rust-written
publication date and Rust reads a Calibre-written date.

Record the command output and Calibre version in `docs/compatibility.md` before
claiming support for a release or platform.

## Fixture generation

Keep inputs small and project-authored. A generator must refuse an existing
destination and a path outside its new temporary root. Record the Calibre
version, operating system, commands, and hashes.

Do not commit a Calibre-generated database until the owner completes the
licensing review described in `docs/provenance.md`.

## Remaining coverage

The roadmap tracks Windows path edge cases, macOS case behavior, multiple
Calibre versions, active FTS, preference-backed trash expiry, and custom-column
writes and cleanup. CI runs the Rust test suite on Linux, macOS, and Windows.
Developers run the Calibre oracle test before recording compatibility with a
Calibre release.
