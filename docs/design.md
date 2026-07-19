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

The public surface is split into handles obtained from `Library`: books,
formats, and covers. Public values contain owned Rust and standard-library
types; SQLite types remain private. Identifier newtypes prevent mixing entity
IDs. Input structs are non-exhaustive and have defaults so fields can be added
without breaking callers.

The API is synchronous and has no async runtime dependency. Async applications
should run library calls on their runtime's blocking-worker facility.
