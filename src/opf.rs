use crate::{Book, BookId, Error, Rating, Result};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

const DC_NAMESPACE: &str = "http://purl.org/dc/elements/1.1/";
const OPF_NAMESPACE: &str = "http://www.idpf.org/2007/opf";
const MAX_OPF_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct TrashBookMetadata {
    pub(crate) id: BookId,
    pub(crate) title: String,
    pub(crate) sort: Option<String>,
    pub(crate) timestamp: Option<String>,
    pub(crate) publication_date: Option<String>,
    pub(crate) author_sort: Option<String>,
    pub(crate) uuid: Option<String>,
    pub(crate) authors: Vec<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) series: Option<String>,
    pub(crate) series_index: f64,
    pub(crate) publisher: Option<String>,
    pub(crate) languages: Vec<String>,
    pub(crate) identifiers: BTreeMap<String, String>,
    pub(crate) comments: Option<String>,
    pub(crate) rating: Option<Rating>,
}

pub(crate) fn write(book: &Book, destination: &Path) -> Result<()> {
    let xml = serialize(book);
    let parent = destination.parent().ok_or_else(|| Error::PathEscape {
        path: destination.to_path_buf(),
        reason: "metadata.opf has no parent directory".into(),
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|source| crate::error::io_error("create temporary OPF", parent, source))?;
    std::io::Write::write_all(&mut temporary, xml.as_bytes())
        .map_err(|source| crate::error::io_error("write temporary OPF", destination, source))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| crate::error::io_error("sync temporary OPF", destination, source))?;
    temporary.persist(destination).map_err(|error| {
        crate::error::io_error("install metadata.opf", destination, error.error)
    })?;
    Ok(())
}

pub(crate) fn read(path: &Path, expected_id: BookId) -> Result<TrashBookMetadata> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|source| crate::error::io_error("inspect metadata.opf", path, source))?;
    if !metadata.is_file() {
        return Err(Error::InvalidLibrary {
            path: path.to_path_buf(),
            reason: "trash metadata.opf is not a regular file".into(),
        });
    }
    if metadata.len() > MAX_OPF_BYTES {
        return Err(Error::InvalidLibrary {
            path: path.to_path_buf(),
            reason: "trash metadata.opf exceeds the 4 MiB safety limit".into(),
        });
    }
    let xml = std::fs::read_to_string(path)
        .map_err(|source| crate::error::io_error("read metadata.opf", path, source))?;
    parse(&xml, path, expected_id)
}

#[allow(clippy::too_many_lines)] // The output order is intentionally explicit and auditable.
fn serialize(book: &Book) -> String {
    let mut output = String::with_capacity(2048);
    output.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    output.push_str(
        "<package xmlns=\"http://www.idpf.org/2007/opf\" unique-identifier=\"uuid_id\" version=\"2.0\">\n",
    );
    output.push_str(
        "  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\n",
    );
    element(&mut output, "dc:title", &book.title, &[]);
    for author in &book.authors {
        let sort = author.sort.as_deref().unwrap_or(&author.name);
        element(
            &mut output,
            "dc:creator",
            &author.name,
            &[("opf:file-as", sort), ("opf:role", "aut")],
        );
    }
    element(
        &mut output,
        "dc:contributor",
        "calibre-rs",
        &[("opf:file-as", "calibre-rs"), ("opf:role", "bkp")],
    );
    element(
        &mut output,
        "dc:identifier",
        &book.id.get().to_string(),
        &[("opf:scheme", "calibre")],
    );
    let generated_uuid;
    let uuid = if let Some(uuid) = &book.uuid {
        uuid.as_str()
    } else {
        generated_uuid = uuid::Uuid::new_v4().to_string();
        &generated_uuid
    };
    element(
        &mut output,
        "dc:identifier",
        uuid,
        &[("id", "uuid_id"), ("opf:scheme", "uuid")],
    );
    for identifier in &book.identifiers {
        element(
            &mut output,
            "dc:identifier",
            &identifier.value,
            &[("opf:scheme", &identifier.kind.to_ascii_uppercase())],
        );
    }
    if let Some(date) = &book.publication_date {
        element(&mut output, "dc:date", date, &[]);
    }
    if let Some(comments) = &book.comments {
        element(&mut output, "dc:description", comments, &[]);
    }
    if let Some(publisher) = &book.publisher {
        element(&mut output, "dc:publisher", &publisher.name, &[]);
    }
    for language in &book.languages {
        element(&mut output, "dc:language", &language.code, &[]);
    }
    for tag in &book.tags {
        element(&mut output, "dc:subject", &tag.name, &[]);
    }
    if let Some(series) = &book.series {
        meta(&mut output, "calibre:series", &series.name);
        meta(
            &mut output,
            "calibre:series_index",
            &series.index.to_string(),
        );
    }
    if let Some(rating) = book.rating {
        meta(&mut output, "calibre:rating", &rating.get().to_string());
    }
    if let Some(timestamp) = &book.timestamp {
        meta(&mut output, "calibre:timestamp", timestamp);
    }
    if let Some(sort) = &book.sort {
        meta(&mut output, "calibre:title_sort", sort);
    }
    if let Some(author_sort) = &book.author_sort {
        meta(&mut output, "calibre:author_sort", author_sort);
    }
    meta(&mut output, "calibre:last_modified", &book.last_modified);
    output.push_str("  </metadata>\n");
    output.push_str(
        "  <guide><reference href=\"cover.jpg\" type=\"cover\" title=\"Cover\"/></guide>\n",
    );
    output.push_str("</package>\n");
    output
}

