use crate::library::{LibraryInner, database_error};
use crate::{BookId, CustomColumnId, Error, Result};
use rusqlite::types::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The declared data type of a Calibre custom column.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CustomColumnKind {
    /// A text value or list of text values.
    Text,
    /// Long-form text.
    Comments,
    /// A series name and per-book index.
    Series,
    /// Calibre's integer rating value.
    Rating,
    /// An ISO-like timestamp stored by Calibre.
    DateTime,
    /// A three-state boolean value.
    Boolean,
    /// An integer value.
    Integer,
    /// A floating-point value.
    Float,
    /// A value selected from a configured enumeration.
    Enumeration,
    /// A value evaluated from a Calibre template.
    Composite,
    /// A data type this crate does not yet recognize.
    Unknown(String),
}

impl CustomColumnKind {
    fn from_database(value: String) -> Self {
        match value.as_str() {
            "text" => Self::Text,
            "comments" => Self::Comments,
            "series" => Self::Series,
            "rating" => Self::Rating,
            "datetime" => Self::DateTime,
            "bool" => Self::Boolean,
            "int" => Self::Integer,
            "float" => Self::Float,
            "enumeration" => Self::Enumeration,
            "composite" => Self::Composite,
            _ => Self::Unknown(value),
        }
    }
}

/// One active custom-column definition.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct CustomColumn {
    /// Numeric definition ID used to derive Calibre's dynamic table names.
    pub id: CustomColumnId,
    /// Programmatic label without the leading `#`.
    pub label: String,
    /// Display name.
    pub name: String,
    /// Declared data type.
    pub kind: CustomColumnKind,
    /// Whether the definition permits multiple values.
    pub is_multiple: bool,
    /// Whether Calibre considers the column editable.
    pub editable: bool,
    /// Raw JSON display configuration stored by Calibre.
    pub display: String,
    /// Whether values live in a normalized value and link table pair.
    pub normalized: bool,
}

/// A custom-column value read from a book.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum CustomColumnValue {
    /// One text value.
    Text(String),
    /// Multiple ordered text values.
    TextList(Vec<String>),
    /// One integer value.
    Integer(i64),
    /// Multiple ordered integer values.
    IntegerList(Vec<i64>),
    /// One floating-point value.
    Real(f64),
    /// Multiple ordered floating-point values.
    RealList(Vec<f64>),
    /// A boolean value. `None` represents Calibre's indeterminate state.
    Boolean(Option<bool>),
    /// A timestamp in its database representation.
    DateTime(String),
    /// A series name and optional per-book index.
    Series {
        /// Series name.
        name: String,
        /// Series index from the link row.
        index: Option<f64>,
    },
    /// The definition requires behavior this crate cannot safely reproduce.
    Unavailable,
}

/// Read-only access to custom-column definitions and values.
#[derive(Clone, Debug)]
pub struct CustomColumns {
    inner: Arc<LibraryInner>,
}

impl CustomColumns {
    pub(crate) const fn new(inner: Arc<LibraryInner>) -> Self {
        Self { inner }
    }

    /// Lists active custom-column definitions in definition-ID order.
    ///
    /// # Errors
    ///
    /// Returns an error when the definition table cannot be read or contains
    /// an invalid numeric ID.
    pub fn definitions(&self) -> Result<Vec<CustomColumn>> {
        let connection = self.inner.read_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, label, name, datatype, is_multiple, editable, display, normalized \
                 FROM custom_columns WHERE mark_for_delete = 0 ORDER BY id",
            )
            .map_err(|error| {
                database_error(
                    "prepare custom-column definitions",
                    &self.inner.database,
                    error,
                )
            })?;
        let definitions = statement
            .query_map([], |row| {
                Ok(CustomColumn {
                    id: CustomColumnId::new(row.get(0)?),
                    label: row.get(1)?,
                    name: row.get(2)?,
                    kind: CustomColumnKind::from_database(row.get(3)?),
                    is_multiple: row.get(4)?,
                    editable: row.get(5)?,
                    display: row.get(6)?,
                    normalized: row.get(7)?,
                })
            })
            .map_err(|error| {
                database_error(
                    "query custom-column definitions",
                    &self.inner.database,
                    error,
                )
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| {
                database_error(
                    "read custom-column definitions",
                    &self.inner.database,
                    error,
                )
            })?;
        if let Some(definition) = definitions
            .iter()
            .find(|definition| definition.id.get() <= 0)
        {
            return Err(Error::InvalidLibrary {
                path: self.inner.database.clone(),
                reason: format!(
                    "custom column #{} has non-positive ID {}",
                    definition.label,
                    definition.id.get()
                ),
            });
        }
        Ok(definitions)
    }

