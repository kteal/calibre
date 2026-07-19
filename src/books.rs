use crate::library::{LibraryInner, database_error};
use crate::model::{
    Book, BookPage, BookQuery, BookSort, DeletionMode, NewBook, SortDirection, UpdateBook,
};
use crate::{BookId, Error, Result};
use rusqlite::{TransactionBehavior, params};
use std::fs;
use std::path::Path;
use std::sync::Arc;

/// Operations on book records and core metadata.
#[derive(Clone, Debug)]
pub struct Books {
    inner: Arc<LibraryInner>,
}

impl Books {
    pub(crate) const fn new(inner: Arc<LibraryInner>) -> Self {
        Self { inner }
    }

    /// Lists books using filtering, stable sorting, and offset pagination.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid offset, database failure, missing
    /// related schema, or unsafe path stored in a matching book.
    #[allow(clippy::needless_pass_by_value)] // Query values are cheap request objects.
    pub fn query(&self, query: BookQuery) -> Result<BookPage> {
        let connection = self.inner.read_connection()?;
        let pattern = query.title_contains.as_deref().map(escape_like);
        let total: i64 = if let Some(pattern) = &pattern {
            connection
                .query_row(
                    "SELECT count(*) FROM books WHERE title LIKE ?1 ESCAPE '\\' COLLATE NOCASE",
                    [format!("%{pattern}%")],
                    |row| row.get(0),
                )
                .map_err(|error| database_error("count books", &self.inner.database, error))?
        } else {
            connection
                .query_row("SELECT count(*) FROM books", [], |row| row.get(0))
                .map_err(|error| database_error("count books", &self.inner.database, error))?
        };
        let sort = match query.sort {
            BookSort::Title => "sort COLLATE NOCASE",
            BookSort::Author => "author_sort COLLATE NOCASE",
            BookSort::Timestamp => "timestamp",
            BookSort::LastModified => "last_modified",
            BookSort::Id => "id",
        };
        let direction = match query.direction {
            SortDirection::Ascending => "ASC",
            SortDirection::Descending => "DESC",
        };
        let sql = if pattern.is_some() {
            format!(
                "SELECT id FROM books WHERE title LIKE ?1 ESCAPE '\\' COLLATE NOCASE \
                 ORDER BY {sort} {direction}, id {direction} LIMIT ?2 OFFSET ?3"
            )
        } else {
            format!(
                "SELECT id FROM books ORDER BY {sort} {direction}, id {direction} \
                 LIMIT ?1 OFFSET ?2"
            )
        };
        let limit = i64::from(query.page.limit);
        let offset = i64::try_from(query.page.offset).map_err(|_| Error::InvalidInput {
            field: "page offset",
            reason: "offset exceeds SQLite's signed integer range".into(),
        })?;
        let ids = {
            let mut statement = connection.prepare(&sql).map_err(|error| {
                database_error("prepare book query", &self.inner.database, error)
            })?;
            if let Some(pattern) = pattern {
                statement
                    .query_map(params![format!("%{pattern}%"), limit, offset], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(|error| database_error("query books", &self.inner.database, error))?
                    .collect::<std::result::Result<Vec<_>, _>>()
            } else {
                statement
                    .query_map(params![limit, offset], |row| row.get::<_, i64>(0))
                    .map_err(|error| database_error("query books", &self.inner.database, error))?
                    .collect::<std::result::Result<Vec<_>, _>>()
            }
            .map_err(|error| database_error("query books", &self.inner.database, error))?
        };
        let items = ids
            .into_iter()
            .map(|id| crate::sql::load_book(&self.inner, &connection, BookId::new(id)))
            .collect::<Result<Vec<_>>>()?;
        Ok(BookPage {
            items,
            total: u64::try_from(total).unwrap_or_default(),
            offset: query.page.offset,
            limit: query.page.limit,
        })
    }

    /// Retrieves one book with its core relationships and assets.
    ///
    /// # Errors
    ///
    /// Returns an error when the book is missing, the database cannot be read,
    /// or a stored asset path is unsafe.
    pub fn get(&self, id: BookId) -> Result<Book> {
        let connection = self.inner.read_connection()?;
        crate::sql::load_book(&self.inner, &connection, id)
    }

    /// Adds a book and optional format and cover files.
    ///
    /// Files are copied into a newly created book directory before the `SQLite`
    /// transaction commits. A failure removes that directory.
    ///
    /// # Errors
    ///
    /// Returns an error for read-only mode, invalid metadata or files,
    /// database failure, unsafe paths, or filesystem staging failure.
    #[allow(clippy::needless_pass_by_value)] // Creation inputs intentionally transfer ownership.
    #[allow(clippy::too_many_lines)] // The transaction boundary keeps compensation visible.
    pub fn add(&self, input: NewBook) -> Result<Book> {
        let _guard = self.inner.lock_writer("add book")?;
        let mut connection = self.inner.write_connection("add book")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                database_error("begin add-book transaction", &self.inner.database, error)
            })?;

