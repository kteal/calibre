use crate::library::{LibraryInner, database_error};
use crate::{BookId, Error, Format, Result};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Operations on ebook format files and their database rows.
#[derive(Clone, Debug)]
pub struct Formats {
    inner: Arc<LibraryInner>,
}

impl Formats {
    pub(crate) const fn new(inner: Arc<LibraryInner>) -> Self {
        Self { inner }
    }

    /// Lists format rows for a book.
    ///
    /// # Errors
    ///
    /// Returns an error when the book or database cannot be read or a stored
    /// path is unsafe.
    pub fn list(&self, book: BookId) -> Result<Vec<Format>> {
        Ok(crate::Library {
            inner: Arc::clone(&self.inner),
        }
        .books()
        .get(book)?
        .formats)
    }

    /// Returns a checked path for a format.
    ///
    /// # Errors
    ///
    /// Returns an error when the book or format is missing or its path is unsafe.
    pub fn path(&self, book: BookId, format: impl AsRef<OsStr>) -> Result<PathBuf> {
        let format = crate::paths::format_name(format.as_ref())?;
        self.list(book)?
            .into_iter()
            .find(|item| item.format.eq_ignore_ascii_case(&format))
            .map(|item| item.path)
            .ok_or_else(|| Error::UnsupportedOperation {
                operation: "resolve format",
                reason: format!("book {book} has no {format} format"),
            })
    }

    /// Reads a format into memory.
    ///
    /// # Errors
    ///
    /// Returns an error when resolution or file reading fails.
    pub fn read(&self, book: BookId, format: impl AsRef<OsStr>) -> Result<Vec<u8>> {
        let path = self.path(book, format)?;
        fs::read(&path).map_err(|source| crate::error::io_error("read format", path, source))
    }

    /// Streams a format to a writer.
    ///
    /// # Errors
    ///
    /// Returns an error when resolution, file reading, or writer output fails.
    pub fn write_to(
        &self,
        book: BookId,
        format: impl AsRef<OsStr>,
        writer: &mut impl Write,
    ) -> Result<u64> {
        let path = self.path(book, format)?;
        let mut source = fs::File::open(&path)
            .map_err(|error| crate::error::io_error("open format", &path, error))?;
        std::io::copy(&mut source, writer)
            .map_err(|error| crate::error::io_error("stream format", path, error))
    }

    /// Copies a format to a caller-selected destination.
    ///
    /// # Errors
    ///
    /// Returns an error when resolution, reading, or destination writing fails.
    pub fn copy_to(
        &self,
        book: BookId,
        format: impl AsRef<OsStr>,
        destination: impl AsRef<Path>,
    ) -> Result<u64> {
        let source = self.path(book, format)?;
        let mut source_file = fs::File::open(&source)
            .map_err(|error| crate::error::io_error("open format", &source, error))?;
        let mut destination_file = fs::File::create(destination.as_ref()).map_err(|error| {
            crate::error::io_error("create format destination", destination.as_ref(), error)
        })?;
        std::io::copy(&mut source_file, &mut destination_file).map_err(|error| {
            crate::error::io_error("copy format out of library", destination.as_ref(), error)
        })
    }

    /// Adds a new format. Existing formats are not replaced.
    ///
    /// # Errors
    ///
    /// Returns an error for read-only mode, active FTS state, invalid input,
    /// an existing format, or database/filesystem failure.
    pub fn add(&self, book: BookId, source: impl AsRef<Path>) -> Result<Format> {
        self.put_path(book, source.as_ref(), false)
    }

    /// Adds a new format by streaming from a reader.
    ///
    /// # Errors
    ///
    /// Returns an error for read-only mode, active FTS state, an invalid or
    /// existing format, reader failure, or database/filesystem failure.
    pub fn add_from_reader(
        &self,
        book: BookId,
        format: impl AsRef<OsStr>,
        reader: &mut impl Read,
    ) -> Result<Format> {
        self.put_reader(book, format.as_ref(), reader, false)
    }

    /// Adds or atomically replaces a format.
    ///
    /// # Errors
    ///
    /// Returns an error for read-only mode, active FTS state, invalid input,
    /// or database/filesystem failure.
    pub fn replace(&self, book: BookId, source: impl AsRef<Path>) -> Result<Format> {
        self.put_path(book, source.as_ref(), true)
    }

