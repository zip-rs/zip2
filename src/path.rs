//! Path manipulation utilities

use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    fs,
    path::{Component, Path, PathBuf},
};
use typed_path::{Utf8WindowsComponent, Utf8WindowsPath};

use crate::{
    read::extract::make_writable_dir_all,
    result::{ZipResult, invalid},
};

/// Parse an entry name with Windows rules on every host. A drive prefix at the start of a
/// later component (`c:x` in `/c:x`) is split off as a [`Utf8WindowsComponent::Prefix`], since
/// on Windows pushing `c:x` replaces the path.
fn components(name: &str) -> impl Iterator<Item = Utf8WindowsComponent<'_>> {
    Utf8WindowsPath::new(name)
        .components()
        .flat_map(|component| match component {
            Utf8WindowsComponent::Normal(s) => split_drive_prefix(s),
            component => [Some(component), None],
        })
        .flatten()
}

/// Split drive prefixes off one component: `c:x` becomes `Prefix(c:)` and `Normal("x")`.
fn split_drive_prefix(component: &str) -> [Option<Utf8WindowsComponent<'_>>; 2] {
    let mut rest = component;
    // `c:d:x` is `c:` followed by `d:x`, which is a drive prefix again.
    while let [drive, b':', ..] = rest.as_bytes()
        && drive.is_ascii_alphabetic()
    {
        rest = &rest[2..];
    }
    // `c:d:` parses as `Prefix(c:)` then `Normal("d:")`; one `Prefix` is enough.
    let prefix = Utf8WindowsPath::new(&component[..component.len() - rest.len()])
        .components()
        .next();
    let rest = match rest {
        "" => None,
        "." => Some(Utf8WindowsComponent::CurDir),
        ".." => Some(Utf8WindowsComponent::ParentDir),
        rest => Some(Utf8WindowsComponent::Normal(rest)),
    };
    [prefix, rest]
}

/// Simplify an archive entry name into components to be resolved *under*
/// the destination directory.
///
/// NOTE: This intentionally drops absolute-path information (`Prefix` /
/// `RootDir`). The result MUST NOT be used for containment checks; use
/// [`resolve_enclosed`] instead.
pub(crate) fn simplified_components(input: &Path) -> Option<Vec<&OsStr>> {
    let mut out = Vec::new();
    let input_str = input.to_str()?;
    for component in components(input_str) {
        match component {
            // Skip prefix and root directory components instead of rejecting the entire path
            // This allows extraction of ZIP files with absolute paths, similar to other ZIP tools
            Utf8WindowsComponent::Prefix(_) | Utf8WindowsComponent::RootDir => (),
            Utf8WindowsComponent::ParentDir => {
                out.pop()?;
            }
            Utf8WindowsComponent::Normal(s) => {
                // As in `enclosed_name`: a separator inside one component means the name is
                // split differently when it is written.
                // Colons inside a component (not a leading drive prefix) indicate Windows
                // Alternate Data Streams (ADS) or invalid filename characters.
                // On Unix, colons are permitted in filenames (e.g. cargo-afl outputs).
                if s.contains(['/', '\\']) || (cfg!(windows) && s.contains(':')) {
                    return None;
                }
                out.push(OsStr::new(s));
            }
            Utf8WindowsComponent::CurDir => (),
        }
    }
    Some(out)
}

pub(crate) fn file_name_sanitized(no_null_filename: &str) -> PathBuf {
    components(no_null_filename)
        .filter_map(|component| match component {
            Utf8WindowsComponent::Normal(s) => Some(s),
            _ => None,
        })
        // In verbatim paths `/` isn't a separator, so a component can still contain `..` or a
        // drive prefix.
        .flat_map(|s| s.split(['/', '\\']))
        .flat_map(split_drive_prefix)
        .filter_map(|component| match component {
            Some(Utf8WindowsComponent::Normal(s)) => Some(s),
            _ => None,
        })
        .collect()
}

