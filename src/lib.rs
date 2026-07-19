//! Native, synchronous creation of and access to Calibre libraries.
//!
//! This crate does not invoke Calibre or require it at runtime. Start by
//! opening a library and querying a page of books:
//!
//! ```no_run
//! use calibre::{BookQuery, Library};
//!
//! # fn main() -> Result<(), calibre::Error> {
//! let library = Library::open("/path/to/library")?;
//! let page = library.books().query(BookQuery::default())?;
//! for book in page.items {
//!     println!("{}: {}", book.id, book.title);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! A new core schema-27 library can be created without Calibre or Python:
//!
//! ```no_run
//! use calibre::{Library, NewBook};
//!
//! # fn main() -> Result<(), calibre::Error> {
//! let library = Library::create("/path/to/new-library")?;
//! library.books().add(NewBook {
//!     title: "A dated book".into(),
//!     publication_date: Some("2024-02-29".into()),
//!     ..NewBook::default()
//! })?;
//! # Ok(())
//! # }
//! ```

// Sibling implementation modules share crate-visible helpers. Keeping their
// visibility explicit makes accidental public re-exports easier to audit.
#![allow(clippy::redundant_pub_crate)]

mod audit;
mod books;
mod covers;
mod custom_columns;
mod error;
mod formats;
mod ids;
mod library;
mod model;
mod opf;
mod paths;
mod recovery;
mod schema;
mod sql;
mod trash;

pub use audit::{AuditIssue, AuditIssueKind, AuditReport, Auditor};
pub use books::Books;
pub use covers::Covers;
pub use custom_columns::{CustomColumn, CustomColumnKind, CustomColumnValue, CustomColumns};
pub use error::{Error, Result};
pub use formats::Formats;
pub use ids::{AuthorId, BookId, CustomColumnId, FormatId};
pub use library::{Capabilities, Compatibility, Library, OpenMode, OpenOptions};
pub use model::{
    Author, Book, BookFilter, BookOrder, BookPage, BookQuery, BookSort, DeletionMode, Format,
    FormatFile, Identifier, Language, NewBook, PageRequest, Publisher, Rating, Series,
    SortDirection, Tag, UpdateBook,
};
pub use recovery::{RecoveryEntry, RecoveryOperation, RecoveryReport};
pub use trash::{DEFAULT_TRASH_EXPIRY, Trash, TrashContents, TrashEntry, TrashEntryKind};
