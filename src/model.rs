use crate::{AuthorId, BookId, FormatId};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// An author attached to a book.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Author {
    /// Database ID.
    pub id: AuthorId,
    /// Display name.
    pub name: String,
    /// Calibre's stored sort value.
    pub sort: Option<String>,
    /// Optional link attached to the author.
    pub link: String,
}

/// A tag attached to a book.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Tag {
    /// Database ID.
    pub id: i64,
    /// Tag name.
    pub name: String,
    /// Optional link.
    pub link: String,
}

/// A series attached to a book.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Series {
    /// Database ID.
    pub id: i64,
    /// Series name.
    pub name: String,
    /// Stored sort value.
    pub sort: Option<String>,
    /// Optional link.
    pub link: String,
    /// This book's index in the series.
    pub index: f64,
}

/// A publisher attached to a book.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Publisher {
    /// Database ID.
    pub id: i64,
    /// Publisher name.
    pub name: String,
    /// Stored sort value.
    pub sort: Option<String>,
    /// Optional link.
    pub link: String,
}

/// A language code attached to a book.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Language {
    /// Database ID.
    pub id: i64,
    /// Calibre's stored ISO 639 code, normally three letters.
    pub code: String,
    /// Optional link.
    pub link: String,
}

/// A typed external identifier such as an ISBN.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Identifier {
    /// Identifier kind.
    pub kind: String,
    /// Identifier value.
    pub value: String,
}

/// A Calibre rating on its stored zero-to-ten scale.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rating(u8);

impl Rating {
    /// Creates a nonzero rating.
    ///
    /// Calibre stores ratings from 0 through 10, but treats zero as absent.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::InvalidInput`] for zero or a value above 10.
    pub fn new(value: u8) -> crate::Result<Self> {
        if (1..=10).contains(&value) {
            Ok(Self(value))
        } else {
            Err(crate::Error::InvalidInput {
                field: "rating",
                reason: "expected an integer from 1 through 10".into(),
            })
        }
    }

    /// Returns the stored value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// One ebook format row and its resolved filesystem state.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Format {
    /// Format row ID.
    pub id: FormatId,
    /// Uppercase logical format, such as `EPUB`.
    pub format: String,
    /// Size recorded in `metadata.db`.
    pub stored_size: u64,
    /// Size observed on disk, or `None` if the file is missing.
    pub file_size: Option<u64>,
    /// Checked absolute path within the library.
    pub path: PathBuf,
}

/// Complete core metadata for one book.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Book {
    /// Book ID.
    pub id: BookId,
    /// Display title.
    pub title: String,
    /// Stored title sort value.
    pub sort: Option<String>,
    /// Stored creation/import timestamp.
    pub timestamp: Option<String>,
    /// Stored publication date.
    pub publication_date: Option<String>,
    /// Stored author sort value.
    pub author_sort: Option<String>,
    /// Relative book-directory path from `metadata.db`.
    pub relative_path: PathBuf,
    /// Book UUID.
    pub uuid: Option<String>,
    /// Stored last-modified timestamp.
    pub last_modified: String,
    /// Authors in Calibre link order.
    pub authors: Vec<Author>,
    /// Tags in Calibre link order.
    pub tags: Vec<Tag>,
    /// Optional series and series index.
    pub series: Option<Series>,
    /// Optional publisher.
    pub publisher: Option<Publisher>,
    /// Languages in Calibre link order.
    pub languages: Vec<Language>,
    /// Identifiers keyed by their stored type.
    pub identifiers: Vec<Identifier>,
    /// Optional HTML comments.
    pub comments: Option<String>,
    /// Optional rating.
    pub rating: Option<Rating>,
    /// Available format rows, including missing files.
    pub formats: Vec<Format>,
    /// Checked cover path when the database says a cover exists.
    pub cover_path: Option<PathBuf>,
}

/// A stable sort key for book queries.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum BookSort {
    /// Sort by Calibre's stored title-sort value.
    #[default]
    Title,
    /// Sort by stored author sort.
    Author,
    /// Sort by import timestamp.
    Timestamp,
    /// Sort by last modification.
    LastModified,
    /// Sort by numeric ID.
    Id,
    /// Sort by publication date.
    PublicationDate,
    /// Sort by series name.
    Series,
    /// Sort by publisher name.
    Publisher,
    /// Sort by Calibre's stored rating.
    Rating,
}

/// Sort direction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SortDirection {
    /// Ascending.
    #[default]
    Ascending,
    /// Descending.
    Descending,
}

/// Offset-based pagination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageRequest {
    /// Maximum rows to return.
    pub limit: u32,
    /// Rows to skip.
    pub offset: u64,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            limit: 100,
            offset: 0,
        }
    }
}

/// One filter applied to a book query.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BookFilter {
    /// Match a case-insensitive title fragment.
    TitleContains(String),
    /// Match a case-insensitive author-name fragment.
    AuthorContains(String),
    /// Match an exact tag name.
    Tag(String),
    /// Match an exact series name.
    Series(String),
    /// Match an exact publisher name.
    Publisher(String),
    /// Match an exact stored language code.
    Language(String),
    /// Match an exact identifier value and, when supplied, its kind.
    Identifier {
        /// Optional identifier kind such as `isbn`.
        kind: Option<String>,
        /// Identifier value.
        value: String,
    },
    /// Match an available logical format such as `EPUB`.
    Format(String),
    /// Match ratings within an inclusive range.
    RatingRange {
        /// Inclusive lower bound.
        minimum: Rating,
        /// Inclusive upper bound.
        maximum: Rating,
    },
    /// Match the stored cover-presence flag.
    HasCover(bool),
    /// Restrict results to these book IDs.
    Ids(Vec<BookId>),
}

