use crate::library::{LibraryInner, database_error};
use crate::recovery::{RecoveryJournal, TrashAssetKind, TrashDirection};
use crate::{Book, BookId, Error, Format, Result};
use rusqlite::{Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const TRASH_DIRECTORY: &str = ".caltrash";
const BOOK_CATEGORY: &str = "b";
const FORMAT_CATEGORY: &str = "f";

/// Calibre's default age for automatic trash expiry.
pub const DEFAULT_TRASH_EXPIRY: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// A category in Calibre's per-library trash.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TrashEntryKind {
    /// A complete deleted book directory.
    Book,
    /// One or more deleted formats for a live book.
    Formats,
}

/// A visible entry in Calibre's per-library trash.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct TrashEntry {
    /// Original book ID.
    pub book_id: BookId,
    /// Trash category.
    pub kind: TrashEntryKind,
    /// Display title captured when the entry was removed.
    pub title: String,
    /// Ordered display authors captured when the entry was removed.
    pub authors: Vec<String>,
    /// Modification time of the entry directory, used for expiry.
    pub modified: SystemTime,
    /// Available logical formats for a format-trash entry.
    pub formats: Vec<String>,
    /// Cover path for a trashed book or its still-live format owner.
    pub cover_path: Option<PathBuf>,
}

/// Current contents of both Calibre trash categories.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct TrashContents {
    /// Whole-book entries.
    pub books: Vec<TrashEntry>,
    /// Per-book format entries.
    pub formats: Vec<TrashEntry>,
}

/// Operations on Calibre's per-library trash.
#[derive(Clone, Debug)]
pub struct Trash {
    inner: Arc<LibraryInner>,
}

impl Trash {
    pub(crate) const fn new(inner: Arc<LibraryInner>) -> Self {
        Self { inner }
    }

    /// Lists book and format trash without creating directories.
    ///
    /// Malformed whole-book entries without readable core OPF metadata are
    /// skipped, matching Calibre's listing behavior.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe trash paths or filesystem failures.
    pub fn list(&self) -> Result<TrashContents> {
        Ok(TrashContents {
            books: self.list_books()?,
            formats: self.list_formats()?,
        })
    }

    /// Returns the path of a whole-book trash entry.
    ///
    /// # Errors
    ///
    /// Returns an error when the entry is missing or is not a regular,
    /// non-symlink directory.
    pub fn book_path(&self, book: BookId) -> Result<PathBuf> {
        ensure_category(&self.inner.root, TrashEntryKind::Book)?;
        let path = trash_entry_path(&self.inner.root, TrashEntryKind::Book, book)?;
        ensure_directory(&path, "resolve trashed book")?;
        Ok(path)
    }

    /// Returns the path of a trashed format.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid format, missing entry, or unsafe file.
    pub fn format_path(&self, book: BookId, format: impl AsRef<OsStr>) -> Result<PathBuf> {
        let format = crate::paths::format_name(format.as_ref())?;
        ensure_category(&self.inner.root, TrashEntryKind::Formats)?;
        let path = format_trash_path(&self.inner.root, book, &format)?;
        let parent = path.parent().ok_or_else(|| Error::PathEscape {
            path: path.clone(),
            reason: "trashed format has no parent directory".into(),
        })?;
        ensure_directory(parent, "resolve trashed format")?;
        ensure_file(&path, "resolve trashed format")?;
        Ok(path)
    }

    /// Reads a trashed format into memory without restoring it.
    ///
    /// # Errors
    ///
    /// Returns an error when path resolution or reading fails.
    pub fn read_format(&self, book: BookId, format: impl AsRef<OsStr>) -> Result<Vec<u8>> {
        let path = self.format_path(book, format)?;
        fs::read(&path)
            .map_err(|source| crate::error::io_error("read trashed format", path, source))
    }

    /// Copies a trashed format without restoring it.
    ///
    /// # Errors
    ///
    /// Returns an error when reading or writing fails.
    pub fn copy_format_to(
        &self,
        book: BookId,
        format: impl AsRef<OsStr>,
        destination: impl AsRef<Path>,
    ) -> Result<u64> {
        let source = self.format_path(book, format)?;
        if source == destination.as_ref() {
            return Err(Error::InvalidInput {
                field: "trash copy destination",
                reason: "destination is the trashed format itself".into(),
            });
        }
        let mut input = fs::File::open(&source)
            .map_err(|error| crate::error::io_error("open trashed format", &source, error))?;
        let destination = destination.as_ref();
        let mut output = fs::File::create(destination).map_err(|error| {
            crate::error::io_error("create trash copy destination", destination, error)
        })?;
        std::io::copy(&mut input, &mut output)
            .map_err(|error| crate::error::io_error("copy format from trash", destination, error))
    }

