//! Application directory overrides, read before platform directory discovery.
//!
//! `GWI_HOME` selects the home for settings and import discovery; `GWI_STATE_DIR`
//! selects the state base shared by logs, leases, backups and imported ledgers.
//! These raw process overrides cannot be read from settings (circular discovery).

use std::ffi::OsString;
use std::path::PathBuf;

/// Application home, defaulting to the platform home directory.
pub(crate) fn home_dir() -> Option<PathBuf> {
    resolve(std::env::var_os("GWI_HOME"), dirs::home_dir)
}

/// Application state base, defaulting to the platform state/data directory.
pub(crate) fn state_dir() -> Option<PathBuf> {
    resolve(std::env::var_os("GWI_STATE_DIR"), || {
        dirs::state_dir().or_else(dirs::data_dir)
    })
}

/// Resolve native path values without invoking platform discovery for an override.
fn resolve(value: Option<OsString>, fallback: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
    value
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_path_bypasses_platform_discovery() {
        let path = std::env::temp_dir().join("application home with spaces");
        assert_eq!(
            resolve(Some(path.clone().into_os_string()), || panic!(
                "fallback called"
            )),
            Some(path)
        );
    }

    #[test]
    fn unset_and_empty_paths_keep_platform_defaults() {
        let path = std::env::temp_dir().join("platform");
        for value in [None, Some(OsString::new())] {
            assert_eq!(resolve(value, || Some(path.clone())), Some(path.clone()));
        }
        assert_eq!(resolve(None, || None), None);
    }

    #[cfg(unix)]
    #[test]
    fn native_non_unicode_paths_are_preserved() {
        use std::os::unix::ffi::OsStringExt;
        let path = OsString::from_vec(b"/tmp/non-unicode-\xff".to_vec());
        assert_eq!(
            resolve(Some(path.clone()), || None),
            Some(PathBuf::from(path))
        );
    }
}