    /// Reads one custom-column value by its label.
    ///
    /// A leading `#` is accepted. A defined column with no value returns
    /// `None`; an unknown label is invalid input.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown label, missing book, unexpected dynamic
    /// table shape, or database failure.
    pub fn value(&self, book: BookId, label: &str) -> Result<Option<CustomColumnValue>> {
        let label = label.strip_prefix('#').unwrap_or(label);
        let definition = self
            .definitions()?
            .into_iter()
            .find(|definition| definition.label == label)
            .ok_or_else(|| Error::InvalidInput {
                field: "custom column label",
                reason: format!("unknown active custom column #{label}"),
            })?;
        let connection = self.inner.read_connection()?;
        ensure_book(&connection, &self.inner, book)?;
        read_value(&connection, &self.inner, book, &definition)
    }

    /// Reads every active custom-column value for a book.
    ///
    /// Columns with no stored value are omitted. Composite and unknown column
    /// kinds are included as [`CustomColumnValue::Unavailable`].
    ///
    /// # Errors
    ///
    /// Returns an error for a missing book, unexpected dynamic table shape, or
    /// database failure.
    pub fn values(&self, book: BookId) -> Result<BTreeMap<String, CustomColumnValue>> {
        let definitions = self.definitions()?;
        let connection = self.inner.read_connection()?;
        ensure_book(&connection, &self.inner, book)?;
        let mut result = BTreeMap::new();
        for definition in definitions {
            if let Some(value) = read_value(&connection, &self.inner, book, &definition)? {
                result.insert(definition.label, value);
            }
        }
        Ok(result)
    }
}

fn ensure_book(
    connection: &rusqlite::Connection,
    inner: &LibraryInner,
    book: BookId,
) -> Result<()> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM books WHERE id = ?1)",
            [book.get()],
            |row| row.get(0),
        )
        .map_err(|error| database_error("find custom-column book", &inner.database, error))?;
    if exists {
        Ok(())
    } else {
        Err(Error::NotFound {
            entity: "book",
            id: book.get(),
        })
    }
}

fn read_value(
    connection: &rusqlite::Connection,
    inner: &LibraryInner,
    book: BookId,
    definition: &CustomColumn,
) -> Result<Option<CustomColumnValue>> {
    let id = definition.id.get();
    if id <= 0 {
        return Err(Error::InvalidLibrary {
            path: inner.database.clone(),
            reason: format!(
                "custom column #{0} has non-positive ID {id}",
                definition.label
            ),
        });
    }
    if matches!(
        definition.kind,
        CustomColumnKind::Composite | CustomColumnKind::Unknown(_)
    ) {
        return Ok(Some(CustomColumnValue::Unavailable));
    }
    let value_table = format!("custom_column_{id}");
    if definition.normalized {
        let link_table = format!("books_custom_column_{id}_link");
        validate_columns(connection, inner, &value_table, &["id", "value"])?;
        let mut required = vec!["id", "book", "value"];
        if definition.kind == CustomColumnKind::Series {
            required.push("extra");
        }
        validate_columns(connection, inner, &link_table, &required)?;
        let extra = if definition.kind == CustomColumnKind::Series {
            "l.extra"
        } else {
            "NULL"
        };
        let sql = format!(
            "SELECT v.value, {extra} FROM {link_table} AS l \
             JOIN {value_table} AS v ON v.id = l.value \
             WHERE l.book = ?1 ORDER BY l.id"
        );
        let mut statement = connection.prepare(&sql).map_err(|error| {
            database_error("prepare normalized custom value", &inner.database, error)
        })?;
        let rows = statement
            .query_map([book.get()], |row| {
                Ok((row.get::<_, Value>(0)?, row.get::<_, Option<f64>>(1)?))
            })
            .map_err(|error| {
                database_error("query normalized custom value", &inner.database, error)
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| {
                database_error("read normalized custom value", &inner.database, error)
            })?;
        convert_rows(definition, rows, &inner.database)
    } else {
        validate_columns(connection, inner, &value_table, &["id", "book", "value"])?;
        let sql = format!("SELECT value, NULL FROM {value_table} WHERE book = ?1 ORDER BY id");
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| database_error("prepare custom value", &inner.database, error))?;
        let rows = statement
            .query_map([book.get()], |row| {
                Ok((row.get::<_, Value>(0)?, row.get::<_, Option<f64>>(1)?))
            })
            .map_err(|error| database_error("query custom value", &inner.database, error))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| database_error("read custom value", &inner.database, error))?;
        convert_rows(definition, rows, &inner.database)
    }
}

