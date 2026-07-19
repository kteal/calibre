use std::fmt;

macro_rules! id_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(i64);

        impl $name {
            /// Constructs an ID from its database representation.
            #[must_use]
            pub const fn new(value: i64) -> Self {
                Self(value)
            }

            /// Returns the database representation.
            #[must_use]
            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_type!(BookId, "A stable numeric book ID within one library.");
id_type!(AuthorId, "A stable numeric author ID within one library.");
id_type!(
    FormatId,
    "A stable numeric format-row ID within one library."
);
id_type!(
    CustomColumnId,
    "A stable numeric custom-column ID within one library."
);