        let title = nonempty_or(&input.title, "Unknown");
        let authors = clean_values(&input.authors, "Unknown");
        validate_languages(&input.languages)?;
        let first_author = &authors[0];
        let title_sort = crate::sql::conservative_title_sort(&title);
        let author_sort = authors
            .iter()
            .map(|author| crate::sql::conservative_author_sort(author))
            .collect::<Vec<_>>()
            .join(" & ");
        transaction
            .execute(
                "INSERT INTO books(title, sort, series_index, author_sort, path, uuid) \
                 VALUES (?1, ?2, ?3, ?4, '', uuid4())",
                params![
                    title,
                    title_sort,
                    if input.series_index == 0.0 {
                        1.0
                    } else {
                        input.series_index
                    },
                    author_sort
                ],
            )
            .map_err(|error| database_error("insert book", &self.inner.database, error))?;
        let id = BookId::new(transaction.last_insert_rowid());
        let relative_path = crate::paths::book_relative_path(&title, first_author, id.get());
        let directory = crate::paths::resolve(&self.inner.root, &relative_path)?;
        if directory.exists() {
            return Err(Error::InvalidInput {
                field: "book path",
                reason: format!("destination already exists: {}", directory.display()),
            });
        }

        let result = (|| -> Result<()> {
            let parent = directory.parent().ok_or_else(|| Error::PathEscape {
                path: relative_path.clone(),
                reason: "book path has no parent".into(),
            })?;
            fs::create_dir_all(parent).map_err(|source| {
                crate::error::io_error("create author directory", parent, source)
            })?;
            fs::create_dir(&directory).map_err(|source| {
                crate::error::io_error("create book directory", &directory, source)
            })?;

            crate::sql::replace_many_to_many(
                &transaction,
                id,
                &authors,
                "authors",
                "name",
                "books_authors_link",
                "author",
            )
            .map_err(|error| database_error("set book authors", &self.inner.database, error))?;
            crate::sql::replace_many_to_many(
                &transaction,
                id,
                &input.tags,
                "tags",
                "name",
                "books_tags_link",
                "tag",
            )
            .map_err(|error| database_error("set book tags", &self.inner.database, error))?;
            crate::sql::replace_many_to_many(
                &transaction,
                id,
                &input.languages,
                "languages",
                "lang_code",
                "books_languages_link",
                "lang_code",
            )
            .map_err(|error| database_error("set book languages", &self.inner.database, error))?;
            crate::sql::replace_many_to_one(
                &transaction,
                id,
                input.series.as_deref(),
                "series",
                "name",
                "books_series_link",
                "series",
            )
            .map_err(|error| database_error("set book series", &self.inner.database, error))?;
            crate::sql::replace_many_to_one(
                &transaction,
                id,
                input.publisher.as_deref(),
                "publishers",
                "name",
                "books_publishers_link",
                "publisher",
            )
            .map_err(|error| database_error("set book publisher", &self.inner.database, error))?;
            crate::sql::replace_identifiers(&transaction, id, &input.identifiers).map_err(
                |error| database_error("set book identifiers", &self.inner.database, error),
            )?;
            crate::sql::replace_comments(&transaction, id, input.comments.as_deref()).map_err(
                |error| database_error("set book comments", &self.inner.database, error),
            )?;
            crate::sql::replace_rating(&transaction, id, input.rating)
                .map_err(|error| database_error("set book rating", &self.inner.database, error))?;

            let stem = crate::paths::format_stem(&title, first_author);
            let mut seen_formats = std::collections::HashSet::new();
            for source in &input.formats {
                let format = crate::paths::format_from_path(source.path())?;
                if !seen_formats.insert(format.clone()) {
                    return Err(Error::InvalidInput {
                        field: "formats",
                        reason: format!("duplicate format {format}"),
                    });
                }
                let size = copy_new_asset(
                    source.path(),
                    &directory.join(format!("{stem}.{}", format.to_ascii_lowercase())),
                )?;
                transaction
                    .execute(
                        "INSERT INTO data(book, format, uncompressed_size, name) \
                         VALUES (?1, ?2, ?3, ?4)",
                        params![id.get(), format, i64_size(size)?, stem],
                    )
                    .map_err(|error| {
                        database_error("insert book format", &self.inner.database, error)
                    })?;
            }
            if let Some(source) = &input.cover {
                copy_new_asset(source, &directory.join("cover.jpg"))?;
                transaction
                    .execute("UPDATE books SET has_cover = 1 WHERE id = ?1", [id.get()])
                    .map_err(|error| {
                        database_error("set book cover flag", &self.inner.database, error)
                    })?;
            }
            transaction
                .execute(
                    "UPDATE books SET path = ?1 WHERE id = ?2",
                    params![path_to_database(&relative_path)?, id.get()],
                )
                .map_err(|error| database_error("set book path", &self.inner.database, error))?;
            crate::sql::mark_metadata_dirty(&transaction, id).map_err(|error| {
                database_error("mark metadata dirty", &self.inner.database, error)
            })?;
            transaction
                .commit()
                .map_err(|error| database_error("commit add book", &self.inner.database, error))?;
            Ok(())
        })();