fn validate_columns(
    connection: &rusqlite::Connection,
    inner: &LibraryInner,
    table: &str,
    required: &[&str],
) -> Result<()> {
    let sql = format!("PRAGMA table_info({table})");
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| database_error("inspect custom-column table", &inner.database, error))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| database_error("inspect custom-column table", &inner.database, error))?
        .collect::<std::result::Result<BTreeSet<_>, _>>()
        .map_err(|error| database_error("inspect custom-column table", &inner.database, error))?;
    for column in required {
        if !columns.contains(*column) {
            return Err(Error::InvalidLibrary {
                path: inner.database.clone(),
                reason: format!("custom-column table {table} is missing {column}"),
            });
        }
    }
    Ok(())
}

fn convert_rows(
    definition: &CustomColumn,
    rows: Vec<(Value, Option<f64>)>,
    database: &std::path::Path,
) -> Result<Option<CustomColumnValue>> {
    if rows.is_empty() {
        return Ok(None);
    }
    match definition.kind {
        CustomColumnKind::Text | CustomColumnKind::Comments | CustomColumnKind::Enumeration => {
            let values = rows
                .into_iter()
                .map(|(value, _)| value_as_text(value, &definition.label, database))
                .collect::<Result<Vec<_>>>()?;
            if definition.is_multiple {
                Ok(Some(CustomColumnValue::TextList(values)))
            } else {
                Ok(values.into_iter().next().map(CustomColumnValue::Text))
            }
        }
        CustomColumnKind::Series => {
            let Some((value, index)) = rows.into_iter().next() else {
                return Ok(None);
            };
            Ok(Some(CustomColumnValue::Series {
                name: value_as_text(value, &definition.label, database)?,
                index,
            }))
        }
        CustomColumnKind::Rating | CustomColumnKind::Integer => {
            let values = rows
                .into_iter()
                .map(|(value, _)| value_as_integer(&value, &definition.label, database))
                .collect::<Result<Vec<_>>>()?;
            if definition.is_multiple {
                Ok(Some(CustomColumnValue::IntegerList(values)))
            } else {
                Ok(values.into_iter().next().map(CustomColumnValue::Integer))
            }
        }
        CustomColumnKind::Float => {
            let values = rows
                .into_iter()
                .map(|(value, _)| value_as_real(&value, &definition.label, database))
                .collect::<Result<Vec<_>>>()?;
            if definition.is_multiple {
                Ok(Some(CustomColumnValue::RealList(values)))
            } else {
                Ok(values.into_iter().next().map(CustomColumnValue::Real))
            }
        }
        CustomColumnKind::Boolean => {
            let Some((value, _)) = rows.into_iter().next() else {
                return Ok(None);
            };
            let value = match value {
                Value::Null => None,
                Value::Integer(value) => Some(value != 0),
                other => {
                    return Err(value_type_error(
                        &definition.label,
                        "integer",
                        &other,
                        database,
                    ));
                }
            };
            Ok(Some(CustomColumnValue::Boolean(value)))
        }
        CustomColumnKind::DateTime => {
            let Some((value, _)) = rows.into_iter().next() else {
                return Ok(None);
            };
            Ok(Some(CustomColumnValue::DateTime(value_as_text(
                value,
                &definition.label,
                database,
            )?)))
        }
        CustomColumnKind::Composite | CustomColumnKind::Unknown(_) => {
            Ok(Some(CustomColumnValue::Unavailable))
        }
    }
}

fn value_as_text(value: Value, label: &str, database: &std::path::Path) -> Result<String> {
    if let Value::Text(value) = value {
        Ok(value)
    } else {
        Err(value_type_error(label, "text", &value, database))
    }
}

fn value_as_integer(value: &Value, label: &str, database: &std::path::Path) -> Result<i64> {
    if let Value::Integer(value) = value {
        Ok(*value)
    } else {
        Err(value_type_error(label, "integer", value, database))
    }
}

#[allow(clippy::cast_precision_loss)] // SQLite numeric affinity permits integer-backed REAL values.
fn value_as_real(value: &Value, label: &str, database: &std::path::Path) -> Result<f64> {
    match value {
        Value::Real(value) => Ok(*value),
        Value::Integer(value) => Ok(*value as f64),
        other => Err(value_type_error(label, "number", other, database)),
    }
}

fn value_type_error(
    label: &str,
    expected: &str,
    value: &Value,
    database: &std::path::Path,
) -> Error {
    Error::InvalidLibrary {
        path: database.to_path_buf(),
        reason: format!(
            "custom column #{label} expected {expected}, found {}",
            value.data_type()
        ),
    }
}
