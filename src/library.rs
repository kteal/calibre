use crate::{
    Auditor, Books, Covers, CustomColumns, Error, Formats, RecoveryEntry, RecoveryReport, Result,
    Trash,
};
use rusqlite::{Connection, OpenFlags, functions::FunctionFlags};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use uuid::Uuid;

pub(crate) const SUPPORTED_SCHEMA_VERSION: u32 = 27;
const CALIBRE_APPLICATION_ID: u32 = 0x6361_6c69;

/// Access mode for an opened library.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenMode {
    /// `SQLite` and book files are never intentionally modified.
    ReadOnly,
    /// Proven write operations are enabled.
    ReadWrite,
}

/// Options controlling library opening.
#[derive(Clone, Copy, Debug)]
pub struct OpenOptions {
    mode: OpenMode,
    busy_timeout: Duration,
}

impl OpenOptions {
    /// Creates conservative read-only options.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            mode: OpenMode::ReadOnly,
            busy_timeout: Duration::from_secs(5),
        }
    }

    /// Enables or disables read-write mode.
    #[must_use]
    pub const fn read_write(mut self, enabled: bool) -> Self {
        self.mode = if enabled {
            OpenMode::ReadWrite
        } else {
            OpenMode::ReadOnly
        };
        self
    }

    /// Sets `SQLite`'s busy timeout for operations.
    #[must_use]
    pub const fn busy_timeout(mut self, timeout: Duration) -> Self {
        self.busy_timeout = timeout;
        self
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// Schema compatibility details.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Compatibility {
    /// Detected `PRAGMA user_version`.
    pub schema_version: u32,
    /// Detected `PRAGMA application_id`.
    pub application_id: u32,
    /// Human-readable supported-version policy.
    pub supported_schema_versions: &'static str,
}

/// Operation-level capabilities for an opened library.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)] // A report of independent operation capabilities.
pub struct Capabilities {
    /// Core metadata can be read.
    pub read_books: bool,
    /// Core book records and relationships can be written.
    pub write_books: bool,
    /// Formats can be added, replaced, read, and removed.
    pub write_formats: bool,
    /// Covers can be added, replaced, read, and removed.
    pub write_covers: bool,
    /// Permanent deletion is available for libraries without deferred state.
    pub permanent_delete: bool,
    /// Calibre-compatible trash writes are available.
    pub calibre_trash: bool,
    /// Custom-column definitions and stored values can be read.
    pub read_custom_columns: bool,
    /// Custom-column definitions and stored values can be written.
    pub write_custom_columns: bool,
    /// Durable recovery records are waiting to be resolved.
    pub recovery_required: bool,
}

#[derive(Debug)]
pub(crate) struct LibraryInner {
    pub(crate) root: PathBuf,
    pub(crate) database: PathBuf,
    pub(crate) mode: OpenMode,
    pub(crate) busy_timeout: Duration,
    pub(crate) compatibility: Compatibility,
    pub(crate) capabilities: Capabilities,
    write_lock: Mutex<()>,
}

/// A validated handle to an existing Calibre library.
#[derive(Clone, Debug)]
pub struct Library {
    pub(crate) inner: Arc<LibraryInner>,
}