    /// Copies a whole-book trash tree without restoring database state.
    ///
    /// The destination must not already exist. Symlinks in the source tree are
    /// rejected.
    ///
    /// # Errors
    ///
    /// Returns an error for an existing destination, unsafe source, or I/O
    /// failure.
    pub fn copy_book_to(&self, book: BookId, destination: impl AsRef<Path>) -> Result<()> {
        let source = self.book_path(book)?;
        let destination = destination.as_ref();
        if destination.starts_with(&source) {
            return Err(Error::InvalidInput {
                field: "trash copy destination",
                reason: "destination cannot be inside the trashed book".into(),
            });
        }
        if destination.exists() {
            return Err(Error::InvalidInput {
                field: "trash copy destination",
                reason: format!("{} already exists", destination.display()),
            });
        }
        copy_tree(&source, destination)
    }

    /// Restores a whole book with its original ID and core metadata.
    ///
    /// The operation refuses an ID or filesystem collision and OPF state that
    /// this crate cannot preserve.
    ///
    /// # Errors
    ///
    /// Returns an error for read-only mode, unsupported deferred metadata,
    /// conflicts, invalid OPF, or database/filesystem failure.
    pub fn restore_book(&self, book: BookId) -> Result<Book> {
        restore_book(&self.inner, book)
    }

    /// Restores a format to its live book, replacing an active format.
    ///
    /// # Errors
    ///
    /// Returns an error when the book or trash entry is missing, writes are
    /// disabled, or database/filesystem work fails.
    pub fn restore_format(&self, book: BookId, format: impl AsRef<OsStr>) -> Result<Format> {
        let format = crate::paths::format_name(format.as_ref())?;
        let source = self.format_path(book, OsStr::new(&format))?;
        let mut input = fs::File::open(&source)
            .map_err(|error| crate::error::io_error("open trashed format", &source, error))?;
        let restored = crate::Formats::new(Arc::clone(&self.inner)).replace_from_reader(
            book,
            OsStr::new(&format),
            &mut input,
        )?;
        drop(input);

        let _guard = self
            .inner
            .lock_writer("remove restored format from trash")?;
        let _connection = self
            .inner
            .write_connection("remove restored format from trash")?;
        ensure_file(&source, "remove restored format from trash")?;
        fs::remove_file(&source).map_err(|error| {
            crate::error::io_error("remove restored format from trash", &source, error)
        })?;
        remove_empty_format_entry(source.parent().ok_or_else(|| Error::PathEscape {
            path: source.clone(),
            reason: "trashed format has no parent directory".into(),
        })?)?;
        Ok(restored)
    }

    /// Permanently deletes one trash entry.
    ///
    /// Returns `false` when the entry does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error in read-only mode or for unsafe paths and I/O failures.
    pub fn delete_entry(&self, book: BookId, kind: TrashEntryKind) -> Result<bool> {
        let _guard = self.inner.lock_writer("delete trash entry")?;
        let _connection = self.inner.write_connection("delete trash entry")?;
        delete_entry_path(&self.inner.root, book, kind)
    }

    /// Deletes trash entries whose directory mtime is at least `age` old.
    ///
    /// An age of zero empties both trash categories.
    ///
    /// # Errors
    ///
    /// Returns an error in read-only mode or for unsafe paths and I/O failures.
    pub fn expire_older_than(&self, age: Duration) -> Result<u64> {
        let _guard = self.inner.lock_writer("expire trash")?;
        let _connection = self.inner.write_connection("expire trash")?;
        let now = SystemTime::now();
        let mut removed = 0_u64;
        for kind in [TrashEntryKind::Book, TrashEntryKind::Formats] {
            for (book, path, metadata) in numeric_entries(&self.inner.root, kind)? {
                let modified = metadata.modified().map_err(|source| {
                    crate::error::io_error("read trash entry mtime", &path, source)
                })?;
                if now.duration_since(modified).unwrap_or_default() >= age
                    && delete_entry_path(&self.inner.root, book, kind)?
                {
                    removed = removed.saturating_add(1);
                }
            }
        }
        Ok(removed)
    }

    /// Expires entries using Calibre's default fourteen-day age.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::expire_older_than`].
    pub fn expire_default(&self) -> Result<u64> {
        self.expire_older_than(DEFAULT_TRASH_EXPIRY)
    }

    /// Permanently empties both trash categories.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::expire_older_than`].
    pub fn clear(&self) -> Result<u64> {
        self.expire_older_than(Duration::ZERO)
    }

