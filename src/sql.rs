use crate::library::{LibraryInner, database_error};
use crate::model::{Author, Book, Format, Identifier, Language, Publisher, Rating, Series, Tag};
use crate::paths;
use crate::{AuthorId, BookId, Error, FormatId, Result};
use rusqlite::{Connection, OptionalExtension, Transaction};
use std::collections::{BTreeMap, HashSet};

pub(crate) fn conservative_title_sort(title: &str) -> String {
    let trimmed = title.trim();
    for article in ["the ", "an ", "a "] {
        if let Some(rest) = trimmed
            .get(..article.len())
            .filter(|prefix| prefix.eq_ignore_ascii_case(article))
            .and_then(|_| trimmed.get(article.len()..))
        {
            return format!("{rest}, {}", article.trim());
        }
    }
    trimmed.to_owned()
}

pub(crate) fn load_book(inner: &LibraryInner, connection: &Connection, id: BookId) -> Result<Book> {
    let row = connection
        .query_row(
            "SELECT title, sort, timestamp, pubdate, series_index, author_sort, path, uuid, \
             has_cover, last_modified FROM books WHERE id = ?1",
            [id.get()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, bool>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )
        .optional()
        .map_err(|error| database_error("load book", &inner.database, error))?
        .ok_or_else(|| Error::NotFound {
            entity: "book",
            id: id.get(),
        })?;
    let relative_path = std::path::PathBuf::from(&row.6);
    let book_directory = paths::resolve(&inner.root, &relative_path)?;
    let authors = load_authors(inner, connection, id)?;
    let tags = load_tags(inner, connection, id)?;
    let series = load_series(inner, connection, id, row.4)?;
    let publisher = load_publisher(inner, connection, id)?;
    let languages = load_languages(inner, connection, id)?;
    let identifiers = load_identifiers(inner, connection, id)?;
    let comments = connection
        .query_row(
            "SELECT text FROM comments WHERE book = ?1",
            [id.get()],
            |query_row| query_row.get(0),
        )
        .optional()
        .map_err(|error| database_error("load comments", &inner.database, error))?;
    let rating_value: Option<u8> = connection
        .query_row(
            "SELECT r.rating FROM books_ratings_link AS link \
             JOIN ratings AS r ON r.id = link.rating WHERE link.book = ?1 \
             ORDER BY link.id LIMIT 1",
            [id.get()],
            |query_row| query_row.get(0),
        )
        .optional()
        .map_err(|error| database_error("load rating", &inner.database, error))?;
    let rating = rating_value.and_then(|value| Rating::new(value).ok());
    let formats = load_formats(inner, connection, id, &book_directory)?;
    let cover_path = if row.8 {
        Some(paths::resolve(
            &inner.root,
            &relative_path.join("cover.jpg"),
        )?)
    } else {
        None
    };
    Ok(Book {
        id,
        title: row.0,
        sort: row.1,
        timestamp: row.2,
        publication_date: row.3,
        author_sort: row.5,
        relative_path,
        uuid: row.7,
        last_modified: row.9,
        authors,
        tags,
        series,
        publisher,
        languages,
        identifiers,
        comments,
        rating,
        formats,
        cover_path,
    })
}

fn load_authors(inner: &LibraryInner, connection: &Connection, id: BookId) -> Result<Vec<Author>> {
    let mut statement = connection
        .prepare(
            "SELECT a.id, a.name, a.sort, a.link FROM books_authors_link AS link \
             JOIN authors AS a ON a.id = link.author WHERE link.book = ?1 ORDER BY link.id",
        )
        .map_err(|error| database_error("prepare authors query", &inner.database, error))?;
    statement
        .query_map([id.get()], |row| {
            Ok(Author {
                id: AuthorId::new(row.get(0)?),
                name: row.get::<_, String>(1)?.replace('|', ","),
                sort: row.get(2)?,
                link: row.get(3)?,
            })
        })
        .map_err(|error| database_error("load authors", &inner.database, error))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| database_error("load authors", &inner.database, error))
}

fn load_tags(inner: &LibraryInner, connection: &Connection, id: BookId) -> Result<Vec<Tag>> {
    let mut statement = connection
        .prepare(
            "SELECT t.id, t.name, t.link FROM books_tags_link AS link \
             JOIN tags AS t ON t.id = link.tag WHERE link.book = ?1 ORDER BY link.id",
        )
        .map_err(|error| database_error("prepare tags query", &inner.database, error))?;
    statement
        .query_map([id.get()], |row| {
            Ok(Tag {
                id: row.get(0)?,
                name: row.get(1)?,
                link: row.get(2)?,
            })
        })
        .map_err(|error| database_error("load tags", &inner.database, error))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| database_error("load tags", &inner.database, error))
}

