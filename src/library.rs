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
pub(crate) const CALIBRE_APPLICATION_ID: u32 = 0x6361_6c69;

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

/// A validated handle to a Calibre library.
#[derive(Clone, Debug)]
pub struct Library {
    pub(crate) inner: Arc<LibraryInner>,
}

impl Library {
    /// Creates and opens a new schema-27 Calibre library.
    ///
    /// The target may be missing or an existing empty directory on a
    /// filesystem that supports hard links. Creation refuses every non-empty
    /// directory and never replaces `metadata.db`.
    /// The database is built under a private name inside the validated target
    /// root, validated, synced, and then published without replacement.
    ///
    /// The returned handle is read-write. The initialized schema supports this
    /// crate's core metadata, format, cover, recovery, audit, and trash
    /// operations. Custom-column writes, annotation search, and FTS maintenance
    /// remain unsupported.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe or non-empty target, filesystem failure,
    /// database initialization failure, or failed post-creation validation.
    pub fn create(path: impl AsRef<Path>) -> Result<Self> {
        Self::create_with_hook(path.as_ref(), |_| Ok(()))
    }

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

    fn create_with_hook(
        supplied: &Path,
        mut hook: impl FnMut(CreationPhase) -> Result<()>,
    ) -> Result<Self> {
        let (root, created_root) = prepare_creation_root(supplied)?;
        let database = root.join("metadata.db");
        let staged = root.join(format!(
            ".calibre-rs-metadata-{}.tmp",
            Uuid::new_v4().simple()
        ));
        let mut published = false;
        let result = (|| {
            hook(CreationPhase::RootPrepared)?;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)
                .map_err(|source| {
                    crate::error::io_error("create staged metadata.db", &staged, source)
                })?;
            let mut connection =
                open_connection(&staged, OpenMode::ReadWrite, Duration::from_secs(5))?;
            crate::schema::initialize(&mut connection, &staged)?;
            hook(CreationPhase::SchemaCommitted)?;
            validate_created_database(&connection, &staged)?;
            drop(connection);
            std::fs::OpenOptions::new()
                .write(true)
                .open(&staged)
                .and_then(|file| file.sync_all())
                .map_err(|source| {
                    crate::error::io_error("sync staged metadata.db", &staged, source)
                })?;
            std::fs::hard_link(&staged, &database).map_err(|source| {
                crate::error::io_error("publish metadata.db", &database, source)
            })?;
            published = true;
            hook(CreationPhase::Published)?;
            std::fs::remove_file(&staged).map_err(|source| {
                crate::error::io_error("remove staged metadata.db link", &staged, source)
            })?;
            sync_creation_directory(&root)?;
            Self::open_with(&root, OpenOptions::new().read_write(true))
        })();
        if result.is_err() {
            cleanup_failed_creation(&root, &staged, &database, published, created_root)?;
        }
        result
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

pub(crate) fn validate_schema(connection: &Connection, path: &Path) -> Result<()> {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CreationPhase {
    RootPrepared,
    SchemaCommitted,
    Published,
}

fn prepare_creation_root(supplied: &Path) -> Result<(PathBuf, bool)> {
    match std::fs::symlink_metadata(supplied) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(Error::InvalidLibrary {
                    path: supplied.to_path_buf(),
                    reason: "creation target must be a real directory".into(),
                });
            }
            let root = std::fs::canonicalize(supplied).map_err(|source| {
                crate::error::io_error("canonicalize creation target", supplied, source)
            })?;
            let mut entries = std::fs::read_dir(&root).map_err(|source| {
                crate::error::io_error("inspect creation target", &root, source)
            })?;
            if entries
                .next()
                .transpose()
                .map_err(|source| crate::error::io_error("inspect creation target", &root, source))?
                .is_some()
            {
                return Err(Error::InvalidLibrary {
                    path: root,
                    reason: "creation target is not empty".into(),
                });
            }
            Ok((root, false))
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            let parent = creation_parent(supplied);
            let parent = std::fs::canonicalize(parent).map_err(|source| {
                crate::error::io_error("canonicalize creation parent", parent, source)
            })?;
            let name = supplied.file_name().ok_or_else(|| Error::InvalidLibrary {
                path: supplied.to_path_buf(),
                reason: "creation target has no directory name".into(),
            })?;
            let root = parent.join(name);
            std::fs::create_dir(&root).map_err(|source| {
                crate::error::io_error("create library directory", &root, source)
            })?;
            let canonical = std::fs::canonicalize(&root).map_err(|source| {
                crate::error::io_error("canonicalize created library", &root, source)
            })?;
            if canonical != root {
                let _ = std::fs::remove_dir(&root);
                return Err(Error::PathEscape {
                    path: supplied.to_path_buf(),
                    reason: "created library root did not resolve to its validated target".into(),
                });
            }
            Ok((canonical, true))
        }
        Err(source) => Err(crate::error::io_error(
            "inspect creation target",
            supplied,
            source,
        )),
    }
}

