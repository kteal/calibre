use crate::library::{LibraryInner, database_error};
use crate::{BookId, DeletionMode, Error, Format, Result};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
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

    /// Moves one format to Calibre's per-library trash.
    ///
    /// # Errors
    ///
    /// A missing format is a no-op. Returns an error for read-only mode,
    /// active FTS state, an unsafe path, or database/filesystem failure.
    pub fn remove(&self, book: BookId, format: impl AsRef<OsStr>) -> Result<()> {
        self.remove_with(book, format, DeletionMode::Trash)
    }

    /// Removes one format using explicit trash or permanent semantics.
    ///
    /// # Errors
    ///
    /// A missing format is a no-op. Returns an error for read-only mode,
    /// active FTS state, an unsafe path, or database/filesystem failure.
    pub fn remove_with(
        &self,
        book: BookId,
        format: impl AsRef<OsStr>,
        mode: DeletionMode,
    ) -> Result<()> {
        let format = crate::paths::format_name(format.as_ref())?;
        match mode {
            DeletionMode::Trash => crate::trash::move_format_to_trash(&self.inner, book, &format),
            DeletionMode::Permanent => self.remove_permanently(book, OsStr::new(&format)),
        }
    }

    /// Permanently removes one format while keeping the logical book.
    ///
    /// # Errors
    ///
    /// A missing format is a no-op. Returns an error for read-only mode,
    /// active FTS state, an unsafe path, or database/filesystem failure.
    pub fn remove_permanently(&self, book: BookId, format: impl AsRef<OsStr>) -> Result<()> {
        if !self.inner.capabilities.write_formats {
            return Err(Error::UnsupportedOperation {
                operation: "remove format",
                reason: "read-write mode and inactive full-text-search state are required".into(),
            });
        }
        let format = crate::paths::format_name(format.as_ref())?;
        let _guard = self.inner.lock_writer("remove format")?;
        let book_value = crate::Library {
            inner: Arc::clone(&self.inner),
        }
        .books()
        .get(book)?;
        let Some(existing) = book_value
            .formats
            .iter()
            .find(|item| item.format.eq_ignore_ascii_case(&format))
        else {
            return Ok(());
        };
        let path = existing.path.clone();
        let before_file = regular_file_exists(&path, "remove format")?;
        let backup = asset_sibling_path(&path, "remove");
        let mut connection = self.inner.write_connection("remove format")?;
        let before = connection
            .query_row(
                "SELECT format, uncompressed_size, name FROM data \
                 WHERE book = ?1 AND format = ?2 COLLATE NOCASE",
                params![book.get(), format],
                |row| {
                    Ok(crate::recovery::FormatRecoveryState {
                        format: row.get(0)?,
                        size: row.get(1)?,
                        stem: row.get(2)?,
                    })
                },
            )
            .map_err(|error| database_error("inspect format row", &self.inner.database, error))?;
        let journal = crate::recovery::RecoveryJournal::begin_format_removal(
            &self.inner.root,
            book,
            &path,
            &backup,
            before_file,
            before,
        )?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                database_error(
                    "begin remove-format transaction",
                    &self.inner.database,
                    error,
                )
            })?;
        let mut replacement = if before_file {
            Some(AssetReplacement::stage_removal(&path, &backup)?)
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
                journal.complete()?;
                Ok(())
            }
            Err(error) => {
                if let Some(replacement) = replacement.take() {
                    replacement.rollback()?;
                }
                journal.complete()?;
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
        let existing: Option<crate::recovery::FormatRecoveryState> = connection
            .query_row(
                "SELECT format, uncompressed_size, name FROM data \
                 WHERE book = ?1 AND format = ?2 COLLATE NOCASE",
                params![book.get(), format],
                |row| {
                    Ok(crate::recovery::FormatRecoveryState {
                        format: row.get(0)?,
                        size: row.get(1)?,
                        stem: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(|error| database_error("inspect format row", &self.inner.database, error))?;
        if existing.is_some() && !replace {
            return Err(Error::InvalidInput {
                field: "format",
                reason: format!("book {book} already has {format}"),
            });
        }
        let stem = if let Some(existing) = &existing {
            existing.stem.clone()
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
        let before_file = regular_file_exists(&destination, "write format")?;
        let backup = asset_sibling_path(&destination, "backup");
        let staged = asset_sibling_path(&destination, "stage");
        let mut journal = crate::recovery::RecoveryJournal::begin_format_write(
            &self.inner.root,
            book,
            &destination,
            &backup,
            &staged,
            before_file,
            existing,
            &format,
            &stem,
        )?;
        let source_size = match stage_reader(reader, &staged) {
            Ok(size) => size,
            Err(error) => {
                remove_staged_if_present(&staged)?;
                journal.complete()?;
                return Err(error);
            }
        };
        let source_size = match crate::books::i64_size(source_size) {
            Ok(size) => size,
            Err(error) => {
                remove_staged_if_present(&staged)?;
                journal.complete()?;
                return Err(error);
            }
        };
        if let Err(error) = journal.mark_format_ready(source_size) {
            remove_staged_if_present(&staged)?;
            journal.complete()?;
            return Err(error);
        }
        let replacement =
            match AssetReplacement::install_staged(&destination, &backup, &staged, before_file) {
                Ok(replacement) => replacement,
                Err(error) => {
                    remove_staged_if_present(&staged)?;
                    AssetReplacement::restore_uninstalled(&destination, &backup, before_file)?;
                    journal.complete()?;
                    return Err(error);
                }
            };
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                database_error(
                    "begin write-format transaction",
                    &self.inner.database,
                    error,
                )
            })?;
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
            Ok(()) => {
                replacement.commit()?;
                journal.complete()?;
            }
            Err(error) => {
                replacement.rollback()?;
                journal.complete()?;
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

pub(crate) fn mark_format_changed(
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
    pub(crate) fn install_staged(
        destination: &Path,
        backup: &Path,
        staged: &Path,
        before_file: bool,
    ) -> Result<Self> {
        let backup = if before_file {
            fs::rename(destination, backup).map_err(|error| {
                crate::error::io_error("stage existing asset", destination, error)
            })?;
            Some(backup.to_path_buf())
        } else {
            None
        };
        match fs::rename(staged, destination) {
            Ok(()) => Ok(Self {
                destination: destination.to_path_buf(),
                backup,
                installed: true,
            }),
            Err(error) => Err(crate::error::io_error(
                "install staged asset",
                staged,
                error,
            )),
        }
    }

    pub(crate) fn restore_uninstalled(
        destination: &Path,
        backup: &Path,
        before_file: bool,
    ) -> Result<()> {
        let destination_exists = regular_file_exists(destination, "restore uninstalled asset")?;
        let backup_exists = regular_file_exists(backup, "restore uninstalled asset")?;
        match (destination_exists, backup_exists, before_file) {
            (false, true, true) => fs::rename(backup, destination).map_err(|error| {
                crate::error::io_error("restore uninstalled asset", backup, error)
            }),
            (true, false, true) | (false, false, false) => Ok(()),
            _ => Err(Error::UnsupportedOperation {
                operation: "restore uninstalled asset",
                reason: "asset and backup paths have an ambiguous state".into(),
            }),
        }
    }

    pub(crate) fn stage_removal(destination: &Path, backup: &Path) -> Result<Self> {
        fs::rename(destination, backup)
            .map_err(|error| crate::error::io_error("stage asset removal", destination, error))?;
        Ok(Self {
            destination: destination.to_path_buf(),
            backup: Some(backup.to_path_buf()),
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

pub(crate) fn asset_sibling_path(destination: &Path, purpose: &str) -> PathBuf {
    destination.with_file_name(format!(".calibre-rs-{purpose}-{}", uuid::Uuid::new_v4()))
}

pub(crate) fn stage_reader(source: &mut impl Read, staged: &Path) -> Result<u64> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staged)
        .map_err(|error| crate::error::io_error("create staged asset", staged, error))?;
    let size = std::io::copy(source, &mut output)
        .map_err(|error| crate::error::io_error("copy staged asset", staged, error))?;
    output
        .sync_all()
        .map_err(|error| crate::error::io_error("sync staged asset", staged, error))?;
    Ok(size)
}

pub(crate) fn regular_file_exists(path: &Path, operation: &'static str) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(Error::UnsupportedOperation {
            operation,
            reason: format!(
                "expected a regular file or missing path: {}",
                path.display()
            ),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(crate::error::io_error(operation, path, error)),
    }
}

fn remove_staged_if_present(path: &Path) -> Result<()> {
    if regular_file_exists(path, "remove staged asset")? {
        fs::remove_file(path)
            .map_err(|error| crate::error::io_error("remove staged asset", path, error))?;
    }
    Ok(())
}
