//! `BUILT_VERSION` reads `ADI_VERSION` through `option_env!`, which cargo resolves at
//! compile time and then caches — without this hint a rebuild after `git tag` would keep
//! the stale number baked in, and the binary would disagree with the bundle it shipped in.
fn main() {
    println!("cargo:rerun-if-env-changed=ADI_VERSION");
}
