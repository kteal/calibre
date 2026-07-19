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

The implementation registers independent `uuid4` and conservative `title_sort`
SQLite functions because existing Calibre triggers call functions with those
names. It does not reproduce Calibre's locale-aware algorithms.

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