    fn list_books(&self) -> Result<Vec<TrashEntry>> {
        let mut result = Vec::new();
        for (book, path, metadata) in numeric_entries(&self.inner.root, TrashEntryKind::Book)? {
            let opf_path = path.join("metadata.opf");
            if !regular_file_optional(&opf_path, "inspect trash metadata.opf")? {
                continue;
            }
            let Ok(book_metadata) = crate::opf::read(&opf_path, book) else {
                continue;
            };
            let cover = path.join("cover.jpg");
            let cover_path = match fs::symlink_metadata(&cover) {
                Ok(file) if file.is_file() && !file.file_type().is_symlink() => Some(cover),
                Ok(_) => {
                    return Err(Error::InvalidLibrary {
                        path: cover,
                        reason: "trash cover is not a regular non-symlink file".into(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(source) => {
                    return Err(crate::error::io_error("inspect trash cover", cover, source));
                }
            };
            result.push(TrashEntry {
                book_id: book,
                kind: TrashEntryKind::Book,
                title: book_metadata.title,
                authors: book_metadata.authors,
                modified: metadata.modified().map_err(|source| {
                    crate::error::io_error("read trash entry mtime", &path, source)
                })?,
                formats: Vec::new(),
                cover_path,
            });
        }
        Ok(result)
    }

    fn list_formats(&self) -> Result<Vec<TrashEntry>> {
        let mut result = Vec::new();
        for (book, path, metadata) in numeric_entries(&self.inner.root, TrashEntryKind::Formats)? {
            let listing = read_format_metadata(&path)?.unwrap_or_else(|| FormatListingMetadata {
                title: "Unknown".into(),
                authors: vec!["Unknown".into()],
            });
            let mut formats = Vec::new();
            for entry in fs::read_dir(&path).map_err(|source| {
                crate::error::io_error("read format trash entry", &path, source)
            })? {
                let entry = entry.map_err(|source| {
                    crate::error::io_error("read format trash entry", &path, source)
                })?;
                let file_name = entry.file_name();
                if file_name == "metadata.json" {
                    continue;
                }
                let file_metadata = fs::symlink_metadata(entry.path()).map_err(|source| {
                    crate::error::io_error("inspect trashed format", entry.path(), source)
                })?;
                if !file_metadata.is_file() || file_metadata.file_type().is_symlink() {
                    return Err(Error::InvalidLibrary {
                        path: entry.path(),
                        reason: "format trash contains a non-regular entry".into(),
                    });
                }
                formats.push(crate::paths::format_name(&file_name)?);
            }
            formats.sort_unstable();
            let cover_path = crate::Books::new(Arc::clone(&self.inner))
                .get(book)
                .ok()
                .and_then(|active| active.cover_path);
            result.push(TrashEntry {
                book_id: book,
                kind: TrashEntryKind::Formats,
                title: listing.title,
                authors: listing.authors,
                modified: metadata.modified().map_err(|source| {
                    crate::error::io_error("read trash entry mtime", &path, source)
                })?,
                formats,
                cover_path,
            });
        }
        Ok(result)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct FormatListingMetadata {
    title: String,
    authors: Vec<String>,
}

pub(crate) fn move_format_to_trash(
    inner: &Arc<LibraryInner>,
    book: BookId,
    format: &str,
) -> Result<()> {
    if !inner.capabilities.calibre_trash {
        return Err(Error::UnsupportedOperation {
            operation: "move format to Calibre trash",
            reason: "read-write mode and inactive full-text-search state are required".into(),
        });
    }
    let format = crate::paths::format_name(OsStr::new(format))?;
    let _guard = inner.lock_writer("move format to Calibre trash")?;
    let mut connection = inner.write_connection("move format to Calibre trash")?;
    let active = crate::sql::load_book(inner, &connection, book)?;
    let Some(existing) = active
        .formats
        .iter()
        .find(|item| item.format.eq_ignore_ascii_case(&format))
    else {
        return Ok(());
    };
    ensure_file(&existing.path, "move format to Calibre trash")?;
    let trash = format_trash_path(&inner.root, book, &format)?;
    ensure_trash_categories(&inner.root)?;
    let entry_directory = trash.parent().ok_or_else(|| Error::PathEscape {
        path: trash.clone(),
        reason: "format trash path has no parent".into(),
    })?;
    ensure_or_create_directory(entry_directory, "create format trash entry")?;
    let previous = private_sibling(&trash);
    let previous_exists = existing_trash_asset(&trash, false)?;
    let author_names = active
        .authors
        .iter()
        .map(|author| author.name.clone())
        .collect::<Vec<_>>();
    let journal = RecoveryJournal::begin_trash_change(
        &inner.root,
        book,
        &existing.path,
        &trash,
        previous_exists.then_some(previous.as_path()),
        TrashDirection::ToTrash,
        TrashAssetKind::Format {
            format: format.clone(),
        },
        Some((&active.title, &author_names)),
    )?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| {
            database_error("begin format-trash transaction", &inner.database, error)
        })?;
    if previous_exists {
        if let Err(error) = fs::rename(&trash, &previous) {
            journal.complete()?;
            return Err(crate::error::io_error(
                "stage previous trash entry",
                &trash,
                error,
            ));
        }
    }

    if let Err(error) = fs::rename(&existing.path, &trash) {
        restore_previous(&trash, &previous, previous_exists, false)?;
        journal.complete()?;
        return Err(crate::error::io_error(
            "move format to Calibre trash",
            &existing.path,
            error,
        ));
    }
    let result = transaction
        .execute(
            "DELETE FROM data WHERE book = ?1 AND format = ?2 COLLATE NOCASE",
            params![book.get(), format],
        )
        .and_then(|changed| {
            if changed == 0 {
                Err(rusqlite::Error::QueryReturnedNoRows)
            } else {
                Ok(())
            }
        })
        .and_then(|()| crate::formats::mark_format_changed(&transaction, book))
        .and_then(|()| transaction.commit())
        .map_err(|error| database_error("trash format", &inner.database, error));
    if let Err(error) = result {
        restore_live_and_previous(&existing.path, &trash, &previous, previous_exists, false)?;
        journal.complete()?;
        return Err(error);
    }
    write_format_metadata_values(entry_directory, &active.title, &author_names)?;
    remove_previous(&previous, previous_exists, false)?;
    journal.complete()
}

pub(crate) fn move_book_to_trash(inner: &Arc<LibraryInner>, book: BookId) -> Result<()> {
    if !inner.capabilities.calibre_trash {
        return Err(Error::UnsupportedOperation {
            operation: "move book to Calibre trash",
            reason: "read-write mode without custom columns or full-text-search state is required"
                .into(),
        });
    }
    let _guard = inner.lock_writer("move book to Calibre trash")?;
    let mut connection = inner.write_connection("move book to Calibre trash")?;
    ensure_no_deferred_book_state(&connection, inner, book)?;
    let active = crate::sql::load_book(inner, &connection, book)?;
    let live = crate::paths::resolve(&inner.root, &active.relative_path)?;
    ensure_directory(&live, "move book to Calibre trash")?;
    validate_tree(&live)?;
    crate::opf::write(&active, &live.join("metadata.opf"))?;
    ensure_trash_categories(&inner.root)?;
    let trash = trash_entry_path(&inner.root, TrashEntryKind::Book, book)?;
    let previous = private_sibling(&trash);
    let previous_exists = existing_trash_asset(&trash, true)?;
    let journal = RecoveryJournal::begin_trash_change(
        &inner.root,
        book,
        &live,
        &trash,
        previous_exists.then_some(previous.as_path()),
        TrashDirection::ToTrash,
        TrashAssetKind::Book,
        None,
    )?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| database_error("begin book-trash transaction", &inner.database, error))?;
    if previous_exists {
        if let Err(error) = fs::rename(&trash, &previous) {
            journal.complete()?;
            return Err(crate::error::io_error(
                "stage previous trash entry",
                &trash,
                error,
            ));
        }
    }
    if let Err(error) = fs::rename(&live, &trash) {
        restore_previous(&trash, &previous, previous_exists, true)?;
        journal.complete()?;
        return Err(crate::error::io_error(
            "move book to Calibre trash",
            &live,
            error,
        ));
    }
    let result = crate::sql::delete_core_book(&transaction, book)
        .and_then(|changed| {
            if changed {
                Ok(())
            } else {
                Err(rusqlite::Error::QueryReturnedNoRows)
            }
        })
        .and_then(|()| transaction.commit())
        .map_err(|error| database_error("trash book", &inner.database, error));
    if let Err(error) = result {
        restore_live_and_previous(&live, &trash, &previous, previous_exists, true)?;
        journal.complete()?;
        return Err(error);
    }
    remove_previous(&previous, previous_exists, true)?;
    journal.complete()?;
    remove_empty_parent(&live, &inner.root);
    Ok(())
}

#[allow(clippy::too_many_lines)] // The transaction mirrors every restored core field.
fn restore_book(inner: &Arc<LibraryInner>, book: BookId) -> Result<Book> {
    if !inner.capabilities.calibre_trash {
        return Err(Error::UnsupportedOperation {
            operation: "restore book from Calibre trash",
            reason: "read-write mode without custom columns or full-text-search state is required"
                .into(),
        });
    }
    ensure_category(&inner.root, TrashEntryKind::Book)?;
    let trash = trash_entry_path(&inner.root, TrashEntryKind::Book, book)?;
    ensure_directory(&trash, "restore book from Calibre trash")?;
    let metadata = crate::opf::read(&trash.join("metadata.opf"), book)?;
    let formats = scan_book_formats(&trash)?;
    let cover = regular_file_optional(&trash.join("cover.jpg"), "inspect trashed cover")?;
    let first_author = metadata.authors.first().map_or("Unknown", String::as_str);
    let relative = crate::paths::book_relative_path(&metadata.title, first_author, book.get());
    let live = crate::paths::resolve(&inner.root, &relative)?;

    let _guard = inner.lock_writer("restore book from Calibre trash")?;
    let mut connection = inner.write_connection("restore book from Calibre trash")?;
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM books WHERE id = ?1)",
            [book.get()],
            |row| row.get(0),
        )
        .map_err(|error| database_error("check restored book ID", &inner.database, error))?;
    if exists {
        return Err(Error::UnsupportedOperation {
            operation: "restore book from Calibre trash",
            reason: format!("a live book with ID {book} already exists"),
        });
    }
    if path_exists(&live)? {
        return Err(Error::UnsupportedOperation {
            operation: "restore book from Calibre trash",
            reason: format!("destination already exists: {}", live.display()),
        });
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| {
            database_error("begin restore-book transaction", &inner.database, error)
        })?;
    let journal = RecoveryJournal::begin_trash_change(
        &inner.root,
        book,
        &live,
        &trash,
        None,
        TrashDirection::FromTrash,
        TrashAssetKind::Book,
        None,
    )?;
    let parent = live.parent().ok_or_else(|| Error::PathEscape {
        path: live.clone(),
        reason: "restored book destination has no parent".into(),
    })?;
    fs::create_dir_all(parent)
        .map_err(|source| crate::error::io_error("create restored book parent", parent, source))?;
    if let Err(error) = fs::rename(&trash, &live) {
        journal.complete()?;
        return Err(crate::error::io_error(
            "restore book directory from trash",
            &trash,
            error,
        ));
    }

    let result = insert_restored_book(&transaction, inner, &metadata, &relative, &formats, cover)
        .and_then(|()| {
            transaction
                .commit()
                .map_err(|error| database_error("commit restored book", &inner.database, error))
        });
    if let Err(error) = result {
        fs::rename(&live, &trash).map_err(|source| {
            crate::error::io_error("roll back restored book directory", &live, source)
        })?;
        journal.complete()?;
        remove_empty_parent(&live, &inner.root);
        return Err(error);
    }
    journal.complete()?;
    crate::sql::load_book(inner, &connection, book)
}

#[allow(clippy::too_many_lines)] // Keeping all restored relationships in one transaction is auditable.
fn insert_restored_book(
    transaction: &Transaction<'_>,
    inner: &LibraryInner,
    metadata: &crate::opf::TrashBookMetadata,
    relative: &Path,
    formats: &[RestoredFormat],
    cover: bool,
) -> Result<()> {
    let author_sort = metadata.author_sort.clone().unwrap_or_else(|| {
        metadata
            .authors
            .iter()
            .map(|author| crate::sql::conservative_author_sort(author))
            .collect::<Vec<_>>()
            .join(" & ")
    });
    let sort = metadata
        .sort
        .clone()
        .unwrap_or_else(|| crate::sql::conservative_title_sort(&metadata.title));
    let uuid = metadata
        .uuid
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    transaction
        .execute(
            "INSERT INTO books(\
             id, title, sort, timestamp, pubdate, series_index, author_sort, path, uuid, \
             has_cover, last_modified) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, CURRENT_TIMESTAMP)",
            params![
                metadata.id.get(),
                metadata.title,
                sort,
                metadata.timestamp,
                metadata.publication_date,
                metadata.series_index,
                author_sort,
                crate::books::path_to_database(relative)?,
                uuid,
                cover
            ],
        )
        .map_err(|error| database_error("insert restored book", &inner.database, error))?;
    crate::sql::replace_many_to_many(
        transaction,
        metadata.id,
        &metadata.authors,
        "authors",
        "name",
        "books_authors_link",
        "author",
    )
    .map_err(|error| database_error("restore book authors", &inner.database, error))?;
    crate::sql::replace_many_to_many(
        transaction,
        metadata.id,
        &metadata.tags,
        "tags",
        "name",
        "books_tags_link",
        "tag",
    )
    .map_err(|error| database_error("restore book tags", &inner.database, error))?;
    crate::sql::replace_many_to_many(
        transaction,
        metadata.id,
        &metadata.languages,
        "languages",
        "lang_code",
        "books_languages_link",
        "lang_code",
    )
    .map_err(|error| database_error("restore book languages", &inner.database, error))?;
    crate::sql::replace_many_to_one(
        transaction,
        metadata.id,
        metadata.series.as_deref(),
        "series",
        "name",
        "books_series_link",
        "series",
    )
    .map_err(|error| database_error("restore book series", &inner.database, error))?;
    crate::sql::replace_many_to_one(
        transaction,
        metadata.id,
        metadata.publisher.as_deref(),
        "publishers",
        "name",
        "books_publishers_link",
        "publisher",
    )
    .map_err(|error| database_error("restore book publisher", &inner.database, error))?;
    crate::sql::replace_identifiers(transaction, metadata.id, &metadata.identifiers)
        .map_err(|error| database_error("restore book identifiers", &inner.database, error))?;
    crate::sql::replace_comments(transaction, metadata.id, metadata.comments.as_deref())
        .map_err(|error| database_error("restore book comments", &inner.database, error))?;
    crate::sql::replace_rating(transaction, metadata.id, metadata.rating)
        .map_err(|error| database_error("restore book rating", &inner.database, error))?;
    for format in formats {
        transaction
            .execute(
                "INSERT INTO data(book, format, uncompressed_size, name) VALUES (?1, ?2, ?3, ?4)",
                params![metadata.id.get(), format.format, format.size, format.stem],
            )
            .map_err(|error| database_error("restore book format", &inner.database, error))?;
    }
    transaction
        .execute(
            "INSERT OR REPLACE INTO books_pages_link(book, needs_scan) VALUES (?1, 1)",
            [metadata.id.get()],
        )
        .map_err(|error| database_error("restore page scan state", &inner.database, error))?;
    crate::sql::mark_metadata_dirty(transaction, metadata.id)
        .map_err(|error| database_error("mark restored metadata dirty", &inner.database, error))?;
    Ok(())
}

#[derive(Debug)]
struct RestoredFormat {
    format: String,
    size: i64,
    stem: String,
}

fn scan_book_formats(directory: &Path) -> Result<Vec<RestoredFormat>> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in fs::read_dir(directory)
        .map_err(|source| crate::error::io_error("scan trashed book", directory, source))?
    {
        let entry = entry
            .map_err(|source| crate::error::io_error("scan trashed book", directory, source))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|source| crate::error::io_error("inspect trashed book file", &path, source))?;
        if metadata.file_type().is_symlink() {
            return Err(Error::InvalidLibrary {
                path,
                reason: "trashed book contains a symlink".into(),
            });
        }
        if metadata.is_dir() {
            validate_tree(&path)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(Error::InvalidLibrary {
                path,
                reason: "trashed book contains a non-regular entry".into(),
            });
        }
        let name = entry.file_name();
        if name == "metadata.opf" || name == "cover.jpg" {
            continue;
        }
        let Some(extension) = path.extension() else {
            continue;
        };
        let format = crate::paths::format_name(extension)?;
        if !seen.insert(format.clone()) {
            return Err(Error::InvalidLibrary {
                path,
                reason: format!("trashed book contains duplicate {format} formats"),
            });
        }
        let stem = path
            .file_stem()
            .and_then(OsStr::to_str)
            .ok_or_else(|| Error::InvalidLibrary {
                path: path.clone(),
                reason: "trashed format stem is not UTF-8".into(),
            })?
            .to_owned();
        result.push(RestoredFormat {
            format,
            size: crate::books::i64_size(metadata.len())?,
            stem,
        });
    }
    result.sort_by(|left, right| left.format.cmp(&right.format));
    Ok(result)
}

