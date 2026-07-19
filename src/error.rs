use std::fmt;
use std::path::PathBuf;

/// A result returned by this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// An error with operation and path context where relevant.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A filesystem operation failed.
    Io {
        /// The operation being attempted.
        operation: &'static str,
        /// The path involved in the operation.
        path: PathBuf,
        /// The operating-system error.
        source: std::io::Error,
    },
    /// `SQLite` rejected an operation.
    Database {
        /// The operation being attempted.
        operation: &'static str,
        /// The database path.
        path: PathBuf,
        /// A stable, displayable diagnostic that does not expose `SQLite` types.
        message: String,
    },
    /// The directory is not a recognizable supported Calibre library.
    InvalidLibrary {
        /// The library path.
        path: PathBuf,
        /// Why validation failed.
        reason: String,
    },
    /// The schema cannot safely perform the requested operation.
    UnsupportedSchema {
        /// The operation being attempted.
        operation: &'static str,
        /// The detected schema version.
        detected: u32,
        /// The versions supported for this operation.
        supported: &'static str,
    },
    /// A feature is intentionally unavailable until compatibility is proven.
    UnsupportedOperation {
        /// The operation that was refused.
        operation: &'static str,
        /// The compatibility constraint.
        reason: String,
    },
    /// An ID does not identify an existing row.
    NotFound {
        /// Entity kind.
        entity: &'static str,
        /// Numeric ID.
        id: i64,
    },
    /// Caller input is invalid.
    InvalidInput {
        /// Input field.
        field: &'static str,
        /// Validation detail.
        reason: String,
    },
    /// A database path would escape the library root.
    PathEscape {
        /// The untrusted path.
        path: PathBuf,
        /// Why it was rejected.
        reason: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(f, "{operation} failed for {}: {source}", path.display()),
            Self::Database {
                operation,
                path,
                message,
            } => write!(f, "{operation} failed for {}: {message}", path.display()),
            Self::InvalidLibrary { path, reason } => {
                write!(
                    f,
                    "{} is not a valid Calibre library: {reason}",
                    path.display()
                )
            }
            Self::UnsupportedSchema {
                operation,
                detected,
                supported,
            } => write!(
                f,
                "{operation} is unsupported for schema {detected}; supported: {supported}"
            ),
            Self::UnsupportedOperation { operation, reason } => {
                write!(f, "{operation} is unsupported: {reason}")
            }
            Self::NotFound { entity, id } => write!(f, "{entity} {id} was not found"),
            Self::InvalidInput { field, reason } => write!(f, "invalid {field}: {reason}"),
            Self::PathEscape { path, reason } => {
                write!(f, "unsafe library path {}: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub(crate) fn io_error(
    operation: &'static str,
    path: impl Into<PathBuf>,
    source: std::io::Error,
) -> Error {
    Error::Io {
        operation,
        path: path.into(),
        source,
    }
}
