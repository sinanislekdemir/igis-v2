//! The application file API's fixed limits and path rules (spec 0013).
//!
//! M2.5.1 exposes the read-only ext2 image to applications through three
//! `core` builtins. The view is the image root only: this module holds the
//! limits shared by the compiler and the interpreter, and the one path-syntax
//! validator the REPL service also uses for `core.spawn`, so a malformed path
//! means the same thing at every call site. There is no writable API and no
//! subdirectory traversal in this slice.

use crate::Status;

/// Longest accepted path in bytes (spec 0005; spec 0013).
pub const PATH_MAX: usize = 256;

/// Longest name a listing can return in bytes (spec 0013).
pub const NAME_CAP: usize = 64;

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

/// Validates a relative path (spec 0005, spec 0013): at most [`PATH_MAX`]
/// bytes, not empty, no leading or trailing `/`, and no empty or `..`
/// component. The bytes must already be valid UTF-8; callers convert first.
pub fn validate_path(path: &str) -> Status {
    if path.len() > PATH_MAX {
        return Status::CapacityExceeded;
    }
    if path.is_empty() || path.starts_with('/') || path.ends_with('/') {
        return Status::Invalid;
    }
    if path.split('/').any(|part| part.is_empty() || part == "..") {
        return Status::Invalid;
    }
    Status::Ok
}

/// Validates the M2.5.1 root-directory path for `core.fs_list`: exactly `""`
/// or `"."`. Every other path addresses a subdirectory the subset does not
/// support, so it is [`Status::Invalid`] (spec 0013).
pub fn validate_root(path: &str) -> Status {
    if path.is_empty() || path == "." {
        Status::Ok
    } else {
        Status::Invalid
    }
}

/// Validates the M2.5.1 root-entry path for `core.fs_read` and
/// `core.fs_size`: one relative component with no `/`. A path with a
/// separator addresses a subdirectory and is [`Status::Invalid`]; the other
/// syntax rules are [`validate_path`]'s.
pub fn validate_root_name(path: &str) -> Status {
    match validate_path(path) {
        Status::Ok if path.contains('/') => Status::Invalid,
        status => status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_syntax_matches_the_spawn_rule() {
        for path in ["child", "a.b", "x_y", ".", ".."] {
            if path == ".." {
                assert_eq!(validate_path(path), Status::Invalid);
            } else {
                assert_eq!(validate_path(path), Status::Ok, "{path}");
            }
        }
        for path in ["", "/child", "child/", "a//b", "../child", "a/../b"] {
            assert_eq!(validate_path(path), Status::Invalid, "{path}");
        }
        assert_eq!(validate_path("a/b"), Status::Ok);
        let long = "x".repeat(PATH_MAX + 1);
        assert_eq!(validate_path(&long), Status::CapacityExceeded);
        let exact = "x".repeat(PATH_MAX);
        assert_eq!(validate_path(&exact), Status::Ok);
    }

    #[test]
    fn the_root_and_root_entry_views_are_the_m251_subset() {
        assert_eq!(validate_root(""), Status::Ok);
        assert_eq!(validate_root("."), Status::Ok);
        for path in ["child", "a/b", "..", "/"] {
            assert_eq!(validate_root(path), Status::Invalid, "{path}");
        }

        assert_eq!(validate_root_name("child"), Status::Ok);
        assert_eq!(validate_root_name("."), Status::Ok);
        for path in ["a/b", "", "..", "/child", "child/"] {
            assert_eq!(validate_root_name(path), Status::Invalid, "{path}");
        }
        let long = "x".repeat(PATH_MAX + 1);
        assert_eq!(
            validate_root_name(&long),
            Status::CapacityExceeded,
            "syntax errors keep their specific status"
        );
    }

    #[test]
    fn limits_are_the_spec_constants() {
        assert_eq!(PATH_MAX, 256);
        assert_eq!(NAME_CAP, 64);
        assert_eq!(READ_CHUNK, 256);
        assert_eq!((KIND_FILE, KIND_DIR, KIND_OTHER), (0, 1, 2));
    }
}