pub(crate) fn ensure_no_deferred_book_state(
    connection: &rusqlite::Connection,
    inner: &LibraryInner,
    book: BookId,
) -> Result<()> {
    for table in [
        "annotations",
        "books_plugin_data",
        "conversion_options",
        "last_read_positions",
    ] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(|error| {
                database_error("inspect deferred book state", &inner.database, error)
            })?;
        if exists {
            let count: i64 = connection
                .query_row(
                    &format!("SELECT count(*) FROM {table} WHERE book = ?1"),
                    [book.get()],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    database_error("inspect deferred book state", &inner.database, error)
                })?;
            if count != 0 {
                return Err(Error::UnsupportedOperation {
                    operation: "move book to Calibre trash",
                    reason: format!(
                        "book {book} has {table} state that core OPF restoration cannot preserve"
                    ),
                });
            }
        }
    }
    Ok(())
}

fn ensure_trash_categories(root: &Path) -> Result<()> {
    let trash = crate::paths::resolve(root, Path::new(TRASH_DIRECTORY))?;
    ensure_or_create_directory(&trash, "create Calibre trash")?;
    ensure_or_create_directory(&trash.join(BOOK_CATEGORY), "create book trash")?;
    ensure_or_create_directory(&trash.join(FORMAT_CATEGORY), "create format trash")
}

