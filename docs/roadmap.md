# Roadmap

## 0.1: core schema-27 slice

Acceptance:

- open without mutating the database in read-only mode;
- read all core metadata and checked asset paths;
- add and update books, formats, and covers with compensation tests;
- move directories after title or first-author changes;
- reopen Rust changes with Calibre.

Status: implemented. Exact sort and filename parity remains a documented gap.

## 0.2: crash recovery and trash

Acceptance:

- persist a recovery journal for database/filesystem operations;
- inject process interruption at each boundary and recover on next open;
- match Calibre's book and format trash layout;
- restore and expire trash through paired Calibre/Rust tests.

## 0.3: schema and platform matrix

Acceptance:

- fixtures for each declared Calibre and schema version;
- explicit read-only policies for older or newer schemas;
- black-box filename corpus on Linux, macOS, and Windows;
- case-only moves, reserved names, long paths, and non-UTF-8 Unix roots.

## 0.4: dynamic library state

Acceptance:

- discover and validate custom-column table identifiers;
- read and write supported custom-column types;
- clean custom links during deletion;
- maintain preferences, plugin data, and conversion options without exposing
  SQLite types.

## 0.5: notes, annotations, and FTS

Acceptance:

- version and validate side databases;
- update notes and annotations with Calibre reopen tests;
- update or invalidate FTS state without reproducing Calibre's tokenizer;
- enable format and deletion capabilities when FTS is active.

## Deferred

Library creation, OPF backup generation, library restore, and proven
cross-process coordination remain deferred. The ebook reader, editor,
conversion engine, plugin runtime, content server, and device drivers stay out
of scope.