pub(crate) fn enclosed_name(file_name: &str) -> Option<PathBuf> {
    let mut depth = 0usize;
    let mut out_path = PathBuf::new();
    for component in components(file_name) {
        match component {
            Utf8WindowsComponent::Prefix(_) | Utf8WindowsComponent::RootDir => {
                if depth > 0 {
                    return None;
                }
            }
            Utf8WindowsComponent::ParentDir => {
                depth = depth.checked_sub(1)?;
                out_path.pop();
            }
            Utf8WindowsComponent::Normal(s) => {
                // Inside a verbatim (`\\?\`) path `/` is not a separator, so `../b` comes back
                // as one component; joined onto an ordinary path it is split again. The check
                // here and the eventual write must agree on where a component ends.
                // Colons inside a component indicate Windows Alternate Data Streams (ADS)
                // (e.g. `file:Zone.Identifier` or `file:stream`), which bypass security
                // policies or access hidden file streams.
                // On Unix, colons are permitted in filenames (e.g. cargo-afl outputs).
                if s.contains(['/', '\\']) || (cfg!(windows) && s.contains(':')) {
                    return None;
                }
                depth += 1;
                out_path.push(s);
            }
            Utf8WindowsComponent::CurDir => (),
        }
    }
    Some(out_path)
}

/// Strip `base` from the front of the absolute path `path`, returning the components
/// that follow it, or `None` if `path` is not inside `base`.
///
/// This is [`Path::strip_prefix`] with one addition: on Windows it also accepts a `path`
/// that denotes the same volume as `base` in a different form. `base` is canonicalized
/// and therefore uses the verbatim form (`\\?\C:\...`), while a target taken from an
/// archive or from [`std::fs::read_link`] is usually in the ordinary form (`C:\...`).
/// Without this, every absolute target would be rejected on Windows.
pub(crate) fn strip_base_prefix<'p>(base: &Path, path: &'p Path) -> Option<&'p Path> {
    if let Ok(rest) = path.strip_prefix(base) {
        return Some(rest);
    }
    strip_equivalent_prefix(base, path)
}

/// Windows-only fallback of [`strip_base_prefix`], comparing the two prefixes by the
/// volume they denote rather than by their spelling.
///
/// The `#[cfg]` here is on the platform doing the extraction, not on the platform that
/// wrote the archive. An archive is free to contain a verbatim prefix — `\\?\C:\foo` as
/// an entry name or as a symlink target — but `base` only ever reaches this function
/// through `fs::canonicalize` (`ZipArchive::extract`, `ZipArchive::extract_unwrapped_root_dir`
/// and `ZipStreamReader::extract` all canonicalize the destination), so it is always
/// spelled natively, and only Windows parses the other side back into a `Prefix`
/// component; elsewhere it stays an ordinary file name.
///
/// What the archive itself supplies is covered on every platform instead: entry names,
/// which are parsed with `Utf8WindowsPath` whatever the host is, by
/// `test_simplified_components_verbatim_prefix` and `test_enclosed_name_verbatim_prefix`
/// below, and symlink targets by `test_symlink_target_with_verbatim_prefix_cannot_escape`
/// in `tests/extract_symlink.rs`.
#[cfg(windows)]
fn strip_equivalent_prefix<'p>(base: &Path, path: &'p Path) -> Option<&'p Path> {
    use std::path::Prefix;

    /// Reduce the verbatim forms to the form they are equivalent to, so that `\\?\C:\`
    /// and `C:\`, or `\\?\UNC\server\share` and `\\server\share`, compare equal. Drive
    /// letters are case-insensitive, but nothing else is normalized here: the remaining
    /// components are still compared exactly.
    fn volume(prefix: Prefix<'_>) -> Prefix<'_> {
        match prefix {
            Prefix::Disk(disk) | Prefix::VerbatimDisk(disk) => {
                Prefix::Disk(disk.to_ascii_uppercase())
            }
            Prefix::VerbatimUNC(server, share) => Prefix::UNC(server, share),
            other => other,
        }
    }

    let mut base_components = base.components();
    let mut path_components = path.components();

    match (base_components.next(), path_components.next()) {
        (Some(Component::Prefix(base_prefix)), Some(Component::Prefix(path_prefix)))
            if volume(base_prefix.kind()) == volume(path_prefix.kind()) => {}
        _ => return None,
    }

    loop {
        // Taken before advancing, so that it is the remainder once `base` runs out.
        let rest = path_components.as_path();
        match (base_components.next(), path_components.next()) {
            (None, _) => return Some(rest),
            (Some(base_component), Some(path_component)) if base_component == path_component => {}
            _ => return None,
        }
    }
}