fn ensure_or_create_directory(path: &Path, operation: &'static str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(Error::InvalidLibrary {
            path: path.to_path_buf(),
            reason: "trash path is not a regular directory".into(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|source| crate::error::io_error(operation, path, source))
        }
        Err(source) => Err(crate::error::io_error(operation, path, source)),
    }
}

pub(crate) fn trash_entry_path(root: &Path, kind: TrashEntryKind, book: BookId) -> Result<PathBuf> {
    if book.get() <= 0 {
        return Err(Error::InvalidInput {
            field: "book ID",
            reason: "trash book IDs must be positive".into(),
        });
    }
    let category = category_path(root, kind)?;
    let relative = category
        .strip_prefix(root)
        .map_err(|_| Error::PathEscape {
            path: category.clone(),
            reason: "trash category is outside the library root".into(),
        })?
        .join(book.get().to_string());
    let path = crate::paths::resolve(root, &relative)?;
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(Error::InvalidLibrary {
            path,
            reason: "trash entry is a symlink".into(),
        });
    }
    Ok(path)
}

pub(crate) fn format_trash_path(root: &Path, book: BookId, format: &str) -> Result<PathBuf> {
    let entry = trash_entry_path(root, TrashEntryKind::Formats, book)?;
    Ok(entry.join(format.to_ascii_lowercase()))
}