fn element(output: &mut String, name: &str, value: &str, attributes: &[(&str, &str)]) {
    let _ = write!(output, "    <{name}");
    for (attribute, attribute_value) in attributes {
        let _ = write!(
            output,
            " {attribute}=\"{}\"",
            escape_attribute(attribute_value)
        );
    }
    let _ = writeln!(output, ">{}</{name}>", escape_text(value));
}

fn meta(output: &mut String, name: &str, content: &str) {
    let _ = writeln!(
        output,
        "    <meta name=\"{}\" content=\"{}\"/>",
        escape_attribute(name),
        escape_attribute(content)
    );
}

fn escape_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\t'
            | '\n'
            | '\r'
            | '\u{20}'..='\u{D7FF}'
            | '\u{E000}'..='\u{FFFD}'
            | '\u{10000}'..='\u{10FFFF}' => escaped.push(character),
            _ => escaped.push('\u{FFFD}'),
        }
    }
    escaped
}

fn escape_attribute(value: &str) -> String {
    escape_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[allow(clippy::too_many_lines)] // Parsing every supported OPF field together exposes omissions.
fn parse(xml: &str, path: &Path, expected_id: BookId) -> Result<TrashBookMetadata> {
    let document = roxmltree::Document::parse(xml).map_err(|source| Error::InvalidLibrary {
        path: path.to_path_buf(),
        reason: format!("trash metadata.opf is invalid XML: {source}"),
    })?;
    let metadata = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "metadata")
        .ok_or_else(|| Error::InvalidLibrary {
            path: path.to_path_buf(),
            reason: "trash metadata.opf has no metadata element".into(),
        })?;

    for node in metadata.children().filter(roxmltree::Node::is_element) {
        if node.tag_name().name() == "meta" {
            let name = node.attribute("name").unwrap_or_default();
            if matches!(
                name,
                "calibre:user_metadata"
                    | "calibre:link_maps"
                    | "calibre:user_categories"
                    | "calibre:annotations"
            ) {
                return Err(Error::UnsupportedOperation {
                    operation: "restore book from trash",
                    reason: format!("metadata.opf contains unsupported state in {name}"),
                });
            }
        }
    }

    let title = dc_values(metadata, "title")
        .into_iter()
        .next()
        .unwrap_or_else(|| "Unknown".to_owned());
    let creator_nodes = dc_nodes(metadata, "creator")
        .into_iter()
        .filter(|node| {
            node.attribute((OPF_NAMESPACE, "role"))
                .or_else(|| node.attribute("role"))
                .is_none_or(|role| role.eq_ignore_ascii_case("aut"))
        })
        .collect::<Vec<_>>();
    let authors = creator_nodes
        .iter()
        .filter_map(node_text)
        .collect::<Vec<_>>();
    let authors = if authors.is_empty() {
        vec!["Unknown".to_owned()]
    } else {
        authors
    };
    let author_sort = creator_nodes
        .first()
        .and_then(|node| node.attribute((OPF_NAMESPACE, "file-as")))
        .map(str::to_owned)
        .or_else(|| meta_value(metadata, "calibre:author_sort"));

    let mut calibre_id = None;
    let mut uuid = None;
    let mut identifiers = BTreeMap::new();
    for node in dc_nodes(metadata, "identifier") {
        let Some(value) = node_text(&node) else {
            continue;
        };
        let scheme = node
            .attribute((OPF_NAMESPACE, "scheme"))
            .or_else(|| node.attribute("scheme"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        match scheme.as_str() {
            "calibre" => {
                calibre_id = value.parse::<i64>().ok();
            }
            "uuid" => uuid = Some(value),
            "" => {}
            _ => {
                identifiers.insert(scheme, value);
            }
        }
    }
    if calibre_id != Some(expected_id.get()) {
        return Err(Error::InvalidLibrary {
            path: path.to_path_buf(),
            reason: format!(
                "metadata.opf identifies book {:?}, expected {}",
                calibre_id,
                expected_id.get()
            ),
        });
    }

    let rating = match meta_value(metadata, "calibre:rating") {
        Some(value) => {
            let parsed = value.parse::<u8>().map_err(|_| Error::InvalidLibrary {
                path: path.to_path_buf(),
                reason: "metadata.opf has an invalid rating".into(),
            })?;
            Some(Rating::new(parsed).map_err(|_| Error::InvalidLibrary {
                path: path.to_path_buf(),
                reason: "metadata.opf rating is outside 1 through 10".into(),
            })?)
        }
        None => None,
    };
    let series_index = match meta_value(metadata, "calibre:series_index") {
        Some(value) => {
            let parsed = value.parse::<f64>().map_err(|_| Error::InvalidLibrary {
                path: path.to_path_buf(),
                reason: "metadata.opf has an invalid series index".into(),
            })?;
            if !parsed.is_finite() {
                return Err(Error::InvalidLibrary {
                    path: path.to_path_buf(),
                    reason: "metadata.opf has a non-finite series index".into(),
                });
            }
            parsed
        }
        None => 1.0,
    };

    Ok(TrashBookMetadata {
        id: expected_id,
        title,
        sort: meta_value(metadata, "calibre:title_sort"),
        timestamp: meta_value(metadata, "calibre:timestamp"),
        publication_date: dc_values(metadata, "date").into_iter().next(),
        author_sort,
        uuid,
        authors,
        tags: dc_values(metadata, "subject"),
        series: meta_value(metadata, "calibre:series"),
        series_index,
        publisher: dc_values(metadata, "publisher").into_iter().next(),
        languages: dc_values(metadata, "language"),
        identifiers,
        comments: dc_values(metadata, "description").into_iter().next(),
        rating,
    })
}

fn dc_nodes<'a, 'input>(
    metadata: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Vec<roxmltree::Node<'a, 'input>> {
    metadata
        .children()
        .filter(|node| {
            node.is_element()
                && node.tag_name().name() == name
                && node.tag_name().namespace() == Some(DC_NAMESPACE)
        })
        .collect()
}

fn dc_values(metadata: roxmltree::Node<'_, '_>, name: &str) -> Vec<String> {
    dc_nodes(metadata, name)
        .iter()
        .filter_map(node_text)
        .collect()
}

fn node_text(node: &roxmltree::Node<'_, '_>) -> Option<String> {
    node.text()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn meta_value(metadata: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    metadata
        .children()
        .find(|node| {
            node.is_element()
                && node.tag_name().name() == "meta"
                && node.attribute("name") == Some(name)
        })
        .and_then(|node| node.attribute("content").or_else(|| node.text()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{parse, serialize};
    use crate::{Book, BookId, Rating};
    use proptest::prelude::*;
    use std::path::Path;

    fn book() -> Book {
        Book {
            id: BookId::new(7),
            title: "A <title> & test".into(),
            sort: Some("title sort".into()),
            timestamp: Some("2026-01-02 03:04:05+00:00".into()),
            publication_date: Some("0800-01-01 00:00:00+00:00".into()),
            author_sort: Some("Homer".into()),
            relative_path: "Homer/A title (7)".into(),
            uuid: Some("uuid-value".into()),
            last_modified: "2026-01-02 03:04:06+00:00".into(),
            authors: Vec::new(),
            tags: Vec::new(),
            series: None,
            publisher: None,
            languages: Vec::new(),
            identifiers: Vec::new(),
            comments: Some("<p>hello & goodbye</p>".into()),
            rating: Some(Rating::new(8).expect("rating")),
            formats: Vec::new(),
            cover_path: None,
        }
    }

    #[test]
    fn generated_xml_is_well_formed_and_round_trips_core_values() {
        let mut source = book();
        source.authors.push(crate::Author {
            id: crate::AuthorId::new(1),
            name: "Homer & Co.".into(),
            sort: Some("Homer".into()),
            link: String::new(),
        });
        let xml = serialize(&source);
        let parsed =
            parse(&xml, Path::new("metadata.opf"), source.id).expect("parse generated OPF");
        assert_eq!(parsed.title, source.title);
        assert_eq!(parsed.authors, vec!["Homer & Co."]);
        assert_eq!(parsed.comments, source.comments);
        assert_eq!(parsed.rating, source.rating);
    }

    proptest! {
        #[test]
        fn generated_opf_is_valid_for_arbitrary_rust_strings(
            title in any::<String>(),
            comments in any::<String>(),
        ) {
            let mut source = book();
            source.title = title;
            source.comments = Some(comments);
            let xml = serialize(&source);
            prop_assert!(parse(&xml, Path::new("metadata.opf"), source.id).is_ok());
        }
    }
}
