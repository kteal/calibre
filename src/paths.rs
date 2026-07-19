use crate::{Error, Result};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub(crate) fn validate_relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err(Error::PathEscape {
            path: path.to_path_buf(),
            reason: "empty path".into(),
        });
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(Error::PathEscape {
                path: path.to_path_buf(),
                reason: "only normal relative path components are allowed".into(),
            });
        }
    }
    Ok(())
}

pub(crate) fn resolve(root: &Path, relative: &Path) -> Result<PathBuf> {
    validate_relative(relative)?;
    let joined = root.join(relative);
    let mut cursor = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            unreachable!("validated above");
        };
        cursor.push(part);
        match std::fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let canonical = std::fs::canonicalize(&cursor).map_err(|source| {
                    crate::error::io_error("canonicalize symlink", &cursor, source)
                })?;
                if !canonical.starts_with(root) {
                    return Err(Error::PathEscape {
                        path: relative.to_path_buf(),
                        reason: "symlink resolves outside the library".into(),
                    });
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(source) => {
                return Err(crate::error::io_error(
                    "inspect library path",
                    &cursor,
                    source,
                ));
            }
        }
    }
    Ok(joined)
}

pub(crate) fn format_from_path(path: &Path) -> Result<String> {
    let extension = path.extension().ok_or_else(|| Error::InvalidInput {
        field: "format path",
        reason: "a file extension is required".into(),
    })?;
    format_name(extension)
}

pub(crate) fn format_name(value: &OsStr) -> Result<String> {
    let value = value.to_str().ok_or_else(|| Error::InvalidInput {
        field: "format",
        reason: "format extensions must be UTF-8 ASCII".into(),
    })?;
    if value.is_empty()
        || value.len() > 16
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(Error::InvalidInput {
            field: "format",
            reason: "expected 1 to 16 ASCII letters or digits".into(),
        });
    }
    Ok(value.to_ascii_uppercase())
}

pub(crate) fn sanitize_component(value: &str) -> String {
    let mut result = String::new();
    for character in value
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
    {
        let replacement = match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_control() => '_',
            character => character,
        };
        result.push(replacement);
    }
    let trimmed = result.trim().trim_end_matches(['.', ' ']);
    let source = if trimmed.is_empty() {
        "Unknown"
    } else {
        trimmed
    };
    truncate_utf8(source, 100)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].trim_end().to_owned()
}

pub(crate) fn book_relative_path(title: &str, first_author: &str, id: i64) -> PathBuf {
    let author = sanitize_component(first_author);
    let title = sanitize_component(title);
    PathBuf::from(author).join(format!("{title} ({id})"))
}

pub(crate) fn format_stem(title: &str, first_author: &str) -> String {
    sanitize_component(&format!("{title} - {first_author}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn sanitized_components_are_bounded_and_relative(input in ".*") {
            let output = sanitize_component(&input);
            prop_assert!(!output.is_empty());
            prop_assert!(output.len() <= 100);
            prop_assert_eq!(Path::new(&output).components().count(), 1);
            prop_assert!(!output.contains('/'));
            prop_assert!(!output.contains('\\'));
        }
    }

    #[test]
    fn rejects_parent_paths() {
        assert!(validate_relative(Path::new("../outside")).is_err());
    }
}