fn category_path(root: &Path, kind: TrashEntryKind) -> Result<PathBuf> {
    let trash = root.join(TRASH_DIRECTORY);
    match fs::symlink_metadata(&trash) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(Error::InvalidLibrary {
                path: trash,
                reason: "Calibre trash root is not a regular non-symlink directory".into(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(crate::error::io_error(
                "inspect Calibre trash root",
                trash,
                source,
            ));
        }
    }
    let category = root.join(TRASH_DIRECTORY).join(match kind {
        TrashEntryKind::Book => BOOK_CATEGORY,
        TrashEntryKind::Formats => FORMAT_CATEGORY,
    });
    match fs::symlink_metadata(&category) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(Error::InvalidLibrary {
                path: category,
                reason: "trash category is not a regular non-symlink directory".into(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(crate::error::io_error(
                "inspect Calibre trash category",
                category,
                source,
            ));
        }
    }
    Ok(category)
}

fn numeric_entries(
    root: &Path,
    kind: TrashEntryKind,
) -> Result<Vec<(BookId, PathBuf, fs::Metadata)>> {
    let category = category_path(root, kind)?;
    match fs::symlink_metadata(&category) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(Error::InvalidLibrary {
                path: category,
                reason: "trash category is not a regular non-symlink directory".into(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(crate::error::io_error(
                "inspect Calibre trash category",
                category,
                source,
            ));
        }
    }
    let entries = match fs::read_dir(&category) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(crate::error::io_error(
                "read Calibre trash category",
                category,
                source,
            ));
        }
    };
    let mut result = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| {
            crate::error::io_error("read Calibre trash category", &category, source)
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(raw_id) = name.parse::<i64>() else {
            continue;
        };
        if raw_id <= 0 {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|source| {
            crate::error::io_error("inspect trash entry", entry.path(), source)
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Error::InvalidLibrary {
                path: entry.path(),
                reason: "numeric trash entry is not a regular non-symlink directory".into(),
            });
        }
        result.push((BookId::new(raw_id), entry.path(), metadata));
    }
    result.sort_by_key(|(book, _, _)| *book);
    Ok(result)
}

fn ensure_category(root: &Path, kind: TrashEntryKind) -> Result<()> {
    ensure_directory(
        &category_path(root, kind)?,
        "inspect Calibre trash category",
    )
}

fn ensure_directory(path: &Path, operation: &'static str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| crate::error::io_error(operation, path, source))?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(Error::InvalidLibrary {
            path: path.to_path_buf(),
            reason: "expected a regular non-symlink directory".into(),
        })
    }
}

