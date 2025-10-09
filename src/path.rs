//! Path manipulation utilities

use std::{
    ffi::OsStr,
    path::{Component, Path},
};

#[cfg(feature = "camino")]
use camino::{Utf8Component, Utf8Path};

/// Simplify a path by removing the prefix and parent directories and only return normal components
pub(crate) fn simplified_components(input: &Path) -> Option<Vec<&OsStr>> {
    let mut out = Vec::new();
    for component in input.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => return None,
            Component::ParentDir => {
                out.pop()?;
            }
            Component::Normal(_) => out.push(component.as_os_str()),
            Component::CurDir => (),
        }
    }
    Some(out)
}

/// Simplify a UTF-8 path by removing the prefix and parent directories and only return normal components
#[cfg(feature = "camino")]
pub(crate) fn simplified_components_utf8(input: &Utf8Path) -> Option<Vec<&str>> {
    let mut out = Vec::new();
    for component in input.components() {
        match component {
            Utf8Component::Prefix(_) | Utf8Component::RootDir => return None,
            Utf8Component::ParentDir => {
                out.pop()?;
            }
            Utf8Component::Normal(name) => out.push(name),
            Utf8Component::CurDir => (),
        }
    }
    Some(out)
}
