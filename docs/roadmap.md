# Roadmap

## 0.1: core schema-27 slice

Acceptance:

- open without mutating the database in read-only mode;
- read all core metadata and checked asset paths;
- add and update books, formats, and covers with compensation tests;
- move directories after title or first-author changes;
- reopen Rust changes with Calibre.

Status: implemented. Exact sort and filename parity remains a documented gap.

## Next 0.1 batch: complete crash recovery and trash

Acceptance:

- extend the recovery journal from book add and removal to formats, covers, and
  directory moves;
- inject process interruption at each boundary and recover on next open;
- match Calibre's book and format trash layout;
- restore and expire trash through paired Calibre/Rust tests.

Current status: book add and permanent removal have durable recovery. Asset
replacement and directory moves retain in-process compensation.

## Later 0.1 batch: schema and platform matrix

Acceptance:

- fixtures for each declared Calibre and schema version;
- explicit read-only policies for older or newer schemas;
- black-box filename corpus on Linux, macOS, and Windows;
- case-only moves, reserved names, long paths, and non-UTF-8 Unix roots.

## Later 0.1 batch: dynamic library state

Acceptance:

- write supported custom-column types and definitions;
- clean custom links during deletion;
- maintain preferences, plugin data, and conversion options without exposing
  SQLite types.

Current status: the crate discovers active definitions, validates numeric
dynamic table names and columns, and reads stored scalar and normalized values.
It does not evaluate composite templates.

## Later 0.1 batch: notes, annotations, and FTS

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
