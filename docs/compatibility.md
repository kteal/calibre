# Compatibility

## Supported policy

Release 0.1.0 opens libraries with:

- `PRAGMA user_version = 27`;
- `PRAGMA application_id = 0x63616c69`;
- all required core tables and columns.

The crate rejects older, newer, zero-version, and structurally incomplete
databases in both modes. Future releases may add read-only support for other
versions after fixtures prove each schema shape.

Calibre 9.10.0 on Linux passed the create, Rust read, Rust write, Calibre reopen,
Calibre write, Rust reopen cycle. Calibre 9.11.0 documentation and source tag
informed research, but no 9.11.0 executable test has run.

## Implemented operations

| Area | Status | Constraint |
|---|---|---|
| Open | Supported | Existing schema-27 libraries only |
| Read core metadata | Supported | Required schema signature must match |
| Query and pagination | Supported | Stored sort values, stable ID tie break |
| Resolve assets | Supported | Rejects traversal and escaping symlinks |
| Add book | Supported with gaps | Conservative English sort and filename generation |
| Update core metadata | Supported with gaps | Moves directory for title or first-author changes |
| Formats | Supported | Disabled when `full-text-search.db` exists |
| Covers | Supported | `cover.jpg` and `has_cover` kept together |
| Permanent book removal | Conditional | Disabled with FTS or active custom columns |
| Calibre trash | Unsupported | Returns `UnsupportedOperation` |

Read paths use the `data.name` stem from `metadata.db`. The crate reports both
the stored format size and the filesystem size. A missing file produces
`file_size = None`; read and copy operations return an I/O error.

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

SQLite and a filesystem do not provide one shared transaction. A process or
machine crash can interrupt compensation. The roadmap requires a recovery
journal before the crate claims crash-safe cross-resource writes.
