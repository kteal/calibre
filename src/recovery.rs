use crate::library::LibraryInner;
use crate::{BookId, Error, OpenMode, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const MAGIC: &str = "calibre-rs-recovery-v1";
const RECOVERY_DIRECTORY: &str = ".calibre-rs/recovery";

/// A recoverable multi-step operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RecoveryOperation {
    /// Creation of a book directory and database row.
    BookAdd,
    /// Permanent removal of a book directory and database row.
    PermanentBookRemoval,
}

impl RecoveryOperation {
    const fn name(self) -> &'static str {
        match self {
            Self::BookAdd => "book-add",
            Self::PermanentBookRemoval => "book-remove",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "book-add" => Some(Self::BookAdd),
            "book-remove" => Some(Self::PermanentBookRemoval),
            _ => None,
        }
    }
}

/// One durable recovery record left by an interrupted write.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct RecoveryEntry {
    /// Opaque journal identifier.
    pub journal_id: String,
    /// Interrupted operation.
    pub operation: RecoveryOperation,
    /// Related book.
    pub book_id: BookId,
    /// Original or intended book path relative to the library root.
    pub relative_path: PathBuf,
}

/// Outcome of resolving pending recovery records.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct RecoveryReport {
    /// Records resolved and durably removed.
    pub resolved: u64,
}

pub(crate) struct RecoveryJournal {
    path: PathBuf,
}

impl RecoveryJournal {
    pub(crate) fn begin_book_add(root: &Path, book: BookId, relative: &Path) -> Result<Self> {
        begin(root, RecoveryOperation::BookAdd, book, relative, None)
    }

    pub(crate) fn begin_book_removal(
        root: &Path,
        book: BookId,
        relative: &Path,
        staged: &Path,
    ) -> Result<Self> {
        begin(
            root,
            RecoveryOperation::PermanentBookRemoval,
            book,
            relative,
            Some(staged),
        )
    }

    pub(crate) fn complete(self) -> Result<()> {
        remove_journal(&self.path)
    }
}

#[derive(Clone, Debug)]
struct JournalRecord {
    entry: RecoveryEntry,
    staged_path: Option<PathBuf>,
    journal_path: PathBuf,
}

pub(crate) fn has_pending(root: &Path) -> Result<bool> {
    Ok(!scan(root)?.is_empty())
}

pub(crate) fn ensure_clear(root: &Path, operation: &'static str) -> Result<()> {
    if has_pending(root)? {
        Err(Error::UnsupportedOperation {
            operation,
            reason: "the library has pending recovery records; call Library::recover_pending and reopen it".into(),
        })
    } else {
        Ok(())
    }
}

pub(crate) fn pending(root: &Path) -> Result<Vec<RecoveryEntry>> {
    Ok(scan(root)?.into_iter().map(|record| record.entry).collect())
}

pub(crate) fn recover(inner: &LibraryInner) -> Result<RecoveryReport> {
    if inner.mode != OpenMode::ReadWrite {
        return Err(Error::UnsupportedOperation {
            operation: "recover pending writes",
            reason: "library was opened read-only".into(),
        });
    }
    let records = scan(&inner.root)?;
    let connection = inner.read_connection()?;
    let mut resolved = 0_u64;
    for record in records {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM books WHERE id = ?1)",
                [record.entry.book_id.get()],
                |row| row.get(0),
            )
            .map_err(|error| {
                crate::library::database_error("inspect interrupted book", &inner.database, error)
            })?;
        match record.entry.operation {
            RecoveryOperation::BookAdd => recover_add(inner, &record, exists)?,
            RecoveryOperation::PermanentBookRemoval => {
                recover_removal(inner, &record, exists)?;
            }
        }
        remove_journal(&record.journal_path)?;
        resolved = resolved.saturating_add(1);
    }
    Ok(RecoveryReport { resolved })
}

fn recover_add(inner: &LibraryInner, record: &JournalRecord, book_exists: bool) -> Result<()> {
    let directory = crate::paths::resolve(&inner.root, &record.entry.relative_path)?;
    if book_exists && !directory.is_dir() {
        return Err(Error::UnsupportedOperation {
            operation: "recover interrupted book add",
            reason: format!(
                "book {} exists but its directory is missing: {}",
                record.entry.book_id,
                directory.display()
            ),
        });
    }
    if !book_exists && directory.exists() {
        if !directory.is_dir() {
            return Err(Error::UnsupportedOperation {
                operation: "recover interrupted book add",
                reason: format!("orphan path is not a directory: {}", directory.display()),
            });
        }
        fs::remove_dir_all(&directory).map_err(|source| {
            crate::error::io_error("recover interrupted book add", &directory, source)
        })?;
        if let Some(parent) = directory.parent() {
            if parent != inner.root {
                let _ = fs::remove_dir(parent);
            }
        }
    }
    Ok(())
}