fn creation_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn validate_created_database(connection: &Connection, database: &Path) -> Result<()> {
    let schema_version = pragma_u32(connection, "user_version", database)?;
    let application_id = pragma_u32(connection, "application_id", database)?;
    if schema_version != SUPPORTED_SCHEMA_VERSION || application_id != CALIBRE_APPLICATION_ID {
        return Err(Error::InvalidLibrary {
            path: database.to_path_buf(),
            reason: "new database has an unexpected schema identity".into(),
        });
    }
    validate_schema(connection, database)?;
    let books: i64 = connection
        .query_row("SELECT count(*) FROM books", [], |row| row.get(0))
        .map_err(|error| database_error("validate empty catalog", database, error))?;
    if books != 0 {
        return Err(Error::InvalidLibrary {
            path: database.to_path_buf(),
            reason: "new database catalog is not empty".into(),
        });
    }
    Ok(())
}

fn cleanup_failed_creation(
    root: &Path,
    staged: &Path,
    database: &Path,
    published: bool,
    created_root: bool,
) -> Result<()> {
    remove_creation_file(staged)?;
    if published {
        remove_creation_file(database)?;
    }
    for suffix in ["-journal", "-wal", "-shm"] {
        remove_creation_file(&path_with_suffix(staged, suffix))?;
        remove_creation_file(&path_with_suffix(database, suffix))?;
    }
    if created_root {
        std::fs::remove_dir(root).map_err(|source| {
            crate::error::io_error("remove failed library directory", root, source)
        })?;
    }
    Ok(())
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn remove_creation_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(crate::error::io_error(
            "clean up failed library creation",
            path,
            source,
        )),
    }
}

#[cfg(unix)]
fn sync_creation_directory(path: &Path) -> Result<()> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| crate::error::io_error("sync created library", path, source))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
const fn sync_creation_directory(_path: &Path) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::{CreationPhase, Library};
    use crate::Error;

    #[test]
    fn injected_creation_failures_remove_every_staged_artifact() {
        for phase in [
            CreationPhase::RootPrepared,
            CreationPhase::SchemaCommitted,
            CreationPhase::Published,
        ] {
            let parent = tempfile::tempdir().expect("temporary creation parent");
            let target = parent.path().join(format!("failure-{phase:?}"));
            let error = Library::create_with_hook(&target, |current| {
                if current == phase {
                    Err(Error::InvalidInput {
                        field: "injected creation failure",
                        reason: format!("failed at {phase:?}"),
                    })
                } else {
                    Ok(())
                }
            })
            .expect_err("injected failure");
            assert!(
                matches!(
                    &error,
                    Error::InvalidInput {
                        field: "injected creation failure",
                        ..
                    }
                ),
                "unexpected error at {phase:?}: {error:?}"
            );
            assert!(
                !target.exists(),
                "created root must be removed at {phase:?}"
            );
        }
    }

    #[test]
    fn injected_creation_failure_preserves_an_existing_empty_target() {
        let parent = tempfile::tempdir().expect("temporary creation parent");
        let target = parent.path().join("empty");
        std::fs::create_dir(&target).expect("empty target");
        Library::create_with_hook(&target, |phase| {
            if phase == CreationPhase::SchemaCommitted {
                Err(Error::InvalidInput {
                    field: "injected creation failure",
                    reason: "after schema".into(),
                })
            } else {
                Ok(())
            }
        })
        .expect_err("injected failure");
        assert!(target.is_dir());
        assert_eq!(
            std::fs::read_dir(&target).expect("inspect target").count(),
            0
        );
    }

    #[test]
    fn creation_does_not_replace_metadata_published_by_a_racer() {
        let parent = tempfile::tempdir().expect("temporary creation parent");
        let target = parent.path().join("empty");
        std::fs::create_dir(&target).expect("empty target");
        let database = target.join("metadata.db");
        Library::create_with_hook(&target, |phase| {
            if phase == CreationPhase::SchemaCommitted {
                std::fs::write(&database, b"concurrent owner").expect("racing metadata.db");
            }
            Ok(())
        })
        .expect_err("no-clobber publication");
        assert_eq!(
            std::fs::read(database).expect("preserved racing metadata.db"),
            b"concurrent owner"
        );
    }

    #[test]
    fn bare_relative_creation_uses_the_current_directory_as_parent() {
        assert_eq!(
            super::creation_parent(std::path::Path::new("relative-library")),
            std::path::Path::new(".")
        );
    }
}