fn load_series(
    inner: &LibraryInner,
    connection: &Connection,
    id: BookId,
    index: f64,
) -> Result<Option<Series>> {
    connection
        .query_row(
            "SELECT s.id, s.name, s.sort, s.link FROM books_series_link AS link \
             JOIN series AS s ON s.id = link.series WHERE link.book = ?1 LIMIT 1",
            [id.get()],
            |row| {
                Ok(Series {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    sort: row.get(2)?,
                    link: row.get(3)?,
                    index,
                })
            },
        )
        .optional()
        .map_err(|error| database_error("load series", &inner.database, error))
}

fn load_publisher(
    inner: &LibraryInner,
    connection: &Connection,
    id: BookId,
) -> Result<Option<Publisher>> {
    connection
        .query_row(
            "SELECT p.id, p.name, p.sort, p.link FROM books_publishers_link AS link \
             JOIN publishers AS p ON p.id = link.publisher WHERE link.book = ?1 LIMIT 1",
            [id.get()],
            |row| {
                Ok(Publisher {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    sort: row.get(2)?,
                    link: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|error| database_error("load publisher", &inner.database, error))
}

fn load_languages(
    inner: &LibraryInner,
    connection: &Connection,
    id: BookId,
) -> Result<Vec<Language>> {
    let mut statement = connection
        .prepare(
            "SELECT l.id, l.lang_code, l.link FROM books_languages_link AS link \
             JOIN languages AS l ON l.id = link.lang_code WHERE link.book = ?1 \
             ORDER BY link.id",
        )
        .map_err(|error| database_error("prepare languages query", &inner.database, error))?;
    statement
        .query_map([id.get()], |row| {
            Ok(Language {
                id: row.get(0)?,
                code: row.get(1)?,
                link: row.get(2)?,
            })
        })
        .map_err(|error| database_error("load languages", &inner.database, error))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| database_error("load languages", &inner.database, error))
}

fn load_identifiers(
    inner: &LibraryInner,
    connection: &Connection,
    id: BookId,
) -> Result<Vec<Identifier>> {
    let mut statement = connection
        .prepare("SELECT type, val FROM identifiers WHERE book = ?1 ORDER BY id")
        .map_err(|error| database_error("prepare identifiers query", &inner.database, error))?;
    statement
        .query_map([id.get()], |row| {
            Ok(Identifier {
                kind: row.get(0)?,
                value: row.get(1)?,
            })
        })
        .map_err(|error| database_error("load identifiers", &inner.database, error))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| database_error("load identifiers", &inner.database, error))
}

fn load_formats(
    inner: &LibraryInner,
    connection: &Connection,
    id: BookId,
    book_directory: &std::path::Path,
) -> Result<Vec<Format>> {
    let mut statement = connection
        .prepare(
            "SELECT id, format, uncompressed_size, name FROM data \
             WHERE book = ?1 ORDER BY format COLLATE NOCASE, id",
        )
        .map_err(|error| database_error("prepare formats query", &inner.database, error))?;
    let rows = statement
        .query_map([id.get()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| database_error("load formats", &inner.database, error))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| database_error("load formats", &inner.database, error))?;
    rows.into_iter()
        .map(|(format_id, format, size, stem)| {
            let lowercase = format.to_ascii_lowercase();
            let candidate = book_directory.join(format!("{stem}.{lowercase}"));
            let relative = candidate
                .strip_prefix(&inner.root)
                .map_err(|_| Error::PathEscape {
                    path: candidate.clone(),
                    reason: "format candidate is outside the library".into(),
                })?;
            let path = paths::resolve(&inner.root, relative)?;
            let file_size = std::fs::metadata(&path).ok().map(|metadata| metadata.len());
            Ok(Format {
                id: FormatId::new(format_id),
                format,
                stored_size: u64::try_from(size).unwrap_or_default(),
                file_size,
                path,
            })
        })
        .collect()
}

pub(crate) fn mark_metadata_dirty(
    transaction: &Transaction<'_>,
    id: BookId,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT OR IGNORE INTO metadata_dirtied(book) VALUES (?1)",
        [id.get()],
    )?;
    transaction.execute(
        "UPDATE books SET last_modified = strftime('%Y-%m-%d %H:%M:%f+00:00', 'now') \
         WHERE id = ?1",
        [id.get()],
    )?;
    Ok(())
}

