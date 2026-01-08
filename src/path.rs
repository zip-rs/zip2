//! Path manipulation utilities

use std::{
    ffi::OsStr,
    path::{Component, Path},
};

/// Simplify a path by removing the prefix and parent directories and only return normal components
pub(crate) fn simplified_components(input: &Path) -> Option<Vec<&OsStr>> {
    let mut out = Vec::new();
    for component in input.components() {
        match component {
            // Skip prefix and root directory components instead of rejecting the entire path
            // This allows extraction of ZIP files with absolute paths, similar to other ZIP tools
            Component::Prefix(_) | Component::RootDir => (),
            Component::ParentDir => {
                out.pop()?;
            }
            Component::Normal(_) => out.push(component.as_os_str()),
            Component::CurDir => (),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_simplified_components_relative_path() {
        let path = Path::new("foo/bar/baz.txt");
        let components = simplified_components(path).unwrap();
        assert_eq!(components.len(), 3);
        assert_eq!(components[0], "foo");
        assert_eq!(components[1], "bar");
        assert_eq!(components[2], "baz.txt");
    }

    #[test]
    fn test_simplified_components_absolute_unix_path() {
        let path = Path::new("/foo/bar/baz.txt");
        let components = simplified_components(path).unwrap();
        assert_eq!(components.len(), 3);
        assert_eq!(components[0], "foo");
        assert_eq!(components[1], "bar");
        assert_eq!(components[2], "baz.txt");
    }

    #[test]
    fn test_simplified_components_with_parent_dirs() {
        let path = Path::new("foo/../bar/baz.txt");
        let components = simplified_components(path).unwrap();
        assert_eq!(components.len(), 2);
        assert_eq!(components[0], "bar");
        assert_eq!(components[1], "baz.txt");
    }

    #[test]
    fn test_simplified_components_too_many_parent_dirs() {
        let path = Path::new("foo/../../bar");
        let result = simplified_components(path);
        assert!(result.is_none()); // Should still fail for directory traversal attacks
    }

    #[test]
    fn test_simplified_components_with_current_dir() {
        let path = Path::new("foo/./bar/baz.txt");
        let components = simplified_components(path).unwrap();
        assert_eq!(components.len(), 3);
        assert_eq!(components[0], "foo");
        assert_eq!(components[1], "bar");
        assert_eq!(components[2], "baz.txt");
    }

    #[test]
    fn test_simplified_components_empty_path() {
        let path = Path::new("");
        let components = simplified_components(path).unwrap();
        assert_eq!(components.len(), 0);
    }

    #[test]
    fn test_simplified_components_root_only() {
        let path = Path::new("/");
        let components = simplified_components(path).unwrap();
        assert_eq!(components.len(), 0);
    }

    #[cfg(windows)]
    #[test]
    fn test_simplified_components_windows_absolute_path() {
        let path = Path::new(r"C:\foo\bar\baz.txt");
        let components = simplified_components(path).unwrap();
        assert_eq!(components.len(), 3);
        assert_eq!(components[0], "foo");
        assert_eq!(components[1], "bar");
        assert_eq!(components[2], "baz.txt");
    }

    #[cfg(windows)]
    #[test]
    fn test_simplified_components_windows_unc_path() {
        let path = Path::new(r"\\server\share\foo\bar.txt");
        let components = simplified_components(path).unwrap();
        assert_eq!(components.len(), 2);
        assert_eq!(components[0], "foo");
        assert_eq!(components[1], "bar.txt");
    }
}
