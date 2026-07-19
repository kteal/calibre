//! Native, synchronous access to existing Calibre libraries.
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
mod paths;
mod recovery;
mod sql;

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