    /// Adds or atomically replaces a format by streaming from a reader.
    ///
    /// # Errors
    ///
    /// Returns an error for read-only mode, active FTS state, invalid input,
    /// reader failure, or database/filesystem failure.
    pub fn replace_from_reader(
        &self,
        book: BookId,
        format: impl AsRef<OsStr>,
        reader: &mut impl Read,
    ) -> Result<Format> {
        self.put_reader(book, format.as_ref(), reader, true)
    }

    /// Removes one format while keeping the logical book.
    ///
    /// # Errors
    ///
    /// Returns an error for read-only mode, active FTS state, a missing
    /// format, an unsafe path, or database/filesystem failure.
    pub fn remove(&self, book: BookId, format: impl AsRef<OsStr>) -> Result<()> {
        if !self.inner.capabilities.write_formats {
            return Err(Error::UnsupportedOperation {
                operation: "remove format",
                reason: "read-write mode and inactive full-text-search state are required".into(),
            });
        }
        let format = crate::paths::format_name(format.as_ref())?;
        let _guard = self.inner.lock_writer("remove format")?;
        let path = self.path(book, &format)?;
        let mut connection = self.inner.write_connection("remove format")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                database_error(
                    "begin remove-format transaction",
                    &self.inner.database,
                    error,
                )
            })?;
        let mut replacement = if path.exists() {
            Some(AssetReplacement::stage_removal(&path)?)
        } else {
            None
        };
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
            .and_then(|()| mark_format_changed(&transaction, book))
            .and_then(|()| transaction.commit())
            .map_err(|error| database_error("remove format", &self.inner.database, error));
        match result {
            Ok(()) => {
                if let Some(replacement) = replacement.take() {
                    replacement.commit()?;
                }
                Ok(())
            }
            Err(error) => {
                if let Some(replacement) = replacement.take() {
                    replacement.rollback()?;
                }
                Err(error)
            }
        }
    }

    fn put_path(&self, book: BookId, source: &Path, replace: bool) -> Result<Format> {
        let format = crate::paths::format_from_path(source)?;
        let mut reader = fs::File::open(source)
            .map_err(|error| crate::error::io_error("open source format", source, error))?;
        self.put_reader(book, OsStr::new(&format), &mut reader, replace)
    }

    #[allow(clippy::too_many_lines)] // The transaction and file compensation form one boundary.
    fn put_reader(
        &self,
        book: BookId,
        format: &OsStr,
        reader: &mut impl Read,
        replace: bool,
    ) -> Result<Format> {
        if !self.inner.capabilities.write_formats {
            return Err(Error::UnsupportedOperation {
                operation: "write format",
                reason: "read-write mode and inactive full-text-search state are required".into(),
            });
        }
        let format = crate::paths::format_name(format)?;
        let _guard = self.inner.lock_writer("write format")?;
        let book_value = crate::Library {
            inner: Arc::clone(&self.inner),
        }
        .books()
        .get(book)?;
        let directory = crate::paths::resolve(&self.inner.root, &book_value.relative_path)?;
        if !directory.is_dir() {
            return Err(Error::UnsupportedOperation {
                operation: "write format",
                reason: "book directory is missing".into(),
            });
        }
        let mut connection = self.inner.write_connection("write format")?;
        let existing: Option<String> = connection
            .query_row(
                "SELECT name FROM data WHERE book = ?1 AND format = ?2 COLLATE NOCASE",
                params![book.get(), format],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| database_error("inspect format row", &self.inner.database, error))?;
        if existing.is_some() && !replace {
            return Err(Error::InvalidInput {
                field: "format",
                reason: format!("book {book} already has {format}"),
            });
        }
        let stem = if let Some(stem) = existing {
            stem
        } else {
            connection
                .query_row(
                    "SELECT name FROM data WHERE book = ?1 ORDER BY id LIMIT 1",
                    [book.get()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| {
                    database_error("find format filename", &self.inner.database, error)
                })?
                .unwrap_or_else(|| {
                    crate::paths::format_stem(
                        &book_value.title,
                        book_value
                            .authors
                            .first()
                            .map_or("Unknown", |author| &author.name),
                    )
                })
        };
        let destination = directory.join(format!("{stem}.{}", format.to_ascii_lowercase()));
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                database_error(
                    "begin write-format transaction",
                    &self.inner.database,
                    error,
                )
            })?;
        let (replacement, source_size) =
            AssetReplacement::install_from_reader(reader, &destination)?;
        let source_size = match crate::books::i64_size(source_size) {
            Ok(size) => size,
            Err(error) => {
                replacement.rollback()?;
                return Err(error);
            }
        };
        let result = transaction
            .execute(
                "INSERT INTO data(book, format, uncompressed_size, name) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(book, format) DO UPDATE SET \
                 uncompressed_size = excluded.uncompressed_size, name = excluded.name",
                params![book.get(), format, source_size, stem],
            )
            .and_then(|_| mark_format_changed(&transaction, book))
            .and_then(|()| transaction.commit())
            .map_err(|error| database_error("write format", &self.inner.database, error));
        match result {
            Ok(()) => replacement.commit()?,
            Err(error) => {
                replacement.rollback()?;
                return Err(error);
            }
        }
        self.list(book)?
            .into_iter()
            .find(|item| item.format.eq_ignore_ascii_case(&format))
            .ok_or_else(|| Error::UnsupportedOperation {
                operation: "load added format",
                reason: "database committed but the format row was not found".into(),
            })
    }
}

