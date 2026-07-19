#![allow(missing_docs)]

mod support;

use calibre::{Library, NewBook, OpenOptions, UpdateBook};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn metadata_round_trips_through_add_and_update(
        title in "[A-Za-z0-9λ][A-Za-z0-9 λ]{0,29}",
        updated in "[A-Za-z0-9Ω][A-Za-z0-9 Ω]{0,29}",
        tag in "[a-z]{1,12}"
    ) {
        let fixture = support::TestLibrary::new();
        let library = Library::open_with(
            fixture.path(),
            OpenOptions::new().read_write(true),
        ).expect("open");
        let book = library.books().add(NewBook {
            title: title.clone(),
            authors: vec!["Property Author".into()],
            tags: vec![tag.clone()],
            ..NewBook::default()
        }).expect("add");
        prop_assert_eq!(&book.title, title.trim());
        prop_assert_eq!(&book.tags[0].name, &tag);
        let book = library.books().update(book.id, UpdateBook {
            title: Some(updated.clone()),
            ..UpdateBook::default()
        }).expect("update");
        prop_assert_eq!(&book.title, updated.trim());
        prop_assert!(fixture.path().join(book.relative_path).is_dir());
    }
}
