# Provenance

## Policy

The Rust implementation is independent work licensed under MIT OR Apache-2.0.
Do not copy or translate Calibre's GPLv3 source, schema SQL, triggers,
migrations, sorting code, filename algorithms, OPF code, or FTS tokenizer.

Contributors may derive behavior from public API documentation, database
introspection, and black-box tests against disposable libraries. Flag any
proposed reuse of upstream code or exact SQL for owner and licensing review
before adding it.

## Calibre research

Research on 2026-07-18 used:

- Calibre database API manual, version 9.11.0:
  <https://manual.calibre-ebook.com/db_api.html>
- Calibre tag `v9.11.0`, commit
  `b23dfb5d42b93919511ef472d6a85945d7e8c8c5`
- Calibre master commit
  `01200ab0ef9f68cbb01e939d7915e0bac45a976c`
- schema upgrade history:
  <https://github.com/kovidgoyal/calibre/blob/b23dfb5d42b93919511ef472d6a85945d7e8c8c5/src/calibre/db/schema_upgrades.py>
- filesystem backend:
  <https://github.com/kovidgoyal/calibre/blob/b23dfb5d42b93919511ef472d6a85945d7e8c8c5/src/calibre/db/backend.py>
- Calibre GPLv3 license:
  <https://github.com/kovidgoyal/calibre/blob/b23dfb5d42b93919511ef472d6a85945d7e8c8c5/LICENSE>

Database introspection established schema version 27, application ID
`0x63616c69`, table and column names, relationship rows, stored rating scale,
format stems and sizes, and dirty-metadata state. Black-box Calibre 9.10.0 runs
established author link order, uppercase logical formats, lowercase physical
extensions, `cover.jpg`, title/first-author directory moves, and Calibre reopen
behavior.

Research on 2026-07-19 used the public database API documentation for
multi-column sorting and field access. Independent inspection of a disposable
schema-27 database established the `custom_columns` definition shape and the
numeric `custom_column_N` and `books_custom_column_N_link` table patterns.
Inspection covered text, comments, series, rating, datetime, boolean, integer,
float, enumeration, and composite definitions. No Calibre query,
trigger, template evaluator, or custom-column implementation entered this
crate.

The version-2 recovery journal and reconciliation rules are project-specific
designs. They use database values and filesystem paths that the crate already
reads or writes. The implementation does not reuse Calibre recovery code,
algorithms, triggers, or SQL.

Trash research on 2026-07-19 used the public database API and disposable
Calibre 9.10.0 libraries. Black-box runs established the `.caltrash` directory
layout, lowercase format names, compact format metadata, directory-mtime
expiry, replacement behavior, and restoration of original book IDs. Generated
OPF files supplied metadata-field examples. The crate's OPF reader, writer,
recovery journal, and SQLite statements are independent work. No Calibre trash
or OPF implementation entered this crate.

Paired compatibility tests with Calibre 9.10.0 and 9.11.0 exercised both
directions: Calibre restored Rust-created book and format entries, and Rust
restored a book removed by Calibre. The tests call Calibre only as a
development oracle and do not add a runtime dependency.

The 9.11.0 run used Calibre's official x86_64 Linux archive. Its SHA-512 digest
matched the value published at `calibre-ebook.com/signatures` before
extraction.

The implementation registers independent `uuid4` and conservative `title_sort`
SQLite functions because existing Calibre triggers call functions with those
names. It does not reproduce Calibre's locale-aware algorithms.

Native-creation research on 2026-07-19 used disposable empty libraries from
Calibre 9.10.0 and 9.11.0. SQLite PRAGMA introspection and object-name queries
recorded the application ID, schema version, table/column shapes, index
membership, and the presence of bookkeeping triggers without reading schema
SQL, view definitions, or trigger bodies. Row inspection recorded a fresh
library UUID plus the four initial preference keys and JSON scalar/object
values. Controlled removal experiments established that Calibre's core CLI
requires `annotations_dirtied`, while `calibredb check_library`, add, list, and
metadata writes do not require the legacy views or annotation FTS virtual
tables.

The creation DDL, constraints, indexes, and six small bookkeeping triggers are
independent project work based on those observations and on the crate's own
write invariants. They are not copies or translations of Calibre or Citadel
schema SQL or trigger algorithms. Failure staging, validation, cleanup, and
publication are project-specific. Paired black-box tests then ran
`calibredb check_library`, metadata mutation, date exchange, and trash
interoperation against the Rust-created result.

The Calibre 9.11.0 x86_64 Linux archive used for the final creation oracle was
downloaded from the official release host. Its SHA-512 digest matched the
value published at `calibre-ebook.com/signatures` before extraction.

## Citadel research

Citadel and its internal `libcalibre` crate informed test coverage and risk
analysis at commit `f0ec58eee58185a770e9f174d6edf7255245adf6`:

<https://github.com/everydaythingssoftware/citadel/tree/f0ec58eee58185a770e9f174d6edf7255245adf6/crates/libcalibre>

The review identified missing schema checks, unchecked paths, incomplete
filesystem compensation, and gaps in format, cover, deletion, and directory
move tests. No Citadel implementation, SQL, fixture database, or migration was
copied or translated.

## Fixtures

Committed format and cover payloads under `tests/fixtures` contain short text
authored for this project. They contain no third-party book content.

Tests build an independently authored minimal schema in a temporary directory.
The repository does not commit Calibre-generated `metadata.db` files. The
ignored oracle test asks `calibredb` to generate a fresh temporary library and
deletes it when the test process exits.

## Review gate

The owner must review this record and the `.crate` archive before publication.
Any future Calibre-generated binary fixture needs a separate licensing decision
and recorded Calibre version.