        if let Err(error) = result {
            if directory.exists() {
                let _ = fs::remove_dir_all(&directory);
            }
            remove_empty_parent(&directory, &self.inner.root);
            return Err(error);
        }
        self.get(id)
    }

    /// Updates core metadata. A title or first-author change moves the book
    /// directory and renames known format files with rollback compensation.
    ///
    /// # Errors
    ///
    /// Returns an error for read-only mode, a missing book, invalid metadata,
    /// unsafe paths, database failure, or filesystem compensation failure.
    #[allow(clippy::needless_pass_by_value)] // Update inputs are one-shot request values.
    #[allow(clippy::too_many_lines)] // One function owns the DB/filesystem compensation boundary.
    pub fn update(&self, id: BookId, update: UpdateBook) -> Result<Book> {
        let _guard = self.inner.lock_writer("update book")?;
        let existing = self.get(id)?;
        let title = update.title.as_deref().map_or_else(
            || existing.title.clone(),
            |value| nonempty_or(value, "Unknown"),
        );
        let authors = update.authors.as_ref().map_or_else(
            || {
                existing
                    .authors
                    .iter()
                    .map(|author| author.name.clone())
                    .collect()
            },
            |values| clean_values(values, "Unknown"),
        );
        if let Some(languages) = &update.languages {
            validate_languages(languages)?;
        }
        let new_relative = crate::paths::book_relative_path(&title, &authors[0], id.get());
        let old_directory = crate::paths::resolve(&self.inner.root, &existing.relative_path)?;
        let new_directory = crate::paths::resolve(&self.inner.root, &new_relative)?;
        let should_move = existing.relative_path != new_relative;
        if should_move && !old_directory.is_dir() {
            return Err(Error::UnsupportedOperation {
                operation: "update book path",
                reason: "the existing book directory is missing".into(),
            });
        }
        if should_move && new_directory.exists() {
            return Err(Error::InvalidInput {
                field: "book path",
                reason: "the destination book directory already exists".into(),
            });
        }

        let mut connection = self.inner.write_connection("update book")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                database_error("begin update-book transaction", &self.inner.database, error)
            })?;
        let mut moved_directory = false;
        let mut renamed_files = Vec::new();
        let result = (|| -> Result<()> {
            if update.title.is_some() {
                transaction
                    .execute(
                        "UPDATE books SET title = ?1 WHERE id = ?2",
                        params![title, id.get()],
                    )
                    .map_err(|error| {
                        database_error("update book title", &self.inner.database, error)
                    })?;
            }
            if update.authors.is_some() {
                crate::sql::replace_many_to_many(
                    &transaction,
                    id,
                    &authors,
                    "authors",
                    "name",
                    "books_authors_link",
                    "author",
                )
                .map_err(|error| database_error("update authors", &self.inner.database, error))?;
                let author_sort = authors
                    .iter()
                    .map(|author| crate::sql::conservative_author_sort(author))
                    .collect::<Vec<_>>()
                    .join(" & ");
                transaction
                    .execute(
                        "UPDATE books SET author_sort = ?1 WHERE id = ?2",
                        params![author_sort, id.get()],
                    )
                    .map_err(|error| {
                        database_error("update author sort", &self.inner.database, error)
                    })?;
            }
            if let Some(tags) = &update.tags {
                crate::sql::replace_many_to_many(
                    &transaction,
                    id,
                    tags,
                    "tags",
                    "name",
                    "books_tags_link",
                    "tag",
                )
                .map_err(|error| database_error("update tags", &self.inner.database, error))?;
            }
            if let Some(languages) = &update.languages {
                crate::sql::replace_many_to_many(
                    &transaction,
                    id,
                    languages,
                    "languages",
                    "lang_code",
                    "books_languages_link",
                    "lang_code",
                )
                .map_err(|error| database_error("update languages", &self.inner.database, error))?;
            }
            if let Some(series) = &update.series {
                crate::sql::replace_many_to_one(
                    &transaction,
                    id,
                    series.as_deref(),
                    "series",
                    "name",
                    "books_series_link",
                    "series",
                )
                .map_err(|error| database_error("update series", &self.inner.database, error))?;
            }
            if let Some(index) = update.series_index {
                transaction
                    .execute(
                        "UPDATE books SET series_index = ?1 WHERE id = ?2",
                        params![index, id.get()],
                    )
                    .map_err(|error| {
                        database_error("update series index", &self.inner.database, error)
                    })?;
            }
            if let Some(publisher) = &update.publisher {
                crate::sql::replace_many_to_one(
                    &transaction,
                    id,
                    publisher.as_deref(),
                    "publishers",
                    "name",
                    "books_publishers_link",
                    "publisher",
                )
                .map_err(|error| database_error("update publisher", &self.inner.database, error))?;
            }
            if let Some(identifiers) = &update.identifiers {
                crate::sql::replace_identifiers(&transaction, id, identifiers).map_err(
                    |error| database_error("update identifiers", &self.inner.database, error),
                )?;
            }
            if let Some(comments) = &update.comments {
                crate::sql::replace_comments(&transaction, id, comments.as_deref()).map_err(
                    |error| database_error("update comments", &self.inner.database, error),
                )?;
            }
            if let Some(rating) = update.rating {
                crate::sql::replace_rating(&transaction, id, rating).map_err(|error| {
                    database_error("update rating", &self.inner.database, error)
                })?;
            }

            if should_move {
                let new_parent = new_directory.parent().ok_or_else(|| Error::PathEscape {
                    path: new_relative.clone(),
                    reason: "book path has no parent".into(),
                })?;
                fs::create_dir_all(new_parent).map_err(|source| {
                    crate::error::io_error(
                        "create destination author directory",
                        new_parent,
                        source,
                    )
                })?;
                fs::rename(&old_directory, &new_directory).map_err(|source| {
                    crate::error::io_error("move book directory", &old_directory, source)
                })?;
                moved_directory = true;
                let new_stem = crate::paths::format_stem(&title, &authors[0]);
                for format in &existing.formats {
                    let old_name = format.path.file_name().ok_or_else(|| Error::InvalidInput {
                        field: "format path",
                        reason: "existing format has no filename".into(),
                    })?;
                    let moved_old = new_directory.join(old_name);
                    if !moved_old.exists() {
                        continue;
                    }
                    let renamed = new_directory
                        .join(format!("{new_stem}.{}", format.format.to_ascii_lowercase()));
                    if moved_old != renamed {
                        fs::rename(&moved_old, &renamed).map_err(|source| {
                            crate::error::io_error("rename moved format", &moved_old, source)
                        })?;
                        renamed_files.push((renamed, moved_old));
                    }
                }
                transaction
                    .execute(
                        "UPDATE data SET name = ?1 WHERE book = ?2",
                        params![new_stem, id.get()],
                    )
                    .map_err(|error| {
                        database_error("update format names", &self.inner.database, error)
                    })?;
                transaction
                    .execute(
                        "UPDATE books SET path = ?1 WHERE id = ?2",
                        params![path_to_database(&new_relative)?, id.get()],
                    )
                    .map_err(|error| {
                        database_error("update book path", &self.inner.database, error)
                    })?;
            }
            crate::sql::mark_metadata_dirty(&transaction, id).map_err(|error| {
                database_error("mark metadata dirty", &self.inner.database, error)
            })?;
            transaction
                .commit()
                .map_err(|error| database_error("commit update book", &self.inner.database, error))
        })();

        if let Err(error) = result {
            for (renamed, old) in renamed_files.into_iter().rev() {
                let _ = fs::rename(renamed, old);
            }
            if moved_directory {
                let _ = fs::rename(&new_directory, &old_directory);
            }
            return Err(error);
        }
        if should_move {
            remove_empty_parent(&old_directory, &self.inner.root);
        }
        self.get(id)
    }

    /// Removes a book with explicit deletion semantics.
    ///
    /// # Errors
    ///
    /// Returns an error when trash was requested, deferred Calibre state makes
    /// deletion unsafe, the book is missing, or database/filesystem work fails.
    pub fn remove(&self, id: BookId, mode: DeletionMode) -> Result<()> {
        if mode == DeletionMode::Trash {
            return Err(Error::UnsupportedOperation {
                operation: "move book to Calibre trash",
                reason: "Calibre-compatible OPF-backed trash restoration is not implemented".into(),
            });
        }
        if !self.inner.capabilities.permanent_delete {
            return Err(Error::UnsupportedOperation {
                operation: "permanently delete book",
                reason: "active custom-column or full-text-search state requires Calibre-specific cleanup".into(),
            });
        }
        let _guard = self.inner.lock_writer("permanently delete book")?;
        let existing = self.get(id)?;
        let directory = crate::paths::resolve(&self.inner.root, &existing.relative_path)?;
        let staged = self
            .inner
            .root
            .join(format!(".calibre-rs-delete-{}", uuid::Uuid::new_v4()));
        let staged_directory = if directory.exists() {
            fs::rename(&directory, &staged).map_err(|source| {
                crate::error::io_error("stage book deletion", &directory, source)
            })?;
            true
        } else {
            false
        };
        let mut connection = self.inner.write_connection("permanently delete book")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                database_error("begin book-delete transaction", &self.inner.database, error)
            })?;
        let result = transaction
            .execute("DELETE FROM books WHERE id = ?1", [id.get()])
            .and_then(|changed| {
                if changed == 0 {
                    Err(rusqlite::Error::QueryReturnedNoRows)
                } else {
                    Ok(())
                }
            })
            .and_then(|()| transaction.commit())
            .map_err(|error| database_error("delete book", &self.inner.database, error));
        if let Err(error) = result {
            if staged_directory {
                let _ = fs::rename(&staged, &directory);
            }
            return Err(error);
        }
        if staged_directory {
            fs::remove_dir_all(&staged)
                .map_err(|source| crate::error::io_error("remove staged book", &staged, source))?;
        }
        remove_empty_parent(&directory, &self.inner.root);
        Ok(())
    }
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn nonempty_or(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn clean_values(values: &[String], fallback: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let result = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.to_lowercase()))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if result.is_empty() {
        vec![fallback.to_owned()]
    } else {
        result
    }
}