impl Library {
    /// Opens an existing library read-only.
    ///
    /// # Errors
    ///
    /// Returns an error when the root or database cannot be read, the schema
    /// identity is unsupported, or required schema objects are missing.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, OpenOptions::new())
    }

    /// Opens an existing library using explicit options.
    ///
    /// # Errors
    ///
    /// Returns an error when the root or database cannot be read, the schema
    /// identity is unsupported, or required schema objects are missing.
    pub fn open_with(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let supplied = path.as_ref();
        let root = std::fs::canonicalize(supplied)
            .map_err(|source| crate::error::io_error("canonicalize library", supplied, source))?;
        if !root.is_dir() {
            return Err(Error::InvalidLibrary {
                path: root,
                reason: "library root is not a directory".into(),
            });
        }
        let database = root.join("metadata.db");
        let metadata = std::fs::metadata(&database)
            .map_err(|source| crate::error::io_error("inspect metadata.db", &database, source))?;
        if !metadata.is_file() {
            return Err(Error::InvalidLibrary {
                path: root,
                reason: "metadata.db is not a regular file".into(),
            });
        }

        let connection = open_connection(&database, OpenMode::ReadOnly, options.busy_timeout)?;
        let schema_version = pragma_u32(&connection, "user_version", &database)?;
        let application_id = pragma_u32(&connection, "application_id", &database)?;
        if application_id != CALIBRE_APPLICATION_ID {
            return Err(Error::InvalidLibrary {
                path: root,
                reason: format!(
                    "unexpected SQLite application_id {application_id:#x}; expected Calibre"
                ),
            });
        }
        if schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(Error::UnsupportedSchema {
                operation: "open library",
                detected: schema_version,
                supported: "exactly version 27",
            });
        }
        validate_schema(&connection, &database)?;

        let custom_columns = has_active_custom_columns(&connection, &database)?;
        let fts_state = root.join("full-text-search.db").exists();
        let recovery_required = crate::recovery::has_pending(&root)?;
        let writable = options.mode == OpenMode::ReadWrite && !recovery_required;
        drop(connection);

        Ok(Self {
            inner: Arc::new(LibraryInner {
                root,
                database,
                mode: options.mode,
                busy_timeout: options.busy_timeout,
                compatibility: Compatibility {
                    schema_version,
                    application_id,
                    supported_schema_versions: "27",
                },
                capabilities: Capabilities {
                    read_books: true,
                    write_books: writable,
                    write_formats: writable && !fts_state,
                    write_covers: writable,
                    permanent_delete: writable && !fts_state && !custom_columns,
                    calibre_trash: writable && !fts_state && !custom_columns,
                    read_custom_columns: true,
                    write_custom_columns: false,
                    recovery_required,
                },
                write_lock: Mutex::new(()),
            }),
        })
    }

    /// Returns the canonical library root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Returns the selected access mode.
    #[must_use]
    pub fn mode(&self) -> OpenMode {
        self.inner.mode
    }

    /// Returns detected schema compatibility.
    #[must_use]
    pub fn compatibility(&self) -> &Compatibility {
        &self.inner.compatibility
    }

    /// Returns operation-level capabilities.
    #[must_use]
    pub fn capabilities(&self) -> &Capabilities {
        &self.inner.capabilities
    }

    /// Returns book operations.
    #[must_use]
    pub fn books(&self) -> Books {
        Books::new(Arc::clone(&self.inner))
    }

    /// Returns format operations.
    #[must_use]
    pub fn formats(&self) -> Formats {
        Formats::new(Arc::clone(&self.inner))
    }

    /// Returns cover operations.
    #[must_use]
    pub fn covers(&self) -> Covers {
        Covers::new(Arc::clone(&self.inner))
    }

    /// Returns Calibre trash operations.
    #[must_use]
    pub fn trash(&self) -> Trash {
        Trash::new(Arc::clone(&self.inner))
    }

    /// Returns read-only consistency checks.
    #[must_use]
    pub fn auditor(&self) -> Auditor {
        Auditor::new(Arc::clone(&self.inner))
    }

    /// Returns read-only custom-column operations.
    #[must_use]
    pub fn custom_columns(&self) -> CustomColumns {
        CustomColumns::new(Arc::clone(&self.inner))
    }

    /// Lists durable records left by interrupted database/filesystem writes.
    ///
    /// # Errors
    ///
    /// Returns an error when a journal cannot be read or validated.
    pub fn pending_recovery(&self) -> Result<Vec<RecoveryEntry>> {
        crate::recovery::pending(&self.inner.root)
    }

    /// Resolves interrupted book, format, cover, and directory-move writes.
    ///
    /// The database row determines whether a staged directory is kept,
    /// restored, or removed. Reopen the library after recovery to refresh its
    /// capability report.
    ///
    /// # Errors
    ///
    /// Returns an error in read-only mode or when journal, database, or
    /// filesystem state cannot be safely reconciled.
    pub fn recover_pending(&self) -> Result<RecoveryReport> {
        let _guard = self.inner.lock_writer("recover pending writes")?;
        crate::recovery::recover(&self.inner)
    }
}

impl LibraryInner {
    pub(crate) fn read_connection(&self) -> Result<Connection> {
        open_connection(&self.database, OpenMode::ReadOnly, self.busy_timeout)
    }

    pub(crate) fn write_connection(&self, operation: &'static str) -> Result<Connection> {
        let connection = self.recovery_connection_for(operation)?;
        crate::recovery::ensure_clear(&self.root, operation)?;
        Ok(connection)
    }

    pub(crate) fn recovery_connection(&self) -> Result<Connection> {
        self.recovery_connection_for("recover pending writes")
    }

