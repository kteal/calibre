# Changelog

This project follows Semantic Versioning.

## [Unreleased]

- Open and validate schema-27 Calibre libraries in read-only or read-write mode.
- Report schema compatibility and operation capabilities.
- Query and load books with core relationships, formats, sizes, and cover paths.
- Compose relationship, identifier, format, rating, and cover filters with
  stable multi-column sorting.
- Add and update books with compensated filesystem moves.
- Add, replace, retrieve, copy, stream, and remove formats and covers.
- Audit database integrity and core filesystem agreement without mutation.
- Discover custom-column definitions and read scalar and normalized values.
- Recover interrupted book, format, cover, and directory-move writes from
  durable versioned journals.
- List, copy, restore, delete, and expire Calibre-compatible whole-book and
  format trash, with durable recovery for trash moves.
- Make format removal use Calibre trash by default and add explicit permanent
  format removal.
- Permanently delete books when no unsupported deferred state is active.
- Add disposable integration, property, and Calibre oracle tests.

[Unreleased]: https://github.com/kteal/calibre/compare/8067b0f...HEAD
