use crate::library::LibraryInner;
use crate::{BookId, Error, OpenMode, Result};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const V1_MAGIC: &str = "calibre-rs-recovery-v1";
const V2_VERSION: u8 = 2;
const RECOVERY_DIRECTORY: &str = ".calibre-rs/recovery";
const MAX_JOURNAL_SIZE: u64 = 1024 * 1024;

/// A recoverable multi-step operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RecoveryOperation {
    /// Creation of a book directory and database row.
    BookAdd,
    /// Permanent removal of a book directory and database row.
    PermanentBookRemoval,
    /// Addition, replacement, or removal of a format.
    FormatChange,
    /// Addition, replacement, or removal of a cover.
    CoverChange,
    /// A book-directory and format-filename move.
    BookMove,
    /// A book or format moving into or out of Calibre's trash.
    TrashChange,
}

impl RecoveryOperation {
    fn v1_name(self) -> &'static str {
        match self {
            Self::BookAdd => "book-add",
            Self::PermanentBookRemoval => "book-remove",
            Self::FormatChange | Self::CoverChange | Self::BookMove | Self::TrashChange => {
                unreachable!("new operations use version-2 journals")
            }
        }
    }

    fn parse_v1(value: &str) -> Option<Self> {
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
    /// Primary path relative to the library root.
    pub relative_path: PathBuf,
}

/// Outcome of resolving pending recovery records.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct RecoveryReport {
    /// Records resolved and durably removed.
    pub resolved: u64,
}

/// Database state for one format row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FormatRecoveryState {
    pub(crate) format: String,
    pub(crate) size: i64,
    pub(crate) stem: String,
}

/// One existing format file rename performed during a book move.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveryFileRename {
    pub(crate) old_name: PathBuf,
    pub(crate) new_name: PathBuf,
}

pub(crate) struct RecoveryJournal {
    path: PathBuf,
    record: Option<JournalV2>,
}

impl RecoveryJournal {
    pub(crate) fn begin_book_add(root: &Path, book: BookId, relative: &Path) -> Result<Self> {
        begin_v1(root, RecoveryOperation::BookAdd, book, relative, None)
    }

    pub(crate) fn begin_book_removal(
        root: &Path,
        book: BookId,
        relative: &Path,
        staged: &Path,
    ) -> Result<Self> {
        begin_v1(
            root,
            RecoveryOperation::PermanentBookRemoval,
            book,
            relative,
            Some(staged),
        )
    }

    #[allow(clippy::too_many_arguments)] // The record must capture every recovery boundary.
    pub(crate) fn begin_format_write(
        root: &Path,
        book: BookId,
        destination: &Path,
        backup: &Path,
        staged: &Path,
        before_file: bool,
        before: Option<FormatRecoveryState>,
        format: &str,
        stem: &str,
    ) -> Result<Self> {
        let record = AssetRecord {
            phase: AssetPhase::Staging,
            destination: encode_root_path(root, destination)?,
            backup: encode_root_path(root, backup)?,
            staged: Some(encode_root_path(root, staged)?),
            before_file,
            database: AssetDatabaseRecord::Format {
                before: before.map(FormatSnapshot::complete),
                after: Some(FormatSnapshot {
                    format: format.to_owned(),
                    size: None,
                    stem: stem.to_owned(),
                }),
            },
        };
        begin_v2(root, book, OperationV2::Asset(record))
    }

    pub(crate) fn begin_format_removal(
        root: &Path,
        book: BookId,
        destination: &Path,
        backup: &Path,
        before_file: bool,
        before: FormatRecoveryState,
    ) -> Result<Self> {
        let record = AssetRecord {
            phase: AssetPhase::Ready,
            destination: encode_root_path(root, destination)?,
            backup: encode_root_path(root, backup)?,
            staged: None,
            before_file,
            database: AssetDatabaseRecord::Format {
                before: Some(FormatSnapshot::complete(before)),
                after: None,
            },
        };
        begin_v2(root, book, OperationV2::Asset(record))
    }

    #[allow(clippy::too_many_arguments)] // The record must capture every recovery boundary.
    pub(crate) fn begin_cover_write(
        root: &Path,
        book: BookId,
        destination: &Path,
        backup: &Path,
        staged: &Path,
        before_file: bool,
        before: bool,
    ) -> Result<Self> {
        let record = AssetRecord {
            phase: AssetPhase::Staging,
            destination: encode_root_path(root, destination)?,
            backup: encode_root_path(root, backup)?,
            staged: Some(encode_root_path(root, staged)?),
            before_file,
            database: AssetDatabaseRecord::Cover {
                before,
                after: true,
            },
        };
        begin_v2(root, book, OperationV2::Asset(record))
    }

    pub(crate) fn begin_cover_removal(
        root: &Path,
        book: BookId,
        destination: &Path,
        backup: &Path,
        before_file: bool,
    ) -> Result<Self> {
        let record = AssetRecord {
            phase: AssetPhase::Ready,
            destination: encode_root_path(root, destination)?,
            backup: encode_root_path(root, backup)?,
            staged: None,
            before_file,
            database: AssetDatabaseRecord::Cover {
                before: true,
                after: false,
            },
        };
        begin_v2(root, book, OperationV2::Asset(record))
    }