fn recover_removal(inner: &LibraryInner, record: &JournalRecord, book_exists: bool) -> Result<()> {
    let original = crate::paths::resolve(&inner.root, &record.entry.relative_path)?;
    let staged_relative = record
        .staged_path
        .as_ref()
        .ok_or_else(|| Error::InvalidLibrary {
            path: record.journal_path.clone(),
            reason: "book-removal journal has no staged path".into(),
        })?;
    let staged = crate::paths::resolve(&inner.root, staged_relative)?;
    if book_exists {
        if staged.exists() {
            if !staged.is_dir() {
                return Err(Error::UnsupportedOperation {
                    operation: "recover interrupted book removal",
                    reason: format!("staged path is not a directory: {}", staged.display()),
                });
            }
            if original.exists() {
                return Err(Error::UnsupportedOperation {
                    operation: "recover interrupted book removal",
                    reason: format!(
                        "both original and staged directories exist: {} and {}",
                        original.display(),
                        staged.display()
                    ),
                });
            }
            if let Some(parent) = original.parent() {
                fs::create_dir_all(parent).map_err(|source| {
                    crate::error::io_error("create recovery parent", parent, source)
                })?;
            }
            fs::rename(&staged, &original).map_err(|source| {
                crate::error::io_error("restore interrupted book removal", &staged, source)
            })?;
        } else if !original.is_dir() {
            return Err(Error::UnsupportedOperation {
                operation: "recover interrupted book removal",
                reason: format!(
                    "book {} exists but neither original nor staged directory is present",
                    record.entry.book_id
                ),
            });
        }
    } else if staged.exists() {
        if !staged.is_dir() {
            return Err(Error::UnsupportedOperation {
                operation: "recover interrupted book removal",
                reason: format!("staged path is not a directory: {}", staged.display()),
            });
        }
        fs::remove_dir_all(&staged).map_err(|source| {
            crate::error::io_error("finish interrupted book removal", &staged, source)
        })?;
    } else if original.exists() {
        if !original.is_dir() {
            return Err(Error::UnsupportedOperation {
                operation: "recover interrupted book removal",
                reason: format!("original path is not a directory: {}", original.display()),
            });
        }
        fs::remove_dir_all(&original).map_err(|source| {
            crate::error::io_error("remove committed book directory", &original, source)
        })?;
    }
    Ok(())
}

fn begin(
    root: &Path,
    operation: RecoveryOperation,
    book: BookId,
    relative: &Path,
    staged: Option<&Path>,
) -> Result<RecoveryJournal> {
    crate::paths::validate_relative(relative)?;
    if let Some(staged) = staged {
        crate::paths::validate_relative(staged)?;
    }
    let directory = crate::paths::resolve(root, Path::new(RECOVERY_DIRECTORY))?;
    fs::create_dir_all(&directory).map_err(|source| {
        crate::error::io_error("create recovery journal directory", &directory, source)
    })?;
    let id = uuid::Uuid::new_v4().to_string();
    let destination = directory.join(format!("{id}.journal"));
    let mut temporary = tempfile::NamedTempFile::new_in(&directory)
        .map_err(|source| crate::error::io_error("stage recovery journal", &directory, source))?;
    let staged = staged.map_or_else(|| "-".to_owned(), encode_path);
    write!(
        temporary,
        "{MAGIC}\n{}\n{}\n{}\n{staged}\n",
        operation.name(),
        book.get(),
        encode_path(relative)
    )
    .map_err(|source| crate::error::io_error("write recovery journal", &destination, source))?;
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(|source| crate::error::io_error("sync recovery journal", &destination, source))?;
    temporary.persist(&destination).map_err(|error| {
        crate::error::io_error("install recovery journal", &destination, error.error)
    })?;
    sync_directory(&directory)?;
    Ok(RecoveryJournal { path: destination })
}

fn scan(root: &Path) -> Result<Vec<JournalRecord>> {
    let directory = crate::paths::resolve(root, Path::new(RECOVERY_DIRECTORY))?;
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(crate::error::io_error(
                "read recovery journal directory",
                directory,
                source,
            ));
        }
    };
    let mut paths = entries
        .map(|entry| {
            entry.map(|entry| entry.path()).map_err(|source| {
                crate::error::io_error("read recovery journal entry", &directory, source)
            })
        })
        .collect::<Result<Vec<_>>>()?;
    paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "journal")
    });
    paths.sort();
    paths
        .into_iter()
        .map(|path| parse_journal(root, path))
        .collect()
}

