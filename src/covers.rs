use crate::formats::AssetReplacement;
use crate::library::{LibraryInner, database_error};
use crate::{BookId, Error, Result};
use rusqlite::TransactionBehavior;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Operations on `cover.jpg`.
#[derive(Clone, Debug)]
pub struct Covers {
    inner: Arc<LibraryInner>,
}

impl Covers {
    pub(crate) const fn new(inner: Arc<LibraryInner>) -> Self {
        Self { inner }
    }

    /// Returns the checked cover path when the database says a cover exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the book cannot be loaded or its path is unsafe.
    pub fn path(&self, book: BookId) -> Result<Option<PathBuf>> {
        Ok(crate::Library {
            inner: Arc::clone(&self.inner),
        }
        .books()
        .get(book)?
        .cover_path)
    }

    /// Reads a cover into memory.
    ///
    /// # Errors
    ///
    /// Returns an error when path resolution or file reading fails.
    pub fn read(&self, book: BookId) -> Result<Option<Vec<u8>>> {
        let Some(path) = self.path(book)? else {
            return Ok(None);
        };
        fs::read(&path)
            .map(Some)
            .map_err(|error| crate::error::io_error("read cover", path, error))
    }

    /// Streams a cover to a writer.
    ///
    /// Returns `Ok(None)` when the database records no cover.
    ///
    /// # Errors
    ///
    /// Returns an error when path resolution, file reading, or writer output fails.
    pub fn write_to(&self, book: BookId, writer: &mut impl Write) -> Result<Option<u64>> {
        let Some(path) = self.path(book)? else {
            return Ok(None);
        };
        let mut source = fs::File::open(&path)
            .map_err(|error| crate::error::io_error("open cover", &path, error))?;
        std::io::copy(&mut source, writer)
            .map(Some)
            .map_err(|error| crate::error::io_error("stream cover", path, error))
    }

    /// Adds or atomically replaces `cover.jpg`.
    ///
    /// # Errors
    ///
    /// Returns an error for read-only mode, a missing book directory, or a
    /// database/filesystem failure.
    pub fn replace(&self, book: BookId, source: impl AsRef<Path>) -> Result<PathBuf> {
        let mut source_file = fs::File::open(source.as_ref())
            .map_err(|error| crate::error::io_error("open source cover", source.as_ref(), error))?;
        self.replace_from_reader(book, &mut source_file)
    }

    /// Adds or atomically replaces `cover.jpg` by streaming from a reader.
    ///
    /// # Errors
    ///
    /// Returns an error for read-only mode, a missing book directory, reader
    /// failure, or a database/filesystem failure.
    pub fn replace_from_reader(&self, book: BookId, reader: &mut impl Read) -> Result<PathBuf> {
        let _guard = self.inner.lock_writer("replace cover")?;
        let book_value = crate::Library {
            inner: Arc::clone(&self.inner),
        }
        .books()
        .get(book)?;
        let directory = crate::paths::resolve(&self.inner.root, &book_value.relative_path)?;
        if !directory.is_dir() {
            return Err(Error::UnsupportedOperation {
                operation: "replace cover",
                reason: "book directory is missing".into(),
            });
        }
        let destination = directory.join("cover.jpg");
        let mut connection = self.inner.write_connection("replace cover")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                database_error(
                    "begin replace-cover transaction",
                    &self.inner.database,
                    error,
                )
            })?;
        let (replacement, _) = AssetReplacement::install_from_reader(reader, &destination)?;
        let result = transaction
            .execute("UPDATE books SET has_cover = 1 WHERE id = ?1", [book.get()])
            .and_then(|changed| {
                if changed == 0 {
                    Err(rusqlite::Error::QueryReturnedNoRows)
                } else {
                    Ok(())
                }
            })
            .and_then(|()| crate::sql::mark_metadata_dirty(&transaction, book))
            .and_then(|()| transaction.commit())
            .map_err(|error| database_error("replace cover", &self.inner.database, error));
        match result {
            Ok(()) => {
                replacement.commit()?;
                Ok(destination)
            }
            Err(error) => {
                replacement.rollback()?;
                Err(error)
            }
        }
    }

    /// Removes `cover.jpg` and clears the database flag.
    ///
    /// # Errors
    ///
    /// Returns an error for read-only mode, an unsafe path, or a
    /// database/filesystem failure.
    pub fn remove(&self, book: BookId) -> Result<bool> {
        let _guard = self.inner.lock_writer("remove cover")?;
        let Some(path) = self.path(book)? else {
            return Ok(false);
        };
        let mut connection = self.inner.write_connection("remove cover")?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                database_error(
                    "begin remove-cover transaction",
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
            .execute("UPDATE books SET has_cover = 0 WHERE id = ?1", [book.get()])
            .and_then(|changed| {
                if changed == 0 {
                    Err(rusqlite::Error::QueryReturnedNoRows)
                } else {
                    Ok(())
                }
            })
            .and_then(|()| crate::sql::mark_metadata_dirty(&transaction, book))
            .and_then(|()| transaction.commit())
            .map_err(|error| database_error("remove cover", &self.inner.database, error));
        match result {
            Ok(()) => {
                if let Some(replacement) = replacement.take() {
                    replacement.commit()?;
                }
                Ok(true)
            }
            Err(error) => {
                if let Some(replacement) = replacement.take() {
                    replacement.rollback()?;
                }
                Err(error)
            }
        }
    }
}