#[cfg(not(windows))]
fn strip_equivalent_prefix<'p>(_base: &Path, _path: &'p Path) -> Option<&'p Path> {
    None
}

/// The maximum number of symlinks that may be followed while resolving a single
/// path (the equivalent of `MAXSYMLINKS` on Linux).
const MAX_SYMLINK_HOPS: usize = 40;

/// Resolve `components` into a path that is guaranteed to stay inside `base`.
///
/// Resolution starts at `start`, which must be `base` itself or a path inside it,
/// and walks one component at a time. When a symlink is encountered, its target is
/// pushed back onto the queue rather than being trusted as-is, so every component
/// of the target is checked in exactly the same way. This is what prevents an
/// outward symlink from being smuggled in as an intermediate component of a target
/// that only *looks* enclosed.
///
/// `base` is assumed to be canonicalized. When `create_dirs` is set, missing
/// intermediate directories are created; the last component is left alone.
///
/// Returns an error if resolution would leave `base`.
pub(crate) fn resolve_enclosed(
    base: &Path,
    start: &Path,
    components: impl IntoIterator<Item = OsString>,
    create_dirs: bool,
) -> ZipResult<PathBuf> {
    debug_assert!(start.starts_with(base));

    let mut current = start.to_path_buf();
    let mut queue: VecDeque<OsString> = components.into_iter().collect();
    let mut hops = 0usize;

    // Set once a component does not exist (and is not created). A later entry may create it as a
    // symlink, which would change the meaning of any subsequent `..` to the unknown
    // target's parent rather than the symlink's parent.
    let mut missing = false;

    while let Some(component) = queue.pop_front() {
        if component.is_empty() || component == OsStr::new(".") {
            continue;
        }
        if component == OsStr::new("..") {
            if missing {
                return Err(invalid!(
                    "Path may escape the destination directory through a future symlink - '..' after a missing component"
                ));
            }
            // Never walk above `base`: `..` is rejected once we are back at `base`.
            if current == base {
                return Err(invalid!(
                    "Path escapes the destination directory - component is '..'"
                ));
            }
            current.pop();
            continue;
        }

        current.push(&component);
        // Pushing a prefixed component replaces `current` on Windows; check before creating dirs.
        if !current.starts_with(base) {
            return Err(invalid!(
                "Path escapes the destination directory - component is not a plain name"
            ));
        }

        let meta = match fs::symlink_metadata(&current) {
            Ok(meta) => meta,
            // Nothing to follow here, so keep walking; this is also what allows a
            // symlink whose target does not exist (yet) to be accepted.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if create_dirs && !queue.is_empty() {
                    make_writable_dir_all(&current)?;
                } else {
                    missing = true;
                }
                continue;
            }
            Err(e) => return Err(e.into()),
        };

        if !meta.is_symlink() {
            continue;
        }

        hops += 1;
        if hops > MAX_SYMLINK_HOPS {
            return Err(invalid!("Extraction followed a symlink too deep"));
        }

        let target = fs::read_link(&current)?;
        current.pop(); // consume the symlink itself and resolve its target from its parent

        let rest: PathBuf = if target.is_absolute() {
            // An absolute target is rejected unless it can be reduced to a path relative
            // to the canonicalized `base`. Whole components are compared, so a different
            // Windows drive or UNC share does not match.
            let Some(rest) = strip_base_prefix(base, &target) else {
                return Err(invalid!("Symlink target escapes the destination directory"));
            };
            current = base.to_path_buf();
            rest.to_path_buf()
        } else {
            target
        };
        // Push the target's components back to the front of the queue, so that its
        // intermediate components are checked just like any other component.
        for c in rest.components().rev() {
            match c {
                Component::Prefix(_) | Component::RootDir => {
                    return Err(invalid!("Invalid symlink target path"));
                }
                Component::CurDir => (),
                Component::ParentDir => queue.push_front(OsString::from("..")),
                Component::Normal(s) => queue.push_front(s.to_os_string()),
            }
        }
    }
    // After processing all components, verify we're still inside base
    if !current.starts_with(base) {
        return Err(invalid!(
            "Path escapes the destination directory and not allowed outside"
        ));
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::{enclosed_name, file_name_sanitized, simplified_components};
    use std::ffi::OsStr;
    use std::path::{Component, Path, PathBuf};

    #[test]
    fn test_file_name_sanitized_splits_verbatim_component() {
        for (name, expected) in [
            (r"\\?\C:\../b", "b"),
            (r"\\?\C:\a/../../b", "a/b"),
            (r"\\?\C:\a\..\..\b", "a/b"),
            (r"\\?\C:\a/./b", "a/b"),
            ("../../b", "b"),
            ("a/b", "a/b"),
        ] {
            let sanitized = file_name_sanitized(name);
            assert_eq!(sanitized, PathBuf::from(expected), "{name}");
            assert!(
                sanitized
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
                "{name} -> {sanitized:?}"
            );
        }
    }

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

    #[test]
    fn test_simplified_components_windows_absolute_path() {
        let path = Path::new(r"C:\foo\bar\baz.txt");
        let components = simplified_components(path).unwrap();
        assert_eq!(components.len(), 3);
        assert_eq!(components[0], "foo");
        assert_eq!(components[1], "bar");
        assert_eq!(components[2], "baz.txt");
    }

    /// Nothing stops an archive from spelling an entry name with a verbatim prefix, and
    /// entry names are parsed with `Utf8WindowsPath` whatever the host is, so this has to
    /// hold on every platform rather than on Windows alone.
    #[test]
    fn test_simplified_components_verbatim_prefix() {
        for path in [
            r"\\?\C:\foo\bar.txt",
            r"\\?\UNC\server\share\foo\bar.txt",
            r"\\.\C:\foo\bar.txt",
        ] {
            let components = simplified_components(Path::new(path)).unwrap();
            assert_eq!(components, ["foo", "bar.txt"], "{path}");
        }

        let components = simplified_components(Path::new(r"\\?\C:\foo\..\bar.txt")).unwrap();
        assert_eq!(components, ["bar.txt"]);

        let components = simplified_components(Path::new(r"\\?\C:\")).unwrap();
        assert!(components.is_empty());

        // A verbatim prefix must not buy the entry any extra `..`s either.
        assert_eq!(
            simplified_components(Path::new(r"\\?\C:\..\..\etc\passwd")),
            None
        );
    }

    /// The same for the public [`enclosed_name`] path.
    #[test]
    fn test_enclosed_name_verbatim_prefix() {
        for name in [
            r"\\?\C:\foo\bar.txt",
            r"\\?\UNC\server\share\foo\bar.txt",
            r"\\.\C:\foo\bar.txt",
        ] {
            assert_eq!(
                enclosed_name(name),
                Some(PathBuf::from_iter(["foo", "bar.txt"])),
                "{name}"
            );
        }

        assert_eq!(enclosed_name(r"\\?\C:\..\..\etc\passwd"), None);
    }

    /// Inside a verbatim path `/` is not a separator, so the parser hands `../b` back as one
    /// `Normal` component. Once the result is joined onto an ordinary destination path, `/` is
    /// a separator again, so a caller would write one level above the destination. A name
    /// whose component holds a separator is not enclosed, as `\\?\C:\..\b` and `../b` are not.
    #[test]
    fn test_enclosed_name_verbatim_prefix_with_a_separator_inside_a_component() {
        for name in [
            r"\\?\C:\../b",
            r"\\?\C:\a/../../b",
            r"\\?\UNC\server\share\../b",
            r"\\?\C:\a\..\../b",
            r"\\?\C:\..\b",
            "../b",
        ] {
            assert_eq!(enclosed_name(name), None, "{name}");
            assert_eq!(simplified_components(Path::new(name)), None, "{name}");
        }
    }

    /// `base` is canonicalized, so on Windows it is spelled in the verbatim form. A
    /// target coming from an archive or from `read_link` usually is not, and must still
    /// be recognized as being inside `base`.
    ///
    /// Gated on the extracting platform rather than on the platform the archive came
    /// from; see the comment on `strip_equivalent_prefix` for why that distinction holds.
    #[cfg(windows)]
    #[test]
    fn test_strip_base_prefix_accepts_equivalent_windows_prefixes() {
        use super::strip_base_prefix;

        let base = Path::new(r"\\?\C:\dest");
        for path in [r"C:\dest\sub\file.txt", r"c:\dest\sub\file.txt"] {
            assert_eq!(
                strip_base_prefix(base, Path::new(path)),
                Some(Path::new(r"sub\file.txt")),
                "{path} should be inside {}",
                base.display()
            );
        }
        assert_eq!(
            strip_base_prefix(base, Path::new(r"C:\dest")),
            Some(Path::new(""))
        );

        let unc_base = Path::new(r"\\?\UNC\server\share\dest");
        assert_eq!(
            strip_base_prefix(unc_base, Path::new(r"\\server\share\dest\file.txt")),
            Some(Path::new("file.txt"))
        );
    }

    /// A different volume must not be accepted just because the rest of the path matches.
    #[test]
    fn test_strip_base_prefix_rejects_other_windows_volumes() {
        use super::strip_base_prefix;

        let base = Path::new(r"\\?\C:\dest");
        for path in [
            r"D:\dest\file.txt",
            r"\\attacker\share\dest\file.txt",
            r"C:\dest-sibling\file.txt",
            r"C:\other\file.txt",
        ] {
            assert_eq!(
                strip_base_prefix(base, Path::new(path)),
                None,
                "{path} should not be inside {}",
                base.display()
            );
        }
    }

    /// Each component a name is reduced to must parse as one normal component under Windows rules.
    fn assert_each_is_one_windows_normal_component<'a>(
        name: &str,
        components: impl IntoIterator<Item = &'a OsStr>,
    ) {
        use typed_path::{Utf8WindowsComponent, Utf8WindowsPath};
        for component in components {
            let component = component.to_str().unwrap();
            let parsed: Vec<_> = Utf8WindowsPath::new(component).components().collect();
            assert_eq!(
                parsed,
                [Utf8WindowsComponent::Normal(component)],
                "{name:?} yields {component:?}, which Windows parses as {parsed:?}"
            );
        }
    }

    /// On Windows `c:x` is drive-relative, so `dest.join(enclosed_name)` must never see one.
    const NAMES_WITH_A_DRIVE_PREFIX_INSIDE: &[&str] = &[
        "/c:x",
        r"\c:x",
        "./c:x",
        "a/../c:x",
        "/c:",
        "/c:x/y",
        "b/c:x",
        "b/c:",
        "b/C:/y",
        "/c:d:x",
        "/c:.",
        "/c:..",
        "b/c:..",
        r"\\?\C:\b\c:x",
        r"\\?\C:\a/c:x",
        r"\\?\C:\a/c:../b",
    ];

    #[test]
    fn test_enclosed_name_drive_prefix_inside_the_name() {
        for name in NAMES_WITH_A_DRIVE_PREFIX_INSIDE {
            if let Some(path) = enclosed_name(name) {
                assert_each_is_one_windows_normal_component(name, path.iter());
            }
        }
        // A leading drive prefix is dropped, as for `c:x`.
        assert_eq!(enclosed_name("c:x"), Some(PathBuf::from("x")));
        assert_eq!(enclosed_name("/c:x"), Some(PathBuf::from("x")));
        assert_eq!(enclosed_name("./c:x"), Some(PathBuf::from("x")));
        assert_eq!(enclosed_name("a/../c:x"), Some(PathBuf::from("x")));
        assert_eq!(
            enclosed_name("/c:x/y"),
            Some(PathBuf::from_iter(["x", "y"]))
        );
        assert_eq!(enclosed_name("/c:"), Some(PathBuf::new()));
        // After a normal component it's rejected, like a `Prefix` or `RootDir` there.
        assert_eq!(enclosed_name("b/c:x"), None);
        assert_eq!(enclosed_name("b/C:/y"), None);
        assert_eq!(enclosed_name("/c:.."), None);
    }

    #[test]
    fn test_simplified_components_drive_prefix_inside_the_name() {
        for name in NAMES_WITH_A_DRIVE_PREFIX_INSIDE {
            if let Some(components) = simplified_components(Path::new(name)) {
                assert_each_is_one_windows_normal_component(name, components);
            }
        }
        // Dropped wherever it is, as a leading prefix or root already is.
        assert_eq!(simplified_components(Path::new("/c:x")).unwrap(), ["x"]);
        assert_eq!(
            simplified_components(Path::new("b/c:x")).unwrap(),
            ["b", "x"]
        );
        assert_eq!(simplified_components(Path::new("/c:d:x")).unwrap(), ["x"]);
        assert_eq!(simplified_components(Path::new("b/c:..")).unwrap(), [""; 0]);
        assert_eq!(simplified_components(Path::new("/c:..")), None);
    }

    #[test]
    fn test_file_name_sanitized_drive_prefix_inside_the_name() {
        use super::file_name_sanitized;
        for name in NAMES_WITH_A_DRIVE_PREFIX_INSIDE {
            let path = file_name_sanitized(name);
            assert_each_is_one_windows_normal_component(name, path.iter());
        }
        assert_eq!(file_name_sanitized("b/c:x"), PathBuf::from_iter(["b", "x"]));
    }

    /// Colons inside a component (not a leading drive prefix) represent Windows Alternate Data
    /// Streams (ADS) and must be rejected on Windows to prevent MotW bypass or arbitrary stream writes.
    #[test]
    fn test_colon_that_is_not_a_drive_prefix_is_rejected() {
        if !cfg!(windows) {
            // On Unix, colons are permitted in filenames (e.g. cargo-afl outputs).
            return;
        }
        for name in [
            "foo:bar",
            "b/ab:c",
            "b/1:x",
            "b/:x",
            "b/a.b:c:d",
            "target.exe:Zone.Identifier",
            "foo::$DATA",
        ] {
            assert_eq!(enclosed_name(name), None, "{name}");
            assert_eq!(simplified_components(Path::new(name)), None, "{name}");
        }
    }

    /// On Unix, colons inside filenames are legitimate and must be accepted.
    #[test]
    #[cfg(not(windows))]
    fn test_colon_permitted_on_unix() {
        for name in ["foo:bar", "b/ab:c", "id:000000,sig:11"] {
            assert!(enclosed_name(name).is_some(), "{name}");
            assert!(simplified_components(Path::new(name)).is_some(), "{name}");
        }
    }

    #[test]
    fn test_simplified_components_windows_unc_path() {
        let path = Path::new(r"\\server\share\foo\bar.txt");
        let components = simplified_components(path).unwrap();
        assert_eq!(components.len(), 2);
        assert_eq!(components[0], "foo");
        assert_eq!(components[1], "bar.txt");
    }
}
