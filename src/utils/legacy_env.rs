//! Warning for `OMNI_DEV_*` variables that gwi no longer reads.
//!
//! gwi reads only `GWI_*` names and deliberately has no fallback to the `OMNI_DEV_*` names
//! omni-dev used (ADR-0001, decision 5): a fallback would reintroduce the coupling the split
//! removed. `gwi import` renames the names it finds inside `~/.omni-dev/settings.json`, but a
//! variable exported from a shell profile or an MCP client's launcher config lives in the
//! process environment, where nothing reported it. A user who exported
//! `OMNI_DEV_DRIVE_LEASE_BIOMETRICS_ONLY=1` would get device-owner authentication instead,
//! with no message.
//!
//! This module only **reports** such variables. It never honours an old name, and it never
//! formats a value: the warning names the old variable and its replacement, nothing else, so
//! a path or an account name that happens to be in the old variable cannot leak into
//! the output.

use crate::utils::env::{EnvSource, SystemEnv};

/// Whether a legacy variable set an ordinary option or a Drive lease policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// An ordinary option: the default applies instead.
    Option,
    /// A lease policy: ignoring it silently weakens a safety setting, so the
    /// warning says so.
    LeasePolicy,
}

/// The `OMNI_DEV_*` names the moved code read, with each one's kind. Its gwi replacement is
/// `GWI_` plus the same suffix.
const LEGACY_ENV: &[(&str, Kind)] = &[
    ("OMNI_DEV_GMAIL_ACCOUNT", Kind::Option),
    ("OMNI_DEV_DRIVE_ACCOUNT", Kind::Option),
    ("OMNI_DEV_PROFILE", Kind::Option),
    ("OMNI_DEV_CONFIG_DIR", Kind::Option),
    ("OMNI_DEV_LOG_FILE", Kind::Option),
    ("OMNI_DEV_LOG_DISABLE", Kind::Option),
    ("OMNI_DEV_DRIVE_LEASE_EXPIRY_MINUTES", Kind::LeasePolicy),
    ("OMNI_DEV_DRIVE_LEASE_BACKUP_DIR", Kind::LeasePolicy),
    ("OMNI_DEV_DRIVE_LEASE_BIOMETRICS_ONLY", Kind::LeasePolicy),
    ("OMNI_DEV_DRIVE_LEASE_ALLOW_HEADLESS", Kind::LeasePolicy),
    ("OMNI_DEV_LEASE_LOCK_WAIT_SECS", Kind::LeasePolicy),
];

/// The prefix omni-dev used and the one gwi uses in its place.
const OLD_PREFIX: &str = "OMNI_DEV_";
const NEW_PREFIX: &str = "GWI_";

/// Returns the replacement for a legacy variable name.
fn replacement(legacy: &str) -> String {
    format!("{NEW_PREFIX}{}", &legacy[OLD_PREFIX.len()..])
}

/// Builds one warning line for each legacy variable set in `env`, in table order.
///
/// A variable counts as set when it is non-empty, matching how gwi treats its own variables
/// (`VAR=` neutralises an export for one invocation). The value is looked at only to tell
/// empty from non-empty; it is neither kept, formatted nor acted on.
pub(crate) fn legacy_env_warnings(env: &impl EnvSource) -> Vec<String> {
    LEGACY_ENV
        .iter()
        .filter(|(name, _)| env.var(name).is_some_and(|value| !value.is_empty()))
        .map(|(name, kind)| {
            let new = replacement(name);
            match kind {
                Kind::Option => {
                    format!("warning: {name} is set but gwi ignores it; set {new} instead")
                }
                Kind::LeasePolicy => format!(
                    "warning: {name} is set but gwi ignores it, so the Drive lease policy it \
                     configured is NOT in effect; set {new} instead"
                ),
            }
        })
        .collect()
}

