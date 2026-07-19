use crate::library::{LibraryInner, database_error};
use crate::{BookId, Result};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;

/// The kind of inconsistency found during a read-only library audit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AuditIssueKind {
    /// `SQLite`'s quick integrity check reported a problem.
    DatabaseIntegrity,
    /// A stored book path was unsafe.
    UnsafeBookPath,
    /// A stored format path was unsafe.
    UnsafeFormatPath,
    /// A stored cover path was unsafe.
    UnsafeCoverPath,
    /// A book directory was missing.
    MissingBookDirectory,
    /// A format row used an invalid logical format.
    MalformedFormat,
    /// A format row had no corresponding file.
    MissingFormat,
    /// The stored and observed format sizes differed.
    FormatSizeMismatch,
    /// `has_cover` was set but `cover.jpg` was missing.
    MissingCover,
    /// `cover.jpg` existed while `has_cover` was clear.
    UntrackedCover,
}

/// One audit finding.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct AuditIssue {
    /// Finding category.
    pub kind: AuditIssueKind,
    /// Related book when known.
    pub book_id: Option<BookId>,
    /// Related path when known.
    pub path: Option<PathBuf>,
    /// Diagnostic detail.
    pub detail: String,
}

/// Results of a non-mutating library audit.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct AuditReport {
    /// Books inspected.
    pub books_checked: u64,
    /// Format rows inspected.
    pub formats_checked: u64,
    /// Findings in deterministic book and format order.
    pub issues: Vec<AuditIssue>,
}

impl AuditReport {
    /// Returns true when no issue was found.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

/// Read-only consistency checks for one library.
#[derive(Clone, Debug)]
pub struct Auditor {
    inner: Arc<LibraryInner>,
}

impl Auditor {
    pub(crate) const fn new(inner: Arc<LibraryInner>) -> Self {
        Self { inner }
    }

    /// Checks database integrity and core filesystem state without mutation.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot execute the audit queries or the
    /// filesystem cannot be inspected. Unsafe stored paths become findings.
    pub fn run(&self) -> Result<AuditReport> {
        let connection = self.inner.read_connection()?;
        let mut issues = Vec::new();
        let mut integrity = connection.prepare("PRAGMA quick_check").map_err(|error| {
            database_error("prepare integrity check", &self.inner.database, error)
        })?;
        let messages = integrity
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| database_error("run integrity check", &self.inner.database, error))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| database_error("read integrity check", &self.inner.database, error))?;
        for message in messages.into_iter().filter(|message| message != "ok") {
            issues.push(AuditIssue {
                kind: AuditIssueKind::DatabaseIntegrity,
                book_id: None,
                path: Some(self.inner.database.clone()),
                detail: message,
            });
        }