fn parse_journal(root: &Path, path: PathBuf) -> Result<JournalRecord> {
    let metadata = fs::symlink_metadata(&path)
        .map_err(|source| crate::error::io_error("inspect recovery journal", &path, source))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error::InvalidLibrary {
            path,
            reason: "recovery journal is not a regular non-symlink file".into(),
        });
    }
    if metadata.len() > 64 * 1024 {
        return Err(Error::InvalidLibrary {
            path,
            reason: "recovery journal exceeds 64 KiB".into(),
        });
    }
    let contents = fs::read_to_string(&path)
        .map_err(|source| crate::error::io_error("read recovery journal", &path, source))?;
    let lines = contents.lines().collect::<Vec<_>>();
    if lines.len() != 5 || lines[0] != MAGIC {
        return Err(Error::InvalidLibrary {
            path,
            reason: "unsupported recovery journal encoding".into(),
        });
    }
    let operation = RecoveryOperation::parse(lines[1]).ok_or_else(|| Error::InvalidLibrary {
        path: path.clone(),
        reason: format!("unknown recovery operation {}", lines[1]),
    })?;
    let raw_book = lines[2].parse::<i64>().map_err(|_| Error::InvalidLibrary {
        path: path.clone(),
        reason: "invalid recovery book ID".into(),
    })?;
    if raw_book <= 0 {
        return Err(Error::InvalidLibrary {
            path,
            reason: "recovery book ID is not positive".into(),
        });
    }
    let relative_path = decode_path(lines[3], &path)?;
    crate::paths::resolve(root, &relative_path)?;
    let staged_path = if lines[4] == "-" {
        None
    } else {
        let staged = decode_path(lines[4], &path)?;
        crate::paths::resolve(root, &staged)?;
        Some(staged)
    };
    let journal_id = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| Error::InvalidLibrary {
            path: path.clone(),
            reason: "recovery journal has a non-UTF-8 filename".into(),
        })?
        .to_owned();
    Ok(JournalRecord {
        entry: RecoveryEntry {
            journal_id,
            operation,
            book_id: BookId::new(raw_book),
            relative_path,
        },
        staged_path,
        journal_path: path,
    })
}

fn encode_path(path: &Path) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(windows)]
    let bytes = {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    };
    #[cfg(not(any(unix, windows)))]
    let bytes = path.as_os_str().as_encoded_bytes().to_vec();

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_path(value: &str, journal: &Path) -> Result<PathBuf> {
    if value.len() % 2 != 0 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::InvalidLibrary {
            path: journal.to_path_buf(),
            reason: "invalid recovery path encoding".into(),
        });
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = hex_nibble(pair[0]).ok_or_else(|| Error::InvalidLibrary {
            path: journal.to_path_buf(),
            reason: "invalid recovery path encoding".into(),
        })?;
        let low = hex_nibble(pair[1]).ok_or_else(|| Error::InvalidLibrary {
            path: journal.to_path_buf(),
            reason: "invalid recovery path encoding".into(),
        })?;
        bytes.push((high << 4) | low);
    }
    #[cfg(unix)]
    let path = {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(bytes))
    };
    #[cfg(windows)]
    let path = {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        if bytes.len() % 2 != 0 {
            return Err(Error::InvalidLibrary {
                path: journal.to_path_buf(),
                reason: "invalid Windows recovery path encoding".into(),
            });
        }
        let wide = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        PathBuf::from(OsString::from_wide(&wide))
    };
    #[cfg(not(any(unix, windows)))]
    let path = PathBuf::from(String::from_utf8(bytes).map_err(|_| Error::InvalidLibrary {
        path: journal.to_path_buf(),
        reason: "recovery path is not valid platform text".into(),
    })?);
    crate::paths::validate_relative(&path)?;
    Ok(path)
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn remove_journal(path: &Path) -> Result<()> {
    fs::remove_file(path)
        .map_err(|source| crate::error::io_error("remove recovery journal", path, source))?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
        if fs::remove_dir(parent).is_ok() {
            if let Some(metadata_directory) = parent.parent() {
                let _ = fs::remove_dir(metadata_directory);
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| crate::error::io_error("sync recovery directory", path, source))?;
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)] // Keeps fallible call sites shared with the Unix implementation.
const fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}
