//! The application file API's fixed limits and path rules (spec 0013, spec
//! 0018, spec 0019).
//!
//! Three read-only `core` builtins expose the image tree to applications
//! (`fs_list`, `fs_read`, `fs_size`), and `core.spawn`/`core.run` resolve their
//! launch path against the same tree. `core.fs_write` is the one write
//! primitive and is confined to the writable `home` mount. This module holds
//! the limits shared by the compiler and the interpreter, and the path-syntax
//! validators every call site uses, so a malformed or unwritable path means the
//! same thing everywhere.
//!
//! Paths are relative to the image root (`spec 0018`, L6): `base/shell`,
//! `lib/ui/menu.ig`, `home/readme.txt`. They never start or end with `/`, never
//! contain an empty, `.` or `..` component, are at most [`PATH_MAX`] bytes, use
//! at most [`MAX_COMPONENTS`] components and each component is at most
//! [`NAME_CAP`] bytes.

use crate::Status;

/// Longest accepted path in bytes (spec 0018, L6).
pub const PATH_MAX: usize = 256;

/// Longest name a listing can return, and the longest accepted single path
/// component, in bytes (spec 0013, spec 0018 L6).
pub const NAME_CAP: usize = 64;

/// Most components a path may have (spec 0018, L6). The ext2 walker stops here.
pub const MAX_COMPONENTS: usize = 8;

/// The fixed read chunk in bytes: `core.fs_read` never copies more than this
/// per call, and the compiler requires a destination at least this large
/// (spec 0013).
pub const READ_CHUNK: usize = 256;

/// `kind` for a regular file in a `core.fs_list` result (spec 0013).
pub const KIND_FILE: u64 = 0;

/// `kind` for a directory in a `core.fs_list` result.
pub const KIND_DIR: u64 = 1;

/// `kind` for anything else (a device, socket or symlink); such an entry is
/// listed but cannot be read through the API.
pub const KIND_OTHER: u64 = 2;

/// Validates a relative image path (spec 0018, L6): at most [`PATH_MAX`]
/// bytes, not empty, no leading or trailing `/`, no empty, `.` or `..`
/// component, at most [`MAX_COMPONENTS`] components and each at most
/// [`NAME_CAP`] bytes. The bytes must already be valid UTF-8; callers convert
/// first.
///
/// Over-long paths are [`Status::CapacityExceeded`]; every other syntax error
/// is [`Status::Invalid`].
pub fn validate_path(path: &str) -> Status {
    if path.len() > PATH_MAX {
        return Status::CapacityExceeded;
    }
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') {
        return Status::Invalid;
    }
    let mut components = 0usize;
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.len() > NAME_CAP {
            return Status::Invalid;
        }
        components += 1;
    }
    if components > MAX_COMPONENTS {
        return Status::Invalid;
    }
    Status::Ok
}

/// Validates a `core.fs_list` directory path: the root (`""` or `"."`) or a
/// path that passes [`validate_path`] (spec 0018, A4).
pub fn validate_dir(path: &str) -> Status {
    if path.is_empty() || path == "." {
        Status::Ok
    } else {
        validate_path(path)
    }
}

/// Validates a writable path (spec 0019, A2): it must pass [`validate_path`]
/// and its first component must be `home`. Every other path is
/// [`Status::Invalid`], which is also what a read-only mount's host reports.
pub fn validate_writable(path: &str) -> Status {
    match validate_path(path) {
        Status::Ok => {
            if path == "home" || path.starts_with("home/") {
                Status::Ok
            } else {
                Status::Invalid
            }
        }
        status => status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_syntax_is_the_one_tree_rule() {
        for path in [
            "child",
            "a.b",
            "x_y",
            "base/child",
            "lib/ui/menu.ig",
            "a/b/c/d/e/f/g/h",
        ] {
            assert_eq!(validate_path(path), Status::Ok, "{path}");
        }
        for path in [
            "",
            ".",
            "..",
            "/child",
            "child/",
            "a//b",
            "../child",
            "a/../b",
            "base/./child",
            "a/b/c/d/e/f/g/h/i",
        ] {
            assert_eq!(validate_path(path), Status::Invalid, "{path}");
        }
        let long = "x".repeat(PATH_MAX + 1);
        assert_eq!(validate_path(&long), Status::CapacityExceeded);
        // Exactly PATH_MAX bytes over the eight-component cap still fits.
        let first = "x".repeat(32);
        let rest = "y".repeat(31);
        let exact = [
            first.as_str(),
            rest.as_str(),
            rest.as_str(),
            rest.as_str(),
            rest.as_str(),
            rest.as_str(),
            rest.as_str(),
            rest.as_str(),
        ]
        .join("/");
        assert_eq!(exact.len(), PATH_MAX);
        assert_eq!(validate_path(&exact), Status::Ok);
        let too_long_component = core::str::from_utf8(&[b'x'; NAME_CAP + 1]).unwrap();
        assert_eq!(validate_path(too_long_component), Status::Invalid);
        let longest_component = core::str::from_utf8(&[b'x'; NAME_CAP]).unwrap();
        assert_eq!(validate_path(longest_component), Status::Ok);
    }

    #[test]
    fn a_directory_path_is_the_root_or_a_valid_path() {
        assert_eq!(validate_dir(""), Status::Ok);
        assert_eq!(validate_dir("."), Status::Ok);
        for path in ["base", "base/child", "lib/ui"] {
            assert_eq!(validate_dir(path), Status::Ok, "{path}");
        }
        for path in ["..", "/", "base/", "base//child", "../base"] {
            assert_eq!(validate_dir(path), Status::Invalid, "{path}");
        }
        let long = "x".repeat(PATH_MAX + 1);
        assert_eq!(
            validate_dir(&long),
            Status::CapacityExceeded,
            "syntax errors keep their specific status"
        );
    }

    #[test]
    fn limits_are_the_spec_constants() {
        assert_eq!(PATH_MAX, 256);
        assert_eq!(NAME_CAP, 64);
        assert_eq!(MAX_COMPONENTS, 8);
        assert_eq!(READ_CHUNK, 256);
        assert_eq!((KIND_FILE, KIND_DIR, KIND_OTHER), (0, 1, 2));
    }

    #[test]
    fn only_the_home_tree_is_writable() {
        for path in ["home/written.txt", "home/a/b", "home"] {
            assert_eq!(validate_writable(path), Status::Ok, "{path}");
        }
        for path in [
            "written.txt",
            "base/child",
            "homex/file",
            "lib/home/x",
            "homer",
        ] {
            assert_eq!(validate_writable(path), Status::Invalid, "{path}");
        }
        // Syntax errors keep their own status.
        assert_eq!(validate_writable("../home/x"), Status::Invalid);
        assert_eq!(validate_writable(""), Status::Invalid);
        let mut long = [b'x'; PATH_MAX + 1];
        long[..5].copy_from_slice(b"home/");
        let long = core::str::from_utf8(&long).unwrap();
        assert_eq!(validate_writable(long), Status::CapacityExceeded);
    }
}