        let mut books_statement = connection
            .prepare("SELECT id, path, has_cover FROM books ORDER BY id")
            .map_err(|error| database_error("prepare audit books", &self.inner.database, error))?;
        let books = books_statement
            .query_map([], |row| {
                Ok((
                    BookId::new(row.get(0)?),
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })
            .map_err(|error| database_error("query audit books", &self.inner.database, error))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| database_error("read audit books", &self.inner.database, error))?;

        let mut formats_checked = 0_u64;
        for (book_id, stored_path, has_cover) in &books {
            let relative = PathBuf::from(stored_path);
            let directory = match crate::paths::resolve(&self.inner.root, &relative) {
                Ok(path) => path,
                Err(error) => {
                    issues.push(AuditIssue {
                        kind: AuditIssueKind::UnsafeBookPath,
                        book_id: Some(*book_id),
                        path: Some(relative),
                        detail: error.to_string(),
                    });
                    continue;
                }
            };
            if !directory.is_dir() {
                issues.push(AuditIssue {
                    kind: AuditIssueKind::MissingBookDirectory,
                    book_id: Some(*book_id),
                    path: Some(directory),
                    detail: "book directory does not exist".into(),
                });
                continue;
            }
            audit_cover(&self.inner, *book_id, &relative, *has_cover, &mut issues);

            let mut format_statement = connection
                .prepare(
                    "SELECT format, uncompressed_size, name FROM data \
                     WHERE book = ?1 ORDER BY format COLLATE NOCASE, id",
                )
                .map_err(|error| {
                    database_error("prepare audit formats", &self.inner.database, error)
                })?;
            let formats = format_statement
                .query_map([book_id.get()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|error| {
                    database_error("query audit formats", &self.inner.database, error)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|error| {
                    database_error("read audit formats", &self.inner.database, error)
                })?;
            formats_checked = formats_checked.saturating_add(formats.len() as u64);
            for (format, stored_size, stem) in formats {
                audit_format(
                    &self.inner,
                    *book_id,
                    &directory,
                    &format,
                    stored_size,
                    &stem,
                    &mut issues,
                )?;
            }
        }

        Ok(AuditReport {
            books_checked: books.len() as u64,
            formats_checked,
            issues,
        })
    }
}

fn audit_cover(
    inner: &LibraryInner,
    book_id: BookId,
    book_relative: &std::path::Path,
    has_cover: bool,
    issues: &mut Vec<AuditIssue>,
) {
    let cover_relative = book_relative.join("cover.jpg");
    let cover = match crate::paths::resolve(&inner.root, &cover_relative) {
        Ok(path) => path,
        Err(error) => {
            issues.push(AuditIssue {
                kind: AuditIssueKind::UnsafeCoverPath,
                book_id: Some(book_id),
                path: Some(cover_relative),
                detail: error.to_string(),
            });
            return;
        }
    };
    match (has_cover, cover.is_file()) {
        (true, false) => issues.push(AuditIssue {
            kind: AuditIssueKind::MissingCover,
            book_id: Some(book_id),
            path: Some(cover),
            detail: "database records a cover but cover.jpg is missing".into(),
        }),
        (false, true) => issues.push(AuditIssue {
            kind: AuditIssueKind::UntrackedCover,
            book_id: Some(book_id),
            path: Some(cover),
            detail: "cover.jpg exists but has_cover is clear".into(),
        }),
        _ => {}
    }
}

fn audit_format(
    inner: &LibraryInner,
    book_id: BookId,
    directory: &std::path::Path,
    format: &str,
    stored_size: i64,
    stem: &str,
    issues: &mut Vec<AuditIssue>,
) -> Result<()> {
    let normalized = match crate::paths::format_name(OsStr::new(format)) {
        Ok(value) => value,
        Err(error) => {
            issues.push(AuditIssue {
                kind: AuditIssueKind::MalformedFormat,
                book_id: Some(book_id),
                path: None,
                detail: error.to_string(),
            });
            return Ok(());
        }
    };
    let candidate = directory.join(format!("{stem}.{}", normalized.to_ascii_lowercase()));
    let Ok(relative) = candidate.strip_prefix(&inner.root) else {
        issues.push(AuditIssue {
            kind: AuditIssueKind::UnsafeFormatPath,
            book_id: Some(book_id),
            path: Some(candidate),
            detail: "format candidate is outside the library root".into(),
        });
        return Ok(());
    };
    let path = match crate::paths::resolve(&inner.root, relative) {
        Ok(value) => value,
        Err(error) => {
            issues.push(AuditIssue {
                kind: AuditIssueKind::UnsafeFormatPath,
                book_id: Some(book_id),
                path: Some(relative.to_path_buf()),
                detail: error.to_string(),
            });
            return Ok(());
        }
    };
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => {
            if u64::try_from(stored_size).ok() != Some(metadata.len()) {
                issues.push(AuditIssue {
                    kind: AuditIssueKind::FormatSizeMismatch,
                    book_id: Some(book_id),
                    path: Some(path),
                    detail: format!(
                        "stored size {stored_size} differs from filesystem size {}",
                        metadata.len()
                    ),
                });
            }
        }
        Ok(_) => issues.push(AuditIssue {
            kind: AuditIssueKind::MissingFormat,
            book_id: Some(book_id),
            path: Some(path),
            detail: "format path is not a regular file".into(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => issues.push(AuditIssue {
            kind: AuditIssueKind::MissingFormat,
            book_id: Some(book_id),
            path: Some(path),
            detail: "format file does not exist".into(),
        }),
        Err(error) => {
            return Err(crate::error::io_error(
                "inspect audited format",
                path,
                error,
            ));
        }
    }
    Ok(())
}
