# Design

## ADR-0001: connections and locking

Status: accepted for milestone 0.1.0.

`Library` stores the canonical library root, the path to `metadata.db`, the open
mode, detected compatibility information, and an in-process write mutex. It
does not keep a SQLite connection open. Each operation opens a short-lived
`rusqlite::Connection` with explicit read-only or read-write flags.

This model was chosen for three reasons:

1. Independent read connections allow concurrent callers without making a
   connection cross thread boundaries.
2. A process-local mutex and `BEGIN IMMEDIATE` serialize this crate's writers
   and make lock contention fail predictably.
3. Short-lived connections reduce the chance of retaining stale schema or file
   state after another application changes the library.

The mutex does not coordinate with Calibre or another process. Until a shared
locking protocol is implemented and compatibility-tested, Calibre and this
crate must not write the same library concurrently.

Read-only connections use SQLite's read-only and no-mutex flags. Read-write
connections use no-mutex mode and set a bounded busy timeout. Database-only
writes use transactions.

Operations spanning SQLite and the filesystem stage files inside the target
book directory, rename them into place, then commit the database transaction.
On failure they remove staged files and restore or remove renamed files when
possible. This is compensation, not a cross-filesystem atomic transaction.
Source format and cover files are copied; the caller's files are never moved.

## Compatibility boundary

The first release identifies the schema through `PRAGMA user_version` and
`PRAGMA application_id`, then validates the required tables and columns. Schema
version 27 is the only version accepted in either mode. The crate never changes
`user_version` and never runs Calibre migrations.

Write support is capability-based as well as version-based. An operation whose
filesystem naming, trigger, trash, or dirty-metadata behavior has not been
independently verified returns a structured unsupported-operation error.

## Filesystem containment

Paths read from `metadata.db` are untrusted. Relative database paths must have
only normal platform path components. Resolved paths are checked against the
canonical library root, and existing symlinks are rejected when they resolve
outside it. File extensions are validated separately and cannot contain path
separators.

## Public API shape

`Library` supplies handles for books, formats, covers, audits, and custom
columns. Public values contain owned Rust and standard-library types; SQLite
types remain private. Identifier newtypes prevent mixing entity IDs. Input
structs are non-exhaustive and have defaults so fields can be added without
breaking callers.

The API is synchronous and has no async runtime dependency. Async applications
should run library calls on their runtime's blocking-worker facility.

## ADR-0002: durable book-operation recovery

Status: accepted for pre-release 0.1.0.

Book creation and permanent removal create a versioned journal under
`.calibre-rs/recovery` before changing a book directory. The crate syncs the
journal file and, on Unix, its directory. Normal completion removes and syncs
the record. A pending record blocks other writes.

Recovery uses book-row existence as the decision point. An absent row after an
interrupted add makes the new directory an orphan, so recovery removes it. A
present row after an interrupted removal makes the staged directory live data,
so recovery restores it. An absent row lets recovery finish deleting the
staged directory.

## ADR-0003: durable asset and directory-move recovery

Status: accepted for pre-release 0.1.0.

Format and cover writes store a version-2 JSON journal before staging bytes.
The record contains the old and intended database values, destination,
temporary path, backup path, and whether a file existed before the write. The
crate replaces the staging-phase record with a ready-phase record after it
syncs the staged file.

Recovery treats the database as the commit decision. The old database state
rolls the files back. The intended state rolls them forward. A cover
replacement can keep `has_cover = 1` on both sides, so a ready record rolls
forward when both states match. A staging record always rolls back.

Directory-move records contain the old and new book paths plus each format
filename change. Recovery compares the current `books.path` value with those
two paths, then completes or reverses the directory and file renames. It
refuses recovery if the database matches neither state, both directories
exist, or a required file has an ambiguous state.

Version-1 book journals remain readable. New asset and move records use version
2. Pending records block other writes until `recover_pending()` removes them.
Normal errors still run compensation and remove the journal after they restore
the old state.
