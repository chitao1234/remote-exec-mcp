use std::ffi::OsStr;
use std::path::{Component, Path};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(windows)]
use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

pub fn component_eq(left: &OsStr, right: &OsStr) -> bool {
    os_str_eq(left, right)
}

pub fn path_eq(left: &Path, right: &Path) -> bool {
    let mut left_components = left.components();
    let mut right_components = right.components();

    loop {
        match (left_components.next(), right_components.next()) {
            (None, None) => return true,
            (Some(left), Some(right)) if path_component_eq(left, right) => {}
            _ => return false,
        }
    }
}

pub fn path_has_prefix(path: &Path, prefix: &Path) -> bool {
    let mut path_components = path.components();

    for prefix_component in prefix.components() {
        let Some(path_component) = path_components.next() else {
            return false;
        };
        if !path_component_eq(path_component, prefix_component) {
            return false;
        }
    }

    true
}

pub fn path_is_within(path: &Path, root: &Path) -> bool {
    path_has_prefix(path, root)
}

fn path_component_eq(left: Component<'_>, right: Component<'_>) -> bool {
    component_eq(left.as_os_str(), right.as_os_str())
}

#[cfg(windows)]
fn os_str_eq(left: &OsStr, right: &OsStr) -> bool {
    let left_wide: Vec<u16> = left.encode_wide().collect();
    let right_wide: Vec<u16> = right.encode_wide().collect();
    unsafe {
        CompareStringOrdinal(
            wide_ptr(&left_wide),
            left_wide.len() as i32,
            wide_ptr(&right_wide),
            right_wide.len() as i32,
            1,
        ) == CSTR_EQUAL
    }
}

#[cfg(not(windows))]
fn os_str_eq(left: &OsStr, right: &OsStr) -> bool {
    left == right
}

#[cfg(windows)]
fn wide_ptr(wide: &[u16]) -> *const u16 {
    if wide.is_empty() {
        std::ptr::null()
    } else {
        wide.as_ptr()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use super::{component_eq, path_eq, path_has_prefix};

    #[cfg(not(windows))]
    #[test]
    fn posix_path_comparison_preserves_case() {
        assert!(!component_eq(
            OsStr::new("Artifact.txt"),
            OsStr::new("artifact.txt")
        ));
        assert!(!path_eq(
            Path::new("/tmp/Artifact.txt"),
            Path::new("/tmp/artifact.txt"),
        ));
        assert!(!path_has_prefix(
            Path::new("/tmp/project/file.txt"),
            Path::new("/TMP/project"),
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_comparison_uses_native_case_insensitive_component_checks() {
        use super::path_is_within;

        assert!(component_eq(OsStr::new("RÉSUMÉ"), OsStr::new("résumé")));
        assert!(path_eq(
            Path::new(r"C:\RÉSUMÉ\Ärger.txt"),
            Path::new(r"c:\résumé\ärger.txt"),
        ));
        assert!(path_has_prefix(
            Path::new(r"C:\RÉSUMÉ\bin\zsh.exe"),
            Path::new(r"c:\résumé"),
        ));
        assert!(path_is_within(
            Path::new(r"C:\RÉSUMÉ\bin\zsh.exe"),
            Path::new(r"c:\résumé"),
        ));
        assert!(!path_has_prefix(
            Path::new(r"C:\RÉSUMÉ-tools\bin\zsh.exe"),
            Path::new(r"c:\résumé"),
        ));
    }
}