pub(crate) fn replace_many_to_many(
    transaction: &Transaction<'_>,
    book: BookId,
    values: &[String],
    value_table: &'static str,
    value_column: &'static str,
    link_table: &'static str,
    link_column: &'static str,
) -> rusqlite::Result<()> {
    debug_assert!(matches!(
        (value_table, value_column, link_table, link_column),
        ("authors", "name", "books_authors_link", "author")
            | ("tags", "name", "books_tags_link", "tag")
            | (
                "languages",
                "lang_code",
                "books_languages_link",
                "lang_code"
            )
    ));
    transaction.execute(
        &format!("DELETE FROM {link_table} WHERE book = ?1"),
        [book.get()],
    )?;
    let mut seen = HashSet::new();
    for (order, value) in values.iter().enumerate() {
        let normalized = value.trim();
        if normalized.is_empty() || !seen.insert(normalized.to_lowercase()) {
            continue;
        }
        let stored = if value_table == "authors" {
            normalized.replace(',', "|")
        } else {
            normalized.to_owned()
        };
        let sort = if value_table == "authors" {
            Some(conservative_author_sort(normalized))
        } else {
            None
        };
        if value_table == "authors" {
            transaction.execute(
                "INSERT OR IGNORE INTO authors(name, sort) VALUES (?1, ?2)",
                (&stored, &sort),
            )?;
        } else {
            transaction.execute(
                &format!("INSERT OR IGNORE INTO {value_table}({value_column}) VALUES (?1)"),
                [&stored],
            )?;
        }
        let value_id: i64 = transaction.query_row(
            &format!("SELECT id FROM {value_table} WHERE {value_column} = ?1 COLLATE NOCASE"),
            [&stored],
            |row| row.get(0),
        )?;
        if link_table == "books_languages_link" {
            transaction.execute(
                "INSERT INTO books_languages_link(book, lang_code, item_order) \
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    book.get(),
                    value_id,
                    i64::try_from(order).unwrap_or(i64::MAX)
                ],
            )?;
        } else {
            transaction.execute(
                &format!("INSERT INTO {link_table}(book, {link_column}) VALUES (?1, ?2)"),
                (book.get(), value_id),
            )?;
        }
    }
    Ok(())
}

pub(crate) fn replace_many_to_one(
    transaction: &Transaction<'_>,
    book: BookId,
    value: Option<&str>,
    value_table: &'static str,
    value_column: &'static str,
    link_table: &'static str,
    link_column: &'static str,
) -> rusqlite::Result<()> {
    debug_assert!(matches!(
        (value_table, value_column, link_table, link_column),
        ("series", "name", "books_series_link", "series")
            | ("publishers", "name", "books_publishers_link", "publisher")
    ));
    transaction.execute(
        &format!("DELETE FROM {link_table} WHERE book = ?1"),
        [book.get()],
    )?;
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        let sort = conservative_title_sort(value);
        transaction.execute(
            &format!("INSERT OR IGNORE INTO {value_table}({value_column}, sort) VALUES (?1, ?2)"),
            (value, sort),
        )?;
        let value_id: i64 = transaction.query_row(
            &format!("SELECT id FROM {value_table} WHERE {value_column} = ?1 COLLATE NOCASE"),
            [value],
            |row| row.get(0),
        )?;
        transaction.execute(
            &format!("INSERT INTO {link_table}(book, {link_column}) VALUES (?1, ?2)"),
            (book.get(), value_id),
        )?;
    }
    Ok(())
}

pub(crate) fn replace_identifiers(
    transaction: &Transaction<'_>,
    book: BookId,
    values: &BTreeMap<String, String>,
) -> rusqlite::Result<()> {
    transaction.execute("DELETE FROM identifiers WHERE book = ?1", [book.get()])?;
    for (kind, value) in values {
        let kind = kind.trim().to_ascii_lowercase();
        let value = value.trim();
        if !kind.is_empty() && !value.is_empty() {
            transaction.execute(
                "INSERT INTO identifiers(book, type, val) VALUES (?1, ?2, ?3)",
                rusqlite::params![book.get(), kind, value],
            )?;
        }
    }
    Ok(())
}

pub(crate) fn replace_comments(
    transaction: &Transaction<'_>,
    book: BookId,
    value: Option<&str>,
) -> rusqlite::Result<()> {
    transaction.execute("DELETE FROM comments WHERE book = ?1", [book.get()])?;
    if let Some(text) = value.filter(|text| !text.is_empty()) {
        transaction.execute(
            "INSERT INTO comments(book, text) VALUES (?1, ?2)",
            (book.get(), text),
        )?;
    }
    Ok(())
}

pub(crate) fn replace_rating(
    transaction: &Transaction<'_>,
    book: BookId,
    value: Option<Rating>,
) -> rusqlite::Result<()> {
    transaction.execute(
        "DELETE FROM books_ratings_link WHERE book = ?1",
        [book.get()],
    )?;
    if let Some(rating) = value {
        transaction.execute(
            "INSERT OR IGNORE INTO ratings(rating) VALUES (?1)",
            [rating.get()],
        )?;
        let rating_id: i64 = transaction.query_row(
            "SELECT id FROM ratings WHERE rating = ?1",
            [rating.get()],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO books_ratings_link(book, rating) VALUES (?1, ?2)",
            (book.get(), rating_id),
        )?;
    }
    Ok(())
}

pub(crate) fn conservative_author_sort(author: &str) -> String {
    let trimmed = author.trim();
    if let Some((first, last)) = trimmed.rsplit_once(' ') {
        format!("{last}, {first}")
    } else {
        trimmed.to_owned()
    }
}