/// One ordered sort term.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BookOrder {
    /// Sort field.
    pub field: BookSort,
    /// Sort direction.
    pub direction: SortDirection,
}

impl BookOrder {
    /// Creates a sort term.
    #[must_use]
    pub const fn new(field: BookSort, direction: SortDirection) -> Self {
        Self { field, direction }
    }

    /// Creates an ascending sort term.
    #[must_use]
    pub const fn ascending(field: BookSort) -> Self {
        Self::new(field, SortDirection::Ascending)
    }

    /// Creates a descending sort term.
    #[must_use]
    pub const fn descending(field: BookSort) -> Self {
        Self::new(field, SortDirection::Descending)
    }
}

/// Filtering, multi-column sorting, and pagination for book listing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookQuery {
    /// Filters combined with logical AND.
    pub filters: Vec<BookFilter>,
    /// Sort terms from most significant to least significant.
    pub order: Vec<BookOrder>,
    /// Page request.
    pub page: PageRequest,
}

impl BookQuery {
    /// Adds a filter.
    #[must_use]
    pub fn filter(mut self, filter: BookFilter) -> Self {
        self.filters.push(filter);
        self
    }

    /// Replaces the sort terms.
    #[must_use]
    pub fn order_by(mut self, order: impl IntoIterator<Item = BookOrder>) -> Self {
        self.order = order.into_iter().collect();
        self
    }

    /// Replaces the page request.
    #[must_use]
    pub const fn page(mut self, page: PageRequest) -> Self {
        self.page = page;
        self
    }
}

impl Default for BookQuery {
    fn default() -> Self {
        Self {
            filters: Vec::new(),
            order: vec![BookOrder::ascending(BookSort::Title)],
            page: PageRequest::default(),
        }
    }
}

/// One page of books.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct BookPage {
    /// Returned books.
    pub items: Vec<Book>,
    /// Total rows matching the filter.
    pub total: u64,
    /// Offset used.
    pub offset: u64,
    /// Requested limit.
    pub limit: u32,
}

/// A source file to add as a book format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormatFile {
    path: PathBuf,
}

impl FormatFile {
    /// Creates a format source from a path. The extension is the format.
    #[must_use]
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// Input for creating a book.
#[derive(Clone, Debug)]
pub struct NewBook {
    /// Title. Empty values become `Unknown`.
    pub title: String,
    /// Author display names. Empty lists become `Unknown`.
    pub authors: Vec<String>,
    /// Tags in desired link order.
    pub tags: Vec<String>,
    /// Optional series name.
    pub series: Option<String>,
    /// Series index.
    pub series_index: f64,
    /// Optional publisher.
    pub publisher: Option<String>,
    /// ISO 639 language codes.
    pub languages: Vec<String>,
    /// External identifiers keyed by type.
    pub identifiers: BTreeMap<String, String>,
    /// Optional HTML comments.
    pub comments: Option<String>,
    /// Optional rating on Calibre's zero-to-ten storage scale.
    pub rating: Option<Rating>,
    /// Optional publication date.
    ///
    /// Accepts `YYYY-MM-DD` or a Calibre UTC timestamp of the form
    /// `YYYY-MM-DD HH:MM:SS[.fraction]+00:00`. Date-only values are stored at
    /// midnight UTC.
    pub publication_date: Option<String>,
    /// Source format files copied into the library.
    pub formats: Vec<FormatFile>,
    /// Optional JPEG cover source.
    pub cover: Option<PathBuf>,
}

impl Default for NewBook {
    fn default() -> Self {
        Self {
            title: String::new(),
            authors: Vec::new(),
            tags: Vec::new(),
            series: None,
            series_index: 1.0,
            publisher: None,
            languages: Vec::new(),
            identifiers: BTreeMap::new(),
            comments: None,
            rating: None,
            publication_date: None,
            formats: Vec::new(),
            cover: None,
        }
    }
}

/// Partial core-metadata replacement for a book.
#[derive(Clone, Debug, Default)]
pub struct UpdateBook {
    /// Replacement title.
    pub title: Option<String>,
    /// Replacement authors in link order.
    pub authors: Option<Vec<String>>,
    /// Replacement tags in link order.
    pub tags: Option<Vec<String>>,
    /// `Some(None)` clears the series.
    pub series: Option<Option<String>>,
    /// Replacement series index.
    pub series_index: Option<f64>,
    /// `Some(None)` clears the publisher.
    pub publisher: Option<Option<String>>,
    /// Replacement languages.
    pub languages: Option<Vec<String>>,
    /// Replacement identifiers.
    pub identifiers: Option<BTreeMap<String, String>>,
    /// `Some(None)` clears comments.
    pub comments: Option<Option<String>>,
    /// `Some(None)` clears the rating.
    pub rating: Option<Option<Rating>>,
    /// Replacement publication date.
    ///
    /// `None` leaves the value unchanged, `Some(None)` clears it, and
    /// `Some(Some(value))` sets it. Accepted values are `YYYY-MM-DD` and
    /// Calibre UTC timestamps of the form
    /// `YYYY-MM-DD HH:MM:SS[.fraction]+00:00`.
    pub publication_date: Option<Option<String>>,
}

/// How a book should be removed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DeletionMode {
    /// Delete database state and files after compensating staging.
    Permanent,
    /// Use Calibre's per-library trash.
    Trash,
}
