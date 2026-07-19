# Compatibility

## Supported policy

The pre-release 0.1.0 code opens libraries with:

- `PRAGMA user_version = 27`;
- `PRAGMA application_id = 0x63616c69`;
- all required core tables and columns.

The crate rejects older, newer, zero-version, and structurally incomplete
databases in both modes. Future releases may add read-only support for other
versions after fixtures prove each schema shape.

Calibre 9.10.0 on Linux passed the create, Rust read, Rust write, Calibre reopen,
Calibre write, Rust reopen cycle again on 2026-07-19. Calibre 9.11.0
documentation and source tag informed research, but no 9.11.0 executable test
has run.

## Implemented operations

| Area | Status | Constraint |
|---|---|---|
| Open | Supported | Existing schema-27 libraries only |
| Read core metadata | Supported | Required schema signature must match |
| Query and pagination | Supported | Composable AND filters, multi-sort, stable ID tie break |
| Resolve assets | Supported | Rejects traversal and escaping symlinks |
| Add book | Supported with gaps | Conservative English sort and filename generation |
| Update core metadata | Supported with gaps | Moves directory for title or first-author changes |
| Formats | Supported | Path and streaming I/O; writes disabled with FTS |
| Covers | Supported | Path and streaming I/O; `cover.jpg` and flag kept together |
| Read-only audit | Supported | SQLite quick check and core asset agreement |
| Read custom columns | Partial | Stored scalar and normalized values; no template evaluation |
| Write custom columns | Unsupported | Definition and value mutation are refused |
| Recovery | Supported | Durable for book, format, cover, and directory-move writes |
| Permanent book removal | Conditional | Disabled with FTS or active custom columns |
| Calibre trash | Unsupported | Returns `UnsupportedOperation` |

Read paths use the `data.name` stem from `metadata.db`. The crate reports both
the stored format size and the filesystem size. A missing file produces
`file_size = None`; read and copy operations return an I/O error. The audit API
reports missing files, size mismatches, cover-flag mismatches, unsafe paths, and
SQLite quick-check failures without changing the library.

## Known gaps

Calibre computes title sorts, author sorts, language normalization, Unicode
category matching, filename transliteration, and path shortening with locale
and preference state. This release uses documented stored sort values for
reads. Writes generate conservative English sort values and portable filenames.
Calibre can open those records, but generated values do not claim exact parity.

The update API accepts three-letter language codes because Calibre stores those
codes. It does not map two-letter codes or language names.

Format and book changes do not update Calibre's FTS side database. Capability
checks refuse affected operations when that file exists. Permanent deletion
also refuses active custom columns because Calibre installs cleanup state that
this crate does not reproduce.

Custom-column reads validate numeric table identifiers and expected table
columns before building dynamic SQL. The API reads text, comments, series,
ratings, timestamps, booleans, integers, floats, and enumerations. Composite
columns return `Unavailable` because they require Calibre's template evaluator.
The crate does not parse the raw JSON display configuration.

The crate marks `metadata_dirtied`, updates `last_modified`, and requests page
recount after format changes. Calibre writes `metadata.opf` when it processes
dirty metadata.

## Concurrency

Each call opens a short-lived SQLite connection. Concurrent reads work.
`Library` serializes writers in one process and starts immediate transactions.

Calibre's documented multiple-reader/single-writer guarantee applies to its own
in-process API. Close Calibre before writing through this crate.

## Filesystem transactions

New files use a temporary file in the destination directory followed by rename.
Replacements retain the old file until the database commits. Book deletion
stages the directory inside the library root. On ordinary errors, the crate
restores staged files or directories.

SQLite and a filesystem do not provide one shared transaction. Book, format,
cover, and directory-move writes create journals under
`.calibre-rs/recovery`. Pending journals disable write capabilities.
`recover_pending()` compares the current database state with the journal, then
completes or reverses the filesystem changes.

Recovery requires an intact journal and unambiguous database and filesystem
state. It returns an error and retains the journal if another process changed
either side. Storage-device failure and concurrent external writers remain
outside the compatibility claim.
