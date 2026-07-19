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

Unit tests cover path validation and sanitization properties. Integration tests
build a fresh schema-27 library for each case. They cover open modes, schema
rejection, full metadata, rich queries, concurrent reads, streaming assets,
read-only audits, custom-column reads, staged-write rollback, directory moves,
permanent deletion, recovery journals, and Unix symlink escape.

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

Record the command output and Calibre version in `docs/compatibility.md` before
claiming support for a release or platform.

## Fixture generation

Keep inputs small and project-authored. A generator must refuse an existing
destination and a path outside its new temporary root. Record the Calibre
version, operating system, commands, and hashes.

Do not commit a Calibre-generated database until the owner completes the
licensing review described in `docs/provenance.md`.

## Remaining coverage

The roadmap tracks Windows path edge cases, macOS case behavior, crash recovery
for asset replacement and directory moves, Calibre trash, multiple Calibre
versions, active FTS, and custom-column writes and cleanup. CI runs the Rust
test suite on Linux, macOS, and Windows. Developers run the Calibre oracle test
before recording compatibility with a Calibre release.
