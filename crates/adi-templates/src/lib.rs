//! adi-templates — the on-disk contract of `~/.adi/mono/templates/<id>/template.toml`: what a
//! dashboard template says to run to create one, and how that command actually runs.
//!
//! A template is a directory, `~/.adi/mono/templates/<id>/`, carrying one file,
//! `template.toml`, naming the shell command that creates a dashboard of this kind. Deleting the
//! folder removes the template; the marketplace installs one by landing such a folder
//! ([`adi-marketplace`]'s `Kind::Template`). The directory may also keep its own files under
//! `<id>/files/`, reachable to the command through `$ADI_TEMPLATE_DIR`.
//!
//! Explicit, no magic: the `command` a template.toml carries is exactly what runs, `bash -c
//! '<command>'`. Nothing here substitutes a placeholder into it — a template that needs this
//! dashboard's id or host reads `$ADI_DASHBOARD_ID` / `$ADI_DASHBOARD_HOST` itself, the same way
//! every other environment variable here is read, not rewritten in.
//!
//! This crate owns the shape of `template.toml` and the rule for running one. It does not decide
//! which template a request named, what a dashboard directory is, or what happens to it on
//! failure — that is the caller's.

mod run;
mod toml_file;

pub use run::{Env, RunError, RunOutcome, create};
pub use toml_file::{DEFAULT_TEMPLATE, Template, list_templates, read_template, seed_defaults};
