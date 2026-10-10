//! Gives the Windows CLI enough main-thread stack for the derived clap tree.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Debug clap builders keep large Command temporaries on the stack. The
        // Windows default of 1 MiB is too small; reserve the same 8 MiB commonly
        // available to the main thread on Unix. Pages are committed on demand.
        match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
            Ok("msvc") => println!("cargo::rustc-link-arg-bin=gwi=/STACK:8388608"),
            Ok("gnu") => println!("cargo::rustc-link-arg-bin=gwi=-Wl,--stack,8388608"),
            _ => {}
        }
    }
}