/// Prints [`legacy_env_warnings`] for the process environment to stderr, once per call.
///
/// Written with `eprintln!` rather than `tracing`, so `RUST_LOG=error` cannot hide it.
pub fn warn_legacy_env() {
    for line in legacy_env_warnings(&SystemEnv) {
        eprintln!("{line}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::test_support::env::MapEnv;

    #[test]
    fn a_clean_environment_warns_about_nothing() {
        assert!(legacy_env_warnings(&MapEnv::new()).is_empty());
    }

    #[test]
    fn every_legacy_name_has_a_gwi_replacement_with_the_same_suffix() {
        assert_eq!(LEGACY_ENV.len(), 11);
        for (name, _) in LEGACY_ENV {
            assert!(name.starts_with("OMNI_DEV_"), "{name}");
            assert_eq!(replacement(name), name.replacen("OMNI_DEV_", "GWI_", 1));
        }
    }

    #[test]
    fn each_legacy_name_warns_and_names_its_replacement() {
        for (name, _) in LEGACY_ENV {
            let env = MapEnv::new().with(name, "value");
            let lines = legacy_env_warnings(&env);
            assert_eq!(lines.len(), 1, "{name}: {lines:?}");
            let line = &lines[0];
            assert!(line.starts_with("warning: "), "{line}");
            assert!(line.contains(name), "{line}");
            assert!(line.contains(&replacement(name)), "{line}");
            assert!(!line.contains('\n'), "{line}");
        }
    }

    #[test]
    fn the_old_value_never_appears_in_the_output() {
        let mut env = MapEnv::new();
        for (name, _) in LEGACY_ENV {
            env = env.with(name, "SENTINEL-value-1234");
        }
        let joined = legacy_env_warnings(&env).join("\n");
        assert!(!joined.contains("SENTINEL"), "{joined}");
    }

    #[test]
    fn an_empty_value_counts_as_unset() {
        let env = MapEnv::new().with("OMNI_DEV_DRIVE_LEASE_BIOMETRICS_ONLY", "");
        assert!(legacy_env_warnings(&env).is_empty());
    }

    #[test]
    fn gwi_names_alone_never_warn() {
        let env = MapEnv::new()
            .with("GWI_DRIVE_LEASE_BIOMETRICS_ONLY", "1")
            .with("GWI_GMAIL_ACCOUNT", "work")
            .with("GWI_PROFILE", "home");
        assert!(legacy_env_warnings(&env).is_empty());
    }

    #[test]
    fn a_legacy_name_warns_even_when_its_replacement_is_also_set() {
        // gwi uses the GWI_ value, so the old one is dead weight, but it is still worth removing.
        let env = MapEnv::new()
            .with("OMNI_DEV_PROFILE", "old")
            .with("GWI_PROFILE", "new");
        assert_eq!(legacy_env_warnings(&env).len(), 1);
    }

    #[test]
    fn several_set_variables_give_one_line_each_in_table_order() {
        let env = MapEnv::new()
            .with("OMNI_DEV_LEASE_LOCK_WAIT_SECS", "5")
            .with("OMNI_DEV_PROFILE", "work")
            .with("OMNI_DEV_GMAIL_ACCOUNT", "me");
        let lines = legacy_env_warnings(&env);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].contains("OMNI_DEV_GMAIL_ACCOUNT"), "{lines:?}");
        assert!(lines[1].contains("OMNI_DEV_PROFILE"), "{lines:?}");
        assert!(
            lines[2].contains("OMNI_DEV_LEASE_LOCK_WAIT_SECS"),
            "{lines:?}"
        );
    }

    #[test]
    fn lease_policy_variables_warn_louder_than_the_rest() {
        for (name, kind) in LEGACY_ENV {
            let line = legacy_env_warnings(&MapEnv::new().with(name, "1")).remove(0);
            assert_eq!(
                line.contains("NOT in effect"),
                *kind == Kind::LeasePolicy,
                "{line}"
            );
        }
        assert!(LEGACY_ENV.iter().any(|(name, kind)| *name
            == "OMNI_DEV_DRIVE_LEASE_BIOMETRICS_ONLY"
            && *kind == Kind::LeasePolicy));
    }
}