fn validate_languages(languages: &[String]) -> Result<()> {
    for language in languages {
        if language.len() != 3 || !language.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            return Err(Error::InvalidInput {
                field: "languages",
                reason: format!(
                    "{language:?} is not a three-letter ISO 639 code stored by Calibre"
                ),
            });
        }
    }
    Ok(())
}

fn path_to_database(path: &Path) -> Result<String> {
    let mut components = Vec::new();
    for component in path.components() {
        let std::path::Component::Normal(value) = component else {
            return Err(Error::PathEscape {
                path: path.to_path_buf(),
                reason: "book path contains a non-normal component".into(),
            });
        };
        let value = value.to_str().ok_or_else(|| Error::InvalidInput {
            field: "generated book path",
            reason: "Calibre metadata.db paths must be valid UTF-8".into(),
        })?;
        components.push(value);
    }
    Ok(components.join("/"))
}

pub(crate) fn copy_new_asset(source: &Path, destination: &Path) -> Result<u64> {
    let metadata = fs::metadata(source)
        .map_err(|error| crate::error::io_error("inspect source asset", source, error))?;
    if !metadata.is_file() {
        return Err(Error::InvalidInput {
            field: "asset path",
            reason: format!("{} is not a regular file", source.display()),
        });
    }
    if destination.exists() {
        return Err(Error::InvalidInput {
            field: "asset destination",
            reason: format!("{} already exists", destination.display()),
        });
    }
    let parent = destination.parent().ok_or_else(|| Error::InvalidInput {
        field: "asset destination",
        reason: "destination has no parent directory".into(),
    })?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| crate::error::io_error("stage asset", parent, error))?;
    let mut input = fs::File::open(source)
        .map_err(|error| crate::error::io_error("open source asset", source, error))?;
    let size = std::io::copy(&mut input, staged.as_file_mut())
        .map_err(|error| crate::error::io_error("copy staged asset", destination, error))?;
    staged
        .as_file()
        .sync_all()
        .map_err(|error| crate::error::io_error("sync staged asset", destination, error))?;
    staged.persist(destination).map_err(|error| {
        crate::error::io_error("install staged asset", destination, error.error)
    })?;
    Ok(size)
}

pub(crate) fn i64_size(size: u64) -> Result<i64> {
    i64::try_from(size).map_err(|_| Error::InvalidInput {
        field: "format size",
        reason: "file is too large for Calibre's SQLite size column".into(),
    })
}

fn remove_empty_parent(book_directory: &Path, root: &Path) {
    if let Some(parent) = book_directory.parent().filter(|parent| *parent != root) {
        let _ = fs::remove_dir(parent);
    }
}
