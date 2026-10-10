//! Shared helpers: secrets, settings, environment access, HTTP, filesystem safety.

pub(crate) mod app_dirs;
pub mod browser_command;
pub(crate) mod browser_launch;
pub mod config_dir;
pub mod duration;
pub mod env;
pub mod fs;
pub mod http;
pub mod multipart;
pub mod path;
pub mod rate_limit;
pub mod secret;
pub mod secret_env;
pub mod settings;
pub mod terminal;
pub mod token;