    fn recovery_connection_for(&self, operation: &'static str) -> Result<Connection> {
        if self.mode != OpenMode::ReadWrite {
            return Err(Error::UnsupportedOperation {
                operation,
                reason: "library was opened read-only".into(),
            });
        }
        if self.compatibility.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(Error::UnsupportedSchema {
                operation,
                detected: self.compatibility.schema_version,
                supported: "version 27",
            });
        }
        open_connection(&self.database, OpenMode::ReadWrite, self.busy_timeout)
    }

    pub(crate) fn lock_writer(&self, operation: &'static str) -> Result<MutexGuard<'_, ()>> {
        self.write_lock
            .lock()
            .map_err(|_| Error::UnsupportedOperation {
                operation,
                reason: "the in-process writer lock was poisoned".into(),
            })
    }
}

fn open_connection(path: &Path, mode: OpenMode, timeout: Duration) -> Result<Connection> {
    let flags = match mode {
        OpenMode::ReadOnly => OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        OpenMode::ReadWrite => OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    };
    let connection = Connection::open_with_flags(path, flags)
        .map_err(|error| database_error("open SQLite connection", path, error))?;
    connection
        .busy_timeout(timeout)
        .map_err(|error| database_error("set busy timeout", path, error))?;
    if mode == OpenMode::ReadOnly {
        connection
            .pragma_update(None, "query_only", true)
            .map_err(|error| database_error("enable query-only mode", path, error))?;
    } else {
        register_functions(&connection, path)?;
    }
    Ok(connection)
}

fn register_functions(connection: &Connection, path: &Path) -> Result<()> {
    connection
        .create_scalar_function("uuid4", 0, FunctionFlags::SQLITE_UTF8, |_| {
            Ok(Uuid::new_v4().to_string())
        })
        .map_err(|error| database_error("register uuid4 function", path, error))?;
    connection
        .create_scalar_function(
            "title_sort",
            1,
            FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
            |context| {
                let title = context.get::<String>(0)?;
                Ok(crate::sql::conservative_title_sort(&title))
            },
        )
        .map_err(|error| database_error("register title_sort function", path, error))?;
    Ok(())
}

fn pragma_u32(connection: &Connection, name: &'static str, path: &Path) -> Result<u32> {
    connection
        .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
        .map_err(|error| database_error("read schema identity", path, error))
}

fn validate_schema(connection: &Connection, path: &Path) -> Result<()> {
    const REQUIRED: &[(&str, &[&str])] = &[
        (
            "books",
            &[
                "id",
                "title",
                "sort",
                "timestamp",
                "pubdate",
                "series_index",
                "author_sort",
                "path",
                "uuid",
                "has_cover",
                "last_modified",
            ],
        ),
        ("authors", &["id", "name", "sort", "link"]),
        ("tags", &["id", "name", "link"]),
        ("series", &["id", "name", "sort", "link"]),
        ("publishers", &["id", "name", "sort", "link"]),
        ("languages", &["id", "lang_code", "link"]),
        ("ratings", &["id", "rating", "link"]),
        ("comments", &["book", "text"]),
        ("identifiers", &["book", "type", "val"]),
        (
            "data",
            &["id", "book", "format", "uncompressed_size", "name"],
        ),
        ("books_authors_link", &["id", "book", "author"]),
        ("books_tags_link", &["id", "book", "tag"]),
        ("books_series_link", &["id", "book", "series"]),
        ("books_publishers_link", &["id", "book", "publisher"]),
        (
            "books_languages_link",
            &["id", "book", "lang_code", "item_order"],
        ),
        ("books_ratings_link", &["id", "book", "rating"]),
        ("metadata_dirtied", &["book"]),
        ("books_pages_link", &["book", "needs_scan"]),
    ];
    for (table, columns) in REQUIRED {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| database_error("inspect schema", path, error))?;
        let present = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| database_error("inspect schema", path, error))?
            .collect::<std::result::Result<BTreeSet<_>, _>>()
            .map_err(|error| database_error("inspect schema", path, error))?;
        for column in *columns {
            if !present.contains(*column) {
                return Err(Error::InvalidLibrary {
                    path: path.to_path_buf(),
                    reason: format!("required column {table}.{column} is missing"),
                });
            }
        }
    }
    Ok(())
}

fn has_active_custom_columns(connection: &Connection, path: &Path) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM custom_columns WHERE mark_for_delete = 0)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| database_error("inspect custom columns", path, error))
}

#[allow(clippy::needless_pass_by_value)] // map_err supplies owned SQLite errors.
pub(crate) fn database_error(
    operation: &'static str,
    path: &Path,
    error: rusqlite::Error,
) -> Error {
    Error::Database {
        operation,
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}