    pub(crate) fn begin_book_move(
        root: &Path,
        book: BookId,
        old_directory: &Path,
        new_directory: &Path,
        new_stem: &str,
        files: &[RecoveryFileRename],
    ) -> Result<Self> {
        let files = files
            .iter()
            .map(|rename| {
                validate_filename(&rename.old_name)?;
                validate_filename(&rename.new_name)?;
                Ok(FileRenameRecord {
                    old_name: encode_path(&rename.old_name),
                    new_name: encode_path(&rename.new_name),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let record = BookMoveRecord {
            old_directory: encode_root_path(root, old_directory)?,
            new_directory: encode_root_path(root, new_directory)?,
            new_stem: new_stem.to_owned(),
            files,
        };
        begin_v2(root, book, OperationV2::BookMove(record))
    }

    #[allow(clippy::too_many_arguments)] // Recovery needs both paths, direction, kind, and listing state.
    pub(crate) fn begin_trash_change(
        root: &Path,
        book: BookId,
        live: &Path,
        trash: &Path,
        previous: Option<&Path>,
        direction: TrashDirection,
        kind: TrashAssetKind,
        listing: Option<(&str, &[String])>,
    ) -> Result<Self> {
        let record = TrashRecord {
            direction,
            kind,
            live: encode_root_path(root, live)?,
            trash: encode_root_path(root, trash)?,
            previous: previous
                .map(|path| encode_root_path(root, path))
                .transpose()?,
            listing: listing.map(|(title, authors)| TrashListingRecord {
                title: title.to_owned(),
                authors: authors.to_vec(),
            }),
        };
        begin_v2(root, book, OperationV2::Trash(record))
    }

    pub(crate) fn mark_format_ready(&mut self, size: i64) -> Result<()> {
        let path = self.path.clone();
        let record = self.v2_record_mut("mark format journal ready")?;
        let OperationV2::Asset(asset) = &mut record.operation else {
            return Err(journal_state_error("mark format journal ready"));
        };
        let AssetDatabaseRecord::Format { after, .. } = &mut asset.database else {
            return Err(journal_state_error("mark format journal ready"));
        };
        let after = after
            .as_mut()
            .ok_or_else(|| journal_state_error("mark format journal ready"))?;
        after.size = Some(size);
        asset.phase = AssetPhase::Ready;
        rewrite_v2(&path, record)
    }

    pub(crate) fn mark_cover_ready(&mut self) -> Result<()> {
        let path = self.path.clone();
        let record = self.v2_record_mut("mark cover journal ready")?;
        let OperationV2::Asset(asset) = &mut record.operation else {
            return Err(journal_state_error("mark cover journal ready"));
        };
        if !matches!(asset.database, AssetDatabaseRecord::Cover { .. }) {
            return Err(journal_state_error("mark cover journal ready"));
        }
        asset.phase = AssetPhase::Ready;
        rewrite_v2(&path, record)
    }

    fn v2_record_mut(&mut self, operation: &'static str) -> Result<&mut JournalV2> {
        self.record
            .as_mut()
            .ok_or_else(|| journal_state_error(operation))
    }

    pub(crate) fn complete(self) -> Result<()> {
        remove_journal(&self.path)
    }
}

#[derive(Clone, Debug)]
struct ParsedJournal {
    entry: RecoveryEntry,
    detail: JournalDetail,
    journal_path: PathBuf,
}

#[derive(Clone, Debug)]
enum JournalDetail {
    BookAdd,
    BookRemoval { staged_path: PathBuf },
    V2(JournalV2),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct JournalV2 {
    version: u8,
    book_id: i64,
    operation: OperationV2,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "kebab-case")]
enum OperationV2 {
    Asset(AssetRecord),
    BookMove(BookMoveRecord),
    Trash(TrashRecord),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TrashDirection {
    ToTrash,
    FromTrash,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "asset", rename_all = "kebab-case")]
pub(crate) enum TrashAssetKind {
    Book,
    Format { format: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TrashRecord {
    direction: TrashDirection,
    kind: TrashAssetKind,
    live: String,
    trash: String,
    previous: Option<String>,
    listing: Option<TrashListingRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TrashListingRecord {
    title: String,
    authors: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AssetRecord {
    phase: AssetPhase,
    destination: String,
    backup: String,
    staged: Option<String>,
    before_file: bool,
    database: AssetDatabaseRecord,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum AssetPhase {
    Staging,
    Ready,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "asset", rename_all = "kebab-case")]
enum AssetDatabaseRecord {
    Format {
        before: Option<FormatSnapshot>,
        after: Option<FormatSnapshot>,
    },
    Cover {
        before: bool,
        after: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FormatSnapshot {
    format: String,
    size: Option<i64>,
    stem: String,
}

impl FormatSnapshot {
    fn complete(state: FormatRecoveryState) -> Self {
        Self {
            format: state.format,
            size: Some(state.size),
            stem: state.stem,
        }
    }

    fn to_complete(&self, journal: &Path) -> Result<FormatRecoveryState> {
        let size = self.size.ok_or_else(|| Error::InvalidLibrary {
            path: journal.to_path_buf(),
            reason: "ready format journal has no size".into(),
        })?;
        Ok(FormatRecoveryState {
            format: self.format.clone(),
            size,
            stem: self.stem.clone(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BookMoveRecord {
    old_directory: String,
    new_directory: String,
    new_stem: String,
    files: Vec<FileRenameRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FileRenameRecord {
    old_name: String,
    new_name: String,
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
    let mut connection = inner.recovery_connection()?;
    let mut resolved = 0_u64;
    for record in records {
        match &record.detail {
            JournalDetail::BookAdd => {
                let exists = book_exists(&connection, inner, record.entry.book_id)?;
                recover_add(inner, &record, exists)?;
            }
            JournalDetail::BookRemoval { staged_path } => {
                let exists = book_exists(&connection, inner, record.entry.book_id)?;
                recover_removal(inner, &record, staged_path, exists)?;
            }
            JournalDetail::V2(journal) => match &journal.operation {
                OperationV2::Asset(asset) => {
                    recover_asset(inner, &mut connection, &record, asset)?;
                }
                OperationV2::BookMove(book_move) => {
                    recover_book_move(inner, &mut connection, &record, book_move)?;
                }
                OperationV2::Trash(trash) => {
                    recover_trash(inner, &connection, &record, trash)?;
                }
            },
        }
        remove_journal(&record.journal_path)?;
        resolved = resolved.saturating_add(1);
    }
    Ok(RecoveryReport { resolved })
}

fn book_exists(
    connection: &rusqlite::Connection,
    inner: &LibraryInner,
    book: BookId,
) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM books WHERE id = ?1)",
            [book.get()],
            |row| row.get(0),
        )
        .map_err(|error| {
            crate::library::database_error("inspect interrupted book", &inner.database, error)
        })
}

fn recover_add(inner: &LibraryInner, record: &ParsedJournal, book_exists: bool) -> Result<()> {
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
    if !book_exists && path_exists(&directory)? {
        ensure_directory(&directory, "recover interrupted book add")?;
        fs::remove_dir_all(&directory).map_err(|source| {
            crate::error::io_error("recover interrupted book add", &directory, source)
        })?;
        remove_empty_parent(&directory, &inner.root);
    }
    Ok(())
}

fn recover_removal(
    inner: &LibraryInner,
    record: &ParsedJournal,
    staged_relative: &Path,
    book_exists: bool,
) -> Result<()> {
    let original = crate::paths::resolve(&inner.root, &record.entry.relative_path)?;
    let staged = crate::paths::resolve(&inner.root, staged_relative)?;
    if book_exists {
        if path_exists(&staged)? {
            ensure_directory(&staged, "recover interrupted book removal")?;
            if path_exists(&original)? {
                return Err(ambiguous_paths_error(
                    "recover interrupted book removal",
                    &original,
                    &staged,
                ));
            }
            create_parent(&original, "create recovery parent")?;
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
    } else if path_exists(&staged)? {
        ensure_directory(&staged, "recover interrupted book removal")?;
        fs::remove_dir_all(&staged).map_err(|source| {
            crate::error::io_error("finish interrupted book removal", &staged, source)
        })?;
    } else if path_exists(&original)? {
        ensure_directory(&original, "recover interrupted book removal")?;
        fs::remove_dir_all(&original).map_err(|source| {
            crate::error::io_error("remove committed book directory", &original, source)
        })?;
    }
    Ok(())
}

fn recover_asset(
    inner: &LibraryInner,
    connection: &mut rusqlite::Connection,
    parsed: &ParsedJournal,
    asset: &AssetRecord,
) -> Result<()> {
    validate_asset_location(connection, inner, parsed.entry.book_id, asset)?;
    let current = current_asset_database(connection, inner, parsed.entry.book_id, asset)?;
    let before = expected_asset_database(asset, false, &parsed.journal_path)?;
    if asset.phase == AssetPhase::Staging {
        if current != before {
            return Err(ambiguous_database_error("recover staged asset", parsed));
        }
        reconcile_asset_files(inner, asset, false)?;
        return Ok(());
    }
    let after = expected_asset_database(asset, true, &parsed.journal_path)?;
    let roll_forward = if current == after {
        true
    } else if current == before {
        before == after
    } else {
        return Err(ambiguous_database_error("recover asset change", parsed));
    };
    reconcile_asset_files(inner, asset, roll_forward)?;
    if roll_forward {
        apply_asset_after(connection, inner, parsed.entry.book_id, asset)?;
    }
    Ok(())
}

fn validate_asset_location(
    connection: &rusqlite::Connection,
    inner: &LibraryInner,
    book: BookId,
    asset: &AssetRecord,
) -> Result<()> {
    let relative: String = connection
        .query_row(
            "SELECT path FROM books WHERE id = ?1",
            [book.get()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| {
            crate::library::database_error(
                "inspect interrupted asset book path",
                &inner.database,
                error,
            )
        })?
        .ok_or_else(|| Error::NotFound {
            entity: "book",
            id: book.get(),
        })?;
    let directory = crate::paths::resolve(&inner.root, Path::new(&relative))?;
    let destination = decode_root_path(&inner.root, &asset.destination, &inner.database)?;
    if destination.parent() != Some(directory.as_path()) {
        return Err(Error::PathEscape {
            path: destination,
            reason: "recovery asset is outside its book directory".into(),
        });
    }
    let expected_name = match &asset.database {
        AssetDatabaseRecord::Format { before, after } => {
            let snapshot =
                before
                    .as_ref()
                    .or(after.as_ref())
                    .ok_or_else(|| Error::InvalidLibrary {
                        path: inner.database.clone(),
                        reason: "format journal has neither before nor after state".into(),
                    })?;
            format!("{}.{}", snapshot.stem, snapshot.format.to_ascii_lowercase())
        }
        AssetDatabaseRecord::Cover { .. } => "cover.jpg".to_owned(),
    };
    if destination.file_name() != Some(std::ffi::OsStr::new(&expected_name)) {
        return Err(Error::InvalidLibrary {
            path: destination,
            reason: "recovery asset filename does not match its database state".into(),
        });
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AssetDatabaseState {
    Format(Option<FormatRecoveryState>),
    Cover(bool),
}

fn current_asset_database(
    connection: &rusqlite::Connection,
    inner: &LibraryInner,
    book: BookId,
    asset: &AssetRecord,
) -> Result<AssetDatabaseState> {
    match &asset.database {
        AssetDatabaseRecord::Format { before, after } => {
            let format = before
                .as_ref()
                .or(after.as_ref())
                .ok_or_else(|| Error::InvalidLibrary {
                    path: inner.database.clone(),
                    reason: "format journal has neither before nor after state".into(),
                })?
                .format
                .clone();
            let state = connection
                .query_row(
                    "SELECT format, uncompressed_size, name FROM data \
                     WHERE book = ?1 AND format = ?2 COLLATE NOCASE",
                    params![book.get(), format],
                    |row| {
                        Ok(FormatRecoveryState {
                            format: row.get(0)?,
                            size: row.get(1)?,
                            stem: row.get(2)?,
                        })
                    },
                )
                .optional()
                .map_err(|error| {
                    crate::library::database_error(
                        "inspect interrupted format",
                        &inner.database,
                        error,
                    )
                })?;
            Ok(AssetDatabaseState::Format(state))
        }
        AssetDatabaseRecord::Cover { .. } => connection
            .query_row(
                "SELECT has_cover FROM books WHERE id = ?1",
                [book.get()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| {
                crate::library::database_error("inspect interrupted cover", &inner.database, error)
            })?
            .map(AssetDatabaseState::Cover)
            .ok_or_else(|| Error::NotFound {
                entity: "book",
                id: book.get(),
            }),
    }
}

fn expected_asset_database(
    asset: &AssetRecord,
    after: bool,
    journal: &Path,
) -> Result<AssetDatabaseState> {
    match &asset.database {
        AssetDatabaseRecord::Format {
            before,
            after: after_state,
        } => {
            let state = if after { after_state } else { before };
            Ok(AssetDatabaseState::Format(
                state
                    .as_ref()
                    .map(|snapshot| snapshot.to_complete(journal))
                    .transpose()?,
            ))
        }
        AssetDatabaseRecord::Cover {
            before,
            after: after_state,
        } => Ok(AssetDatabaseState::Cover(if after {
            *after_state
        } else {
            *before
        })),
    }
}

fn apply_asset_after(
    connection: &mut rusqlite::Connection,
    inner: &LibraryInner,
    book: BookId,
    asset: &AssetRecord,
) -> Result<()> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| {
            crate::library::database_error(
                "begin asset recovery transaction",
                &inner.database,
                error,
            )
        })?;
    match &asset.database {
        AssetDatabaseRecord::Format { after, .. } => {
            if let Some(after) = after {
                let after = after.to_complete(&inner.database)?;
                transaction
                    .execute(
                        "INSERT INTO data(book, format, uncompressed_size, name) \
                         VALUES (?1, ?2, ?3, ?4) \
                         ON CONFLICT(book, format) DO UPDATE SET \
                         uncompressed_size = excluded.uncompressed_size, name = excluded.name",
                        params![book.get(), after.format, after.size, after.stem],
                    )
                    .map_err(|error| {
                        crate::library::database_error(
                            "restore recovered format row",
                            &inner.database,
                            error,
                        )
                    })?;
            } else {
                let format = format_name_from_asset(asset, &inner.database)?;
                transaction
                    .execute(
                        "DELETE FROM data WHERE book = ?1 AND format = ?2 COLLATE NOCASE",
                        params![book.get(), format],
                    )
                    .map_err(|error| {
                        crate::library::database_error(
                            "remove recovered format row",
                            &inner.database,
                            error,
                        )
                    })?;
            }
            crate::formats::mark_format_changed(&transaction, book).map_err(|error| {
                crate::library::database_error(
                    "mark recovered format dirty",
                    &inner.database,
                    error,
                )
            })?;
        }
        AssetDatabaseRecord::Cover { after, .. } => {
            transaction
                .execute(
                    "UPDATE books SET has_cover = ?1 WHERE id = ?2",
                    params![after, book.get()],
                )
                .map_err(|error| {
                    crate::library::database_error(
                        "restore recovered cover flag",
                        &inner.database,
                        error,
                    )
                })?;
            crate::sql::mark_metadata_dirty(&transaction, book).map_err(|error| {
                crate::library::database_error("mark recovered cover dirty", &inner.database, error)
            })?;
        }
    }
    transaction.commit().map_err(|error| {
        crate::library::database_error("commit asset recovery", &inner.database, error)
    })
}

fn format_name_from_asset(asset: &AssetRecord, database: &Path) -> Result<String> {
    let AssetDatabaseRecord::Format { before, after } = &asset.database else {
        return Err(Error::InvalidLibrary {
            path: database.to_path_buf(),
            reason: "cover journal used as a format journal".into(),
        });
    };
    before
        .as_ref()
        .or(after.as_ref())
        .map(|state| state.format.clone())
        .ok_or_else(|| Error::InvalidLibrary {
            path: database.to_path_buf(),
            reason: "format journal has no format name".into(),
        })
}

fn reconcile_asset_files(
    inner: &LibraryInner,
    asset: &AssetRecord,
    roll_forward: bool,
) -> Result<()> {
    let destination = decode_root_path(&inner.root, &asset.destination, &inner.database)?;
    let backup = decode_root_path(&inner.root, &asset.backup, &inner.database)?;
    let staged = asset
        .staged
        .as_deref()
        .map(|path| decode_root_path(&inner.root, path, &inner.database))
        .transpose()?;
    let after_file = match asset.database {
        AssetDatabaseRecord::Format { ref after, .. } => after.is_some(),
        AssetDatabaseRecord::Cover { after, .. } => after,
    };
    if roll_forward {
        roll_asset_forward(
            &destination,
            &backup,
            staged.as_deref(),
            asset.before_file,
            after_file,
        )
    } else {
        roll_asset_back(&destination, &backup, staged.as_deref(), asset.before_file)
    }
}

fn roll_asset_forward(
    destination: &Path,
    backup: &Path,
    staged: Option<&Path>,
    before_file: bool,
    after_file: bool,
) -> Result<()> {
    if after_file {
        if let Some(staged) = staged {
            if path_exists(staged)? {
                ensure_file(staged, "recover staged asset")?;
                let destination_exists = path_exists(destination)?;
                let backup_exists = path_exists(backup)?;
                match (destination_exists, backup_exists, before_file) {
                    (true, false, true) => {
                        ensure_file(destination, "recover asset destination")?;
                        fs::rename(destination, backup).map_err(|source| {
                            crate::error::io_error(
                                "stage asset during recovery",
                                destination,
                                source,
                            )
                        })?;
                    }
                    (false, true, true) | (false, false, false) => {}
                    _ => {
                        return Err(ambiguous_paths_error(
                            "roll asset forward",
                            destination,
                            backup,
                        ));
                    }
                }
                fs::rename(staged, destination).map_err(|source| {
                    crate::error::io_error("install recovered asset", staged, source)
                })?;
            }
        }
        ensure_file(destination, "finish recovered asset")?;
    } else {
        if let Some(staged) = staged {
            remove_file_if_present(staged, "remove recovered staged asset")?;
        }
        remove_file_if_present(destination, "finish recovered asset removal")?;
    }
    remove_file_if_present(backup, "remove recovered asset backup")?;
    Ok(())
}

fn roll_asset_back(
    destination: &Path,
    backup: &Path,
    staged: Option<&Path>,
    before_file: bool,
) -> Result<()> {
    if let Some(staged) = staged {
        remove_file_if_present(staged, "remove rolled-back staged asset")?;
    }
    if path_exists(backup)? {
        ensure_file(backup, "restore asset backup")?;
        remove_file_if_present(destination, "remove failed asset replacement")?;
        fs::rename(backup, destination)
            .map_err(|source| crate::error::io_error("restore asset backup", backup, source))?;
    } else if before_file {
        ensure_file(destination, "verify rolled-back asset")?;
    } else {
        remove_file_if_present(destination, "remove rolled-back asset")?;
    }
    Ok(())
}

fn recover_book_move(
    inner: &LibraryInner,
    connection: &mut rusqlite::Connection,
    parsed: &ParsedJournal,
    record: &BookMoveRecord,
) -> Result<()> {
    let old_relative = decode_path(&record.old_directory, &parsed.journal_path)?;
    let new_relative = decode_path(&record.new_directory, &parsed.journal_path)?;
    let current_path: String = connection
        .query_row(
            "SELECT path FROM books WHERE id = ?1",
            [parsed.entry.book_id.get()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| {
            crate::library::database_error("inspect interrupted book move", &inner.database, error)
        })?
        .ok_or_else(|| Error::NotFound {
            entity: "book",
            id: parsed.entry.book_id.get(),
        })?;
    let current = PathBuf::from(current_path);
    let roll_forward = if current == new_relative {
        true
    } else if current == old_relative {
        false
    } else {
        return Err(ambiguous_database_error("recover book move", parsed));
    };
    reconcile_book_move_files(inner, record, &parsed.journal_path, roll_forward)?;
    if roll_forward {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                crate::library::database_error("begin book-move recovery", &inner.database, error)
            })?;
        transaction
            .execute(
                "UPDATE data SET name = ?1 WHERE book = ?2",
                params![record.new_stem, parsed.entry.book_id.get()],
            )
            .map_err(|error| {
                crate::library::database_error("restore moved format names", &inner.database, error)
            })?;
        crate::sql::mark_metadata_dirty(&transaction, parsed.entry.book_id).map_err(|error| {
            crate::library::database_error("mark recovered book move dirty", &inner.database, error)
        })?;
        transaction.commit().map_err(|error| {
            crate::library::database_error("commit book-move recovery", &inner.database, error)
        })?;
    }
    Ok(())
}

fn recover_trash(
    inner: &LibraryInner,
    connection: &rusqlite::Connection,
    parsed: &ParsedJournal,
    record: &TrashRecord,
) -> Result<()> {
    let database_present = match &record.kind {
        TrashAssetKind::Book => book_exists(connection, inner, parsed.entry.book_id)?,
        TrashAssetKind::Format { format } => connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM data WHERE book = ?1 AND format = ?2 COLLATE NOCASE)",
                params![parsed.entry.book_id.get(), format],
                |row| row.get(0),
            )
            .map_err(|error| {
                crate::library::database_error(
                    "inspect interrupted trash format",
                    &inner.database,
                    error,
                )
            })?,
    };
    let live = decode_root_path(&inner.root, &record.live, &parsed.journal_path)?;
    let trash = decode_root_path(&inner.root, &record.trash, &parsed.journal_path)?;
    let previous = record
        .previous
        .as_deref()
        .map(|path| decode_root_path(&inner.root, path, &parsed.journal_path))
        .transpose()?;
    reconcile_trash_paths(&live, &trash, previous.as_deref(), record, database_present)?;
    if !database_present
        && record.direction == TrashDirection::ToTrash
        && matches!(record.kind, TrashAssetKind::Format { .. })
    {
        let listing = record
            .listing
            .as_ref()
            .ok_or_else(|| Error::InvalidLibrary {
                path: parsed.journal_path.clone(),
                reason: "format trash recovery record has no listing metadata".into(),
            })?;
        let directory = trash.parent().ok_or_else(|| Error::InvalidLibrary {
            path: parsed.journal_path.clone(),
            reason: "format trash recovery path has no parent".into(),
        })?;
        crate::trash::write_format_metadata_values(directory, &listing.title, &listing.authors)?;
    }
    Ok(())
}

fn reconcile_trash_paths(
    live: &Path,
    trash: &Path,
    previous: Option<&Path>,
    record: &TrashRecord,
    database_present: bool,
) -> Result<()> {
    let live_exists = path_exists(live)?;
    let trash_exists = path_exists(trash)?;
    let previous_exists = previous.map(path_exists).transpose()?.unwrap_or(false);

    if database_present {
        if record.direction == TrashDirection::FromTrash && live_exists && trash_exists {
            return Err(ambiguous_paths_error(
                "recover trash restoration",
                live,
                trash,
            ));
        }
        if live_exists {
            ensure_trash_asset(live, &record.kind, "verify recovered live asset")?;
        } else {
            if !trash_exists {
                return Err(Error::UnsupportedOperation {
                    operation: "recover trash change",
                    reason: "database state requires a live asset, but no recoverable copy exists"
                        .into(),
                });
            }
            ensure_trash_asset(trash, &record.kind, "recover live trash asset")?;
            create_parent(live, "create recovered live parent")?;
            fs::rename(trash, live).map_err(|source| {
                crate::error::io_error("restore live asset from trash", trash, source)
            })?;
        }
        if previous_exists {
            let previous = previous.ok_or_else(|| Error::InvalidLibrary {
                path: trash.to_path_buf(),
                reason: "previous trash entry exists without a recorded path".into(),
            })?;
            if path_exists(trash)? {
                return Err(ambiguous_paths_error(
                    "restore previous trash entry",
                    trash,
                    previous,
                ));
            }
            ensure_trash_asset(previous, &record.kind, "verify previous trash entry")?;
            fs::rename(previous, trash).map_err(|source| {
                crate::error::io_error("restore previous trash entry", previous, source)
            })?;
        }
    } else {
        if live_exists && trash_exists {
            return Err(ambiguous_paths_error("finish trash change", live, trash));
        }
        if trash_exists {
            ensure_trash_asset(trash, &record.kind, "verify recovered trash asset")?;
        } else {
            if !live_exists {
                return Err(Error::UnsupportedOperation {
                    operation: "recover trash change",
                    reason: "database state requires a trash asset, but no recoverable copy exists"
                        .into(),
                });
            }
            ensure_trash_asset(live, &record.kind, "recover trash asset")?;
            create_parent(trash, "create recovered trash parent")?;
            fs::rename(live, trash).map_err(|source| {
                crate::error::io_error("finish moving asset to trash", live, source)
            })?;
        }
        if previous_exists {
            let previous = previous.ok_or_else(|| Error::InvalidLibrary {
                path: trash.to_path_buf(),
                reason: "previous trash entry exists without a recorded path".into(),
            })?;
            remove_trash_asset(previous, &record.kind, "remove superseded trash entry")?;
        }
    }
    Ok(())
}

fn ensure_trash_asset(path: &Path, kind: &TrashAssetKind, operation: &'static str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| crate::error::io_error(operation, path, source))?;
    let expected = match kind {
        TrashAssetKind::Book => metadata.is_dir() && !metadata.file_type().is_symlink(),
        TrashAssetKind::Format { .. } => metadata.is_file() && !metadata.file_type().is_symlink(),
    };
    if expected {
        Ok(())
    } else {
        Err(Error::InvalidLibrary {
            path: path.to_path_buf(),
            reason: "recovery trash asset has an unexpected file type".into(),
        })
    }
}

fn remove_trash_asset(path: &Path, kind: &TrashAssetKind, operation: &'static str) -> Result<()> {
    ensure_trash_asset(path, kind, operation)?;
    match kind {
        TrashAssetKind::Book => fs::remove_dir_all(path),
        TrashAssetKind::Format { .. } => fs::remove_file(path),
    }
    .map_err(|source| crate::error::io_error(operation, path, source))
}

fn reconcile_book_move_files(
    inner: &LibraryInner,
    record: &BookMoveRecord,
    journal: &Path,
    roll_forward: bool,
) -> Result<()> {
    let old_relative = decode_path(&record.old_directory, journal)?;
    let new_relative = decode_path(&record.new_directory, journal)?;
    let old_directory = crate::paths::resolve(&inner.root, &old_relative)?;
    let new_directory = crate::paths::resolve(&inner.root, &new_relative)?;
    let old_exists = path_exists(&old_directory)?;
    let new_exists = path_exists(&new_directory)?;
    if old_exists && new_exists {
        return Err(ambiguous_paths_error(
            "recover book move",
            &old_directory,
            &new_directory,
        ));
    }
    if !old_exists && !new_exists {
        return Err(Error::UnsupportedOperation {
            operation: "recover book move",
            reason: "neither the old nor new book directory exists".into(),
        });
    }
    if roll_forward {
        if old_exists {
            ensure_directory(&old_directory, "recover book move")?;
            create_parent(&new_directory, "create recovered book parent")?;
            fs::rename(&old_directory, &new_directory).map_err(|source| {
                crate::error::io_error("finish recovered book move", &old_directory, source)
            })?;
        }
        reconcile_filenames(&new_directory, &record.files, journal, true)?;
        remove_empty_parent(&old_directory, &inner.root);
    } else {
        if new_exists {
            ensure_directory(&new_directory, "recover book move")?;
            reconcile_filenames(&new_directory, &record.files, journal, false)?;
            create_parent(&old_directory, "create restored book parent")?;
            fs::rename(&new_directory, &old_directory).map_err(|source| {
                crate::error::io_error("restore interrupted book move", &new_directory, source)
            })?;
        } else {
            reconcile_filenames(&old_directory, &record.files, journal, false)?;
        }
        remove_empty_parent(&new_directory, &inner.root);
    }
    Ok(())
}

fn reconcile_filenames(
    directory: &Path,
    files: &[FileRenameRecord],
    journal: &Path,
    roll_forward: bool,
) -> Result<()> {
    for rename in files {
        let old_name = decode_path(&rename.old_name, journal)?;
        let new_name = decode_path(&rename.new_name, journal)?;
        validate_filename(&old_name)?;
        validate_filename(&new_name)?;
        let old = directory.join(old_name);
        let new = directory.join(new_name);
        let (source, destination) = if roll_forward {
            (&old, &new)
        } else {
            (&new, &old)
        };
        match (path_exists(source)?, path_exists(destination)?) {
            (true, false) => {
                ensure_file(source, "recover moved format")?;
                fs::rename(source, destination).map_err(|error| {
                    crate::error::io_error("recover moved format", source, error)
                })?;
            }
            (false, true) => ensure_file(destination, "verify recovered format")?,
            _ => {
                return Err(ambiguous_paths_error(
                    "recover moved format",
                    source,
                    destination,
                ));
            }
        }
    }
    Ok(())
}

fn begin_v1(
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
    let (directory, destination) = new_journal_path(root)?;
    let mut temporary = tempfile::NamedTempFile::new_in(&directory)
        .map_err(|source| crate::error::io_error("stage recovery journal", &directory, source))?;
    let staged = staged.map_or_else(|| "-".to_owned(), encode_path);
    write!(
        temporary,
        "{V1_MAGIC}\n{}\n{}\n{}\n{staged}\n",
        operation.v1_name(),
        book.get(),
        encode_path(relative)
    )
    .map_err(|source| crate::error::io_error("write recovery journal", &destination, source))?;
    install_journal(temporary, &destination, &directory)?;
    Ok(RecoveryJournal {
        path: destination,
        record: None,
    })
}

fn begin_v2(root: &Path, book: BookId, operation: OperationV2) -> Result<RecoveryJournal> {
    let record = JournalV2 {
        version: V2_VERSION,
        book_id: book.get(),
        operation,
    };
    let (directory, destination) = new_journal_path(root)?;
    let temporary = serialize_journal(&directory, &destination, &record)?;
    install_journal(temporary, &destination, &directory)?;
    Ok(RecoveryJournal {
        path: destination,
        record: Some(record),
    })
}

fn rewrite_v2(path: &Path, record: &JournalV2) -> Result<()> {
    let directory = path.parent().ok_or_else(|| Error::PathEscape {
        path: path.to_path_buf(),
        reason: "recovery journal has no parent".into(),
    })?;
    let temporary = serialize_journal(directory, path, record)?;
    temporary
        .persist(path)
        .map_err(|error| crate::error::io_error("replace recovery journal", path, error.error))?;
    sync_directory(directory)
}

fn serialize_journal(
    directory: &Path,
    destination: &Path,
    record: &JournalV2,
) -> Result<tempfile::NamedTempFile> {
    let mut temporary = tempfile::NamedTempFile::new_in(directory)
        .map_err(|source| crate::error::io_error("stage recovery journal", directory, source))?;
    serde_json::to_writer(&mut temporary, record).map_err(|error| Error::UnsupportedOperation {
        operation: "serialize recovery journal",
        reason: error.to_string(),
    })?;
    temporary
        .write_all(b"\n")
        .map_err(|source| crate::error::io_error("write recovery journal", destination, source))?;
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(|source| crate::error::io_error("sync recovery journal", destination, source))?;
    Ok(temporary)
}

fn install_journal(
    temporary: tempfile::NamedTempFile,
    destination: &Path,
    directory: &Path,
) -> Result<()> {
    temporary.persist(destination).map_err(|error| {
        crate::error::io_error("install recovery journal", destination, error.error)
    })?;
    sync_directory(directory)
}

fn new_journal_path(root: &Path) -> Result<(PathBuf, PathBuf)> {
    let directory = crate::paths::resolve(root, Path::new(RECOVERY_DIRECTORY))?;
    fs::create_dir_all(&directory).map_err(|source| {
        crate::error::io_error("create recovery journal directory", &directory, source)
    })?;
    let id = uuid::Uuid::new_v4().to_string();
    let destination = directory.join(format!("{id}.journal"));
    Ok((directory, destination))
}

fn scan(root: &Path) -> Result<Vec<ParsedJournal>> {
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

fn parse_journal(root: &Path, path: PathBuf) -> Result<ParsedJournal> {
    let metadata = fs::symlink_metadata(&path)
        .map_err(|source| crate::error::io_error("inspect recovery journal", &path, source))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error::InvalidLibrary {
            path,
            reason: "recovery journal is not a regular non-symlink file".into(),
        });
    }
    if metadata.len() > MAX_JOURNAL_SIZE {
        return Err(Error::InvalidLibrary {
            path,
            reason: "recovery journal exceeds 1 MiB".into(),
        });
    }
    let contents = fs::read(&path)
        .map_err(|source| crate::error::io_error("read recovery journal", &path, source))?;
    if contents.starts_with(V1_MAGIC.as_bytes()) {
        parse_v1(root, path, &contents)
    } else {
        parse_v2(root, path, &contents)
    }
}

fn parse_v1(root: &Path, path: PathBuf, contents: &[u8]) -> Result<ParsedJournal> {
    let contents = std::str::from_utf8(contents).map_err(|_| Error::InvalidLibrary {
        path: path.clone(),
        reason: "version-1 recovery journal is not UTF-8".into(),
    })?;
    let lines = contents.lines().collect::<Vec<_>>();
    if lines.len() != 5 || lines[0] != V1_MAGIC {
        return Err(Error::InvalidLibrary {
            path,
            reason: "unsupported recovery journal encoding".into(),
        });
    }
    let operation = RecoveryOperation::parse_v1(lines[1]).ok_or_else(|| Error::InvalidLibrary {
        path: path.clone(),
        reason: format!("unknown recovery operation {}", lines[1]),
    })?;
    let book_id = parse_book_id(lines[2], &path)?;
    let relative_path = decode_path(lines[3], &path)?;
    crate::paths::resolve(root, &relative_path)?;
    let detail = match operation {
        RecoveryOperation::BookAdd => JournalDetail::BookAdd,
        RecoveryOperation::PermanentBookRemoval => {
            if lines[4] == "-" {
                return Err(Error::InvalidLibrary {
                    path,
                    reason: "book-removal journal has no staged path".into(),
                });
            }
            let staged_path = decode_path(lines[4], &path)?;
            crate::paths::resolve(root, &staged_path)?;
            JournalDetail::BookRemoval { staged_path }
        }
        RecoveryOperation::FormatChange
        | RecoveryOperation::CoverChange
        | RecoveryOperation::BookMove
        | RecoveryOperation::TrashChange => {
            return Err(Error::InvalidLibrary {
                path,
                reason: "version-1 journal contains a version-2 operation".into(),
            });
        }
    };
    Ok(ParsedJournal {
        entry: RecoveryEntry {
            journal_id: journal_id(&path)?,
            operation,
            book_id,
            relative_path,
        },
        detail,
        journal_path: path,
    })
}

fn parse_v2(root: &Path, path: PathBuf, contents: &[u8]) -> Result<ParsedJournal> {
    let record =
        serde_json::from_slice::<JournalV2>(contents).map_err(|error| Error::InvalidLibrary {
            path: path.clone(),
            reason: format!("invalid version-2 recovery journal: {error}"),
        })?;
    if record.version != V2_VERSION {
        return Err(Error::InvalidLibrary {
            path,
            reason: format!("unsupported recovery journal version {}", record.version),
        });
    }
    let book_id = positive_book_id(record.book_id, &path)?;
    let (operation, relative_path) = match &record.operation {
        OperationV2::Asset(asset) => {
            validate_asset_record(root, &path, asset)?;
            let operation = match asset.database {
                AssetDatabaseRecord::Format { .. } => RecoveryOperation::FormatChange,
                AssetDatabaseRecord::Cover { .. } => RecoveryOperation::CoverChange,
            };
            (operation, decode_path(&asset.destination, &path)?)
        }
        OperationV2::BookMove(book_move) => {
            validate_book_move_record(root, &path, book_move)?;
            (
                RecoveryOperation::BookMove,
                decode_path(&book_move.old_directory, &path)?,
            )
        }
        OperationV2::Trash(trash) => {
            validate_trash_record(root, &path, book_id, trash)?;
            (
                RecoveryOperation::TrashChange,
                decode_path(&trash.live, &path)?,
            )
        }
    };
    Ok(ParsedJournal {
        entry: RecoveryEntry {
            journal_id: journal_id(&path)?,
            operation,
            book_id,
            relative_path,
        },
        detail: JournalDetail::V2(record),
        journal_path: path,
    })
}

fn validate_asset_record(root: &Path, journal: &Path, asset: &AssetRecord) -> Result<()> {
    let destination = decode_root_path(root, &asset.destination, journal)?;
    let backup = decode_root_path(root, &asset.backup, journal)?;
    validate_auxiliary_asset_path(&destination, &backup, journal)?;
    if let Some(staged) = &asset.staged {
        let staged = decode_root_path(root, staged, journal)?;
        validate_auxiliary_asset_path(&destination, &staged, journal)?;
        if staged == backup {
            return Err(Error::InvalidLibrary {
                path: journal.to_path_buf(),
                reason: "recovery staging and backup paths are identical".into(),
            });
        }
    }
    if asset.phase == AssetPhase::Ready {
        if let AssetDatabaseRecord::Format {
            after: Some(after), ..
        } = &asset.database
        {
            after.to_complete(journal)?;
        }
    }
    Ok(())
}

fn validate_auxiliary_asset_path(
    destination: &Path,
    auxiliary: &Path,
    journal: &Path,
) -> Result<()> {
    if auxiliary == destination || auxiliary.parent() != destination.parent() {
        Err(Error::InvalidLibrary {
            path: journal.to_path_buf(),
            reason: "recovery asset paths are not distinct siblings".into(),
        })
    } else {
        Ok(())
    }
}

fn validate_book_move_record(root: &Path, journal: &Path, record: &BookMoveRecord) -> Result<()> {
    decode_root_path(root, &record.old_directory, journal)?;
    decode_root_path(root, &record.new_directory, journal)?;
    for rename in &record.files {
        let old = decode_path(&rename.old_name, journal)?;
        let new = decode_path(&rename.new_name, journal)?;
        validate_filename(&old)?;
        validate_filename(&new)?;
    }
    Ok(())
}

fn validate_trash_record(
    root: &Path,
    journal: &Path,
    book: BookId,
    record: &TrashRecord,
) -> Result<()> {
    let live = decode_root_path(root, &record.live, journal)?;
    let trash = decode_root_path(root, &record.trash, journal)?;
    if live == trash {
        return Err(Error::InvalidLibrary {
            path: journal.to_path_buf(),
            reason: "trash recovery live and trash paths are identical".into(),
        });
    }
    let expected = match &record.kind {
        TrashAssetKind::Book => {
            crate::trash::trash_entry_path(root, crate::TrashEntryKind::Book, book)?
        }
        TrashAssetKind::Format { format } => {
            let normalized = crate::paths::format_name(std::ffi::OsStr::new(format))?;
            if normalized != *format {
                return Err(Error::InvalidLibrary {
                    path: journal.to_path_buf(),
                    reason: "trash recovery format is not normalized".into(),
                });
            }
            crate::trash::format_trash_path(root, book, format)?
        }
    };
    if trash != expected {
        return Err(Error::InvalidLibrary {
            path: journal.to_path_buf(),
            reason: "trash recovery destination does not match its book and asset".into(),
        });
    }
    if let Some(previous) = &record.previous {
        let previous = decode_root_path(root, previous, journal)?;
        if previous == live
            || previous == trash
            || previous.parent() != trash.parent()
            || !previous
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.starts_with(".calibre-rs-prior-"))
        {
            return Err(Error::InvalidLibrary {
                path: journal.to_path_buf(),
                reason: "trash recovery previous path is not a private trash sibling".into(),
            });
        }
    }
    if matches!(record.kind, TrashAssetKind::Format { .. })
        && record.direction == TrashDirection::ToTrash
        && record.listing.is_none()
    {
        return Err(Error::InvalidLibrary {
            path: journal.to_path_buf(),
            reason: "format trash recovery record has no listing metadata".into(),
        });
    }
    Ok(())
}

fn parse_book_id(value: &str, journal: &Path) -> Result<BookId> {
    let raw = value.parse::<i64>().map_err(|_| Error::InvalidLibrary {
        path: journal.to_path_buf(),
        reason: "invalid recovery book ID".into(),
    })?;
    positive_book_id(raw, journal)
}

fn positive_book_id(raw: i64, journal: &Path) -> Result<BookId> {
    if raw <= 0 {
        Err(Error::InvalidLibrary {
            path: journal.to_path_buf(),
            reason: "recovery book ID is not positive".into(),
        })
    } else {
        Ok(BookId::new(raw))
    }
}

fn journal_id(path: &Path) -> Result<String> {
    path.file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::InvalidLibrary {
            path: path.to_path_buf(),
            reason: "recovery journal has a non-UTF-8 filename".into(),
        })
}

fn encode_root_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).map_err(|_| Error::PathEscape {
        path: path.to_path_buf(),
        reason: "recovery path is outside the library root".into(),
    })?;
    crate::paths::resolve(root, relative)?;
    Ok(encode_path(relative))
}

fn decode_root_path(root: &Path, value: &str, journal: &Path) -> Result<PathBuf> {
    let relative = decode_path(value, journal)?;
    crate::paths::resolve(root, &relative)
}

fn validate_filename(path: &Path) -> Result<()> {
    crate::paths::validate_relative(path)?;
    if path.components().count() != 1 {
        return Err(Error::PathEscape {
            path: path.to_path_buf(),
            reason: "recovery filename must contain one path component".into(),
        });
    }
    Ok(())
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
        let high = hex_nibble(pair[0]).ok_or_else(|| invalid_path_encoding(journal))?;
        let low = hex_nibble(pair[1]).ok_or_else(|| invalid_path_encoding(journal))?;
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

fn invalid_path_encoding(journal: &Path) -> Error {
    Error::InvalidLibrary {
        path: journal.to_path_buf(),
        reason: "invalid recovery path encoding".into(),
    }
}

fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(crate::error::io_error(
            "inspect recovery path",
            path,
            source,
        )),
    }
}

fn ensure_file(path: &Path, operation: &'static str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| crate::error::io_error(operation, path, source))?;
    if metadata.is_file() {
        Ok(())
    } else {
        Err(Error::UnsupportedOperation {
            operation,
            reason: format!("expected a regular file: {}", path.display()),
        })
    }
}

fn ensure_directory(path: &Path, operation: &'static str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| crate::error::io_error(operation, path, source))?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(Error::UnsupportedOperation {
            operation,
            reason: format!("expected a directory: {}", path.display()),
        })
    }
}

fn remove_file_if_present(path: &Path, operation: &'static str) -> Result<()> {
    if path_exists(path)? {
        ensure_file(path, operation)?;
        fs::remove_file(path).map_err(|source| crate::error::io_error(operation, path, source))?;
    }
    Ok(())
}

fn create_parent(path: &Path, operation: &'static str) -> Result<()> {
    let parent = path.parent().ok_or_else(|| Error::PathEscape {
        path: path.to_path_buf(),
        reason: "recovery path has no parent".into(),
    })?;
    fs::create_dir_all(parent).map_err(|source| crate::error::io_error(operation, parent, source))
}

fn remove_empty_parent(path: &Path, root: &Path) {
    if let Some(parent) = path.parent() {
        if parent != root {
            let _ = fs::remove_dir(parent);
        }
    }
}

fn ambiguous_paths_error(operation: &'static str, first: &Path, second: &Path) -> Error {
    Error::UnsupportedOperation {
        operation,
        reason: format!(
            "filesystem state is ambiguous between {} and {}",
            first.display(),
            second.display()
        ),
    }
}

fn ambiguous_database_error(operation: &'static str, record: &ParsedJournal) -> Error {
    Error::UnsupportedOperation {
        operation,
        reason: format!(
            "database state does not match either side of recovery journal {}",
            record.entry.journal_id
        ),
    }
}

fn journal_state_error(operation: &'static str) -> Error {
    Error::UnsupportedOperation {
        operation,
        reason: "recovery journal is in an unexpected internal state".into(),
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