fn ensure_file(path: &Path, operation: &'static str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| crate::error::io_error(operation, path, source))?;
    if metadata.is_file() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(Error::InvalidLibrary {
            path: path.to_path_buf(),
            reason: "expected a regular non-symlink file".into(),
        })
    }
}

fn regular_file_optional(path: &Path, operation: &'static str) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(Error::InvalidLibrary {
            path: path.to_path_buf(),
            reason: "expected a regular non-symlink file or a missing path".into(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(crate::error::io_error(operation, path, source)),
    }
}

fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(crate::error::io_error("inspect path", path, source)),
    }
}

fn private_sibling(path: &Path) -> PathBuf {
    path.with_file_name(format!(".calibre-rs-prior-{}", uuid::Uuid::new_v4()))
}

fn existing_trash_asset(path: &Path, directory: bool) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if !metadata.file_type().is_symlink()
                && ((directory && metadata.is_dir()) || (!directory && metadata.is_file())) =>
        {
            Ok(true)
        }
        Ok(_) => Err(Error::InvalidLibrary {
            path: path.to_path_buf(),
            reason: "existing trash destination has an unexpected file type".into(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(crate::error::io_error(
            "inspect previous trash entry",
            path,
            source,
        )),
    }
}

fn restore_live_and_previous(
    live: &Path,
    trash: &Path,
    previous: &Path,
    previous_exists: bool,
    directory: bool,
) -> Result<()> {
    fs::rename(trash, live)
        .map_err(|source| crate::error::io_error("restore failed trash move", trash, source))?;
    restore_previous(trash, previous, previous_exists, directory)
}

fn restore_previous(
    trash: &Path,
    previous: &Path,
    previous_exists: bool,
    _directory: bool,
) -> Result<()> {
    if previous_exists {
        fs::rename(previous, trash).map_err(|source| {
            crate::error::io_error("restore previous trash entry", previous, source)
        })?;
    }
    Ok(())
}

fn remove_previous(path: &Path, exists: bool, directory: bool) -> Result<()> {
    if !exists {
        return Ok(());
    }
    if directory {
        ensure_directory(path, "remove previous trash entry")?;
        fs::remove_dir_all(path)
    } else {
        ensure_file(path, "remove previous trash entry")?;
        fs::remove_file(path)
    }
    .map_err(|source| crate::error::io_error("remove previous trash entry", path, source))
}

fn delete_entry_path(root: &Path, book: BookId, kind: TrashEntryKind) -> Result<bool> {
    let category = category_path(root, kind)?;
    match fs::symlink_metadata(&category) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(Error::InvalidLibrary {
                path: category,
                reason: "trash category is not a regular non-symlink directory".into(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(crate::error::io_error(
                "inspect trash category",
                category,
                source,
            ));
        }
    }
    let path = trash_entry_path(root, kind, book)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(&path)
                .map_err(|source| crate::error::io_error("delete trash entry", &path, source))?;
            Ok(true)
        }
        Ok(_) => Err(Error::InvalidLibrary {
            path,
            reason: "trash entry is not a regular non-symlink directory".into(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(crate::error::io_error("inspect trash entry", path, source)),
    }
}

fn read_format_metadata(directory: &Path) -> Result<Option<FormatListingMetadata>> {
    let path = directory.join("metadata.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
        Ok(_) => {
            return Err(Error::InvalidLibrary {
                path,
                reason: "format trash metadata is not a regular non-symlink file".into(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(crate::error::io_error(
                "inspect format trash metadata",
                path,
                source,
            ));
        }
    };
    if metadata.len() > 1024 * 1024 {
        return Ok(None);
    }
    let input = fs::File::open(&path)
        .map_err(|source| crate::error::io_error("open format trash metadata", &path, source))?;
    Ok(serde_json::from_reader(input).ok())
}

pub(crate) fn write_format_metadata_at(
    directory: &Path,
    metadata: &FormatListingMetadata,
) -> Result<()> {
    let destination = directory.join("metadata.json");
    let mut temporary = tempfile::NamedTempFile::new_in(directory).map_err(|source| {
        crate::error::io_error("create temporary trash metadata", directory, source)
    })?;
    serde_json::to_writer(&mut temporary, metadata).map_err(|source| {
        Error::UnsupportedOperation {
            operation: "serialize format trash metadata",
            reason: source.to_string(),
        }
    })?;
    temporary.as_file_mut().sync_all().map_err(|source| {
        crate::error::io_error("sync format trash metadata", &destination, source)
    })?;
    temporary.persist(&destination).map_err(|error| {
        crate::error::io_error("install format trash metadata", destination, error.error)
    })?;
    Ok(())
}

pub(crate) fn write_format_metadata_values(
    directory: &Path,
    title: &str,
    authors: &[String],
) -> Result<()> {
    write_format_metadata_at(
        directory,
        &FormatListingMetadata {
            title: title.to_owned(),
            authors: authors.to_vec(),
        },
    )
}

fn remove_empty_format_entry(directory: &Path) -> Result<()> {
    let mut has_format = false;
    for entry in fs::read_dir(directory)
        .map_err(|source| crate::error::io_error("inspect format trash entry", directory, source))?
    {
        let entry = entry.map_err(|source| {
            crate::error::io_error("inspect format trash entry", directory, source)
        })?;
        if entry.file_name() != "metadata.json" {
            has_format = true;
            break;
        }
    }
    if !has_format {
        let metadata = directory.join("metadata.json");
        if regular_file_optional(&metadata, "inspect format trash metadata")? {
            fs::remove_file(&metadata).map_err(|source| {
                crate::error::io_error("remove format trash metadata", &metadata, source)
            })?;
        }
        fs::remove_dir(directory).map_err(|source| {
            crate::error::io_error("remove empty format trash entry", directory, source)
        })?;
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    ensure_directory(source, "copy book from trash")?;
    fs::create_dir(destination).map_err(|source_error| {
        crate::error::io_error("create trash copy destination", destination, source_error)
    })?;
    let result = (|| {
        for entry in fs::read_dir(source)
            .map_err(|error| crate::error::io_error("read trashed book", source, error))?
        {
            let entry = entry
                .map_err(|error| crate::error::io_error("read trashed book", source, error))?;
            let input = entry.path();
            let output = destination.join(entry.file_name());
            let metadata = fs::symlink_metadata(&input).map_err(|error| {
                crate::error::io_error("inspect trashed book entry", &input, error)
            })?;
            if metadata.file_type().is_symlink() {
                return Err(Error::InvalidLibrary {
                    path: input,
                    reason: "trashed book contains a symlink".into(),
                });
            }
            if metadata.is_dir() {
                copy_tree(&input, &output)?;
            } else if metadata.is_file() {
                fs::copy(&input, &output).map_err(|error| {
                    crate::error::io_error("copy trashed book entry", &input, error)
                })?;
            } else {
                return Err(Error::InvalidLibrary {
                    path: input,
                    reason: "trashed book contains a non-regular entry".into(),
                });
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    result
}

fn validate_tree(directory: &Path) -> Result<()> {
    ensure_directory(directory, "validate trashed book tree")?;
    for entry in fs::read_dir(directory)
        .map_err(|source| crate::error::io_error("validate trashed book tree", directory, source))?
    {
        let entry = entry.map_err(|source| {
            crate::error::io_error("validate trashed book tree", directory, source)
        })?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|source| {
            crate::error::io_error("validate trashed book entry", entry.path(), source)
        })?;
        if metadata.file_type().is_symlink() {
            return Err(Error::InvalidLibrary {
                path: entry.path(),
                reason: "trashed book contains a symlink".into(),
            });
        }
        if metadata.is_dir() {
            validate_tree(&entry.path())?;
        } else if !metadata.is_file() {
            return Err(Error::InvalidLibrary {
                path: entry.path(),
                reason: "trashed book contains a non-regular entry".into(),
            });
        }
    }
    Ok(())
}

fn remove_empty_parent(book_directory: &Path, root: &Path) {
    if let Some(parent) = book_directory.parent().filter(|parent| *parent != root) {
        let _ = fs::remove_dir(parent);
    }
}