fn mark_format_changed(
    transaction: &rusqlite::Transaction<'_>,
    book: BookId,
) -> rusqlite::Result<()> {
    crate::sql::mark_metadata_dirty(transaction, book)?;
    transaction.execute(
        "UPDATE books_pages_link SET needs_scan = 1 WHERE book = ?1",
        [book.get()],
    )?;
    Ok(())
}

pub(crate) struct AssetReplacement {
    destination: PathBuf,
    backup: Option<PathBuf>,
    installed: bool,
}

impl AssetReplacement {
    pub(crate) fn install_from_reader(
        source: &mut impl Read,
        destination: &Path,
    ) -> Result<(Self, u64)> {
        let backup = if destination.exists() {
            let backup =
                destination.with_file_name(format!(".calibre-rs-backup-{}", uuid::Uuid::new_v4()));
            fs::rename(destination, &backup).map_err(|error| {
                crate::error::io_error("stage existing asset", destination, error)
            })?;
            Some(backup)
        } else {
            None
        };
        match copy_reader_to_new_asset(source, destination) {
            Ok(size) => Ok((
                Self {
                    destination: destination.to_path_buf(),
                    backup,
                    installed: true,
                },
                size,
            )),
            Err(error) => {
                if let Some(backup) = &backup {
                    let _ = fs::rename(backup, destination);
                }
                Err(error)
            }
        }
    }

    pub(crate) fn stage_removal(destination: &Path) -> Result<Self> {
        let backup =
            destination.with_file_name(format!(".calibre-rs-remove-{}", uuid::Uuid::new_v4()));
        fs::rename(destination, &backup)
            .map_err(|error| crate::error::io_error("stage asset removal", destination, error))?;
        Ok(Self {
            destination: destination.to_path_buf(),
            backup: Some(backup),
            installed: false,
        })
    }

    pub(crate) fn commit(mut self) -> Result<()> {
        if let Some(backup) = self.backup.take() {
            fs::remove_file(&backup).map_err(|error| {
                crate::error::io_error("remove staged asset backup", backup, error)
            })?;
        }
        Ok(())
    }

    pub(crate) fn rollback(mut self) -> Result<()> {
        if self.installed && self.destination.exists() {
            fs::remove_file(&self.destination).map_err(|error| {
                crate::error::io_error("remove failed replacement", &self.destination, error)
            })?;
        }
        if let Some(backup) = self.backup.take() {
            fs::rename(&backup, &self.destination)
                .map_err(|error| crate::error::io_error("restore asset backup", backup, error))?;
        }
        Ok(())
    }
}

fn copy_reader_to_new_asset(source: &mut impl Read, destination: &Path) -> Result<u64> {
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
    let size = std::io::copy(source, staged.as_file_mut())
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
