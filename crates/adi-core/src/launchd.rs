//! Supervises a bundled binary as a per-user launchd `LaunchAgent` (`gui/$UID`,
//! `RunAtLoad` + `KeepAlive`). The one place that talks to `launchctl`, mirroring
//! Swift's `Launchd`.

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::paths;
use crate::proc;

/// The current uid, cached for the life of the process. Resolved via `id -u` so we
/// avoid an `unsafe` `getuid` call; a short-lived CLI resolves it at most once.
fn uid() -> u32 {
    static UID: OnceLock<u32> = OnceLock::new();
    *UID.get_or_init(|| {
        proc::run(&["/usr/bin/id", "-u"])
            .text
            .trim()
            .parse()
            .unwrap_or(0)
    })
}

#[must_use]
pub fn gui_domain() -> String {
    format!("gui/{}", uid())
}

/// The `gui/$UID/<label>` service target `launchctl` addresses.
#[must_use]
pub fn target(label: &str) -> String {
    format!("{}/{label}", gui_domain())
}

#[must_use]
pub fn plist_path(label: &str) -> PathBuf {
    paths::launch_agents_dir().join(format!("{label}.plist"))
}

/// Install and start the `LaunchAgent`: write the plist, boot out any stale instance
/// (so `bootstrap` can't fail on a dupe), bootstrap, then enable.
pub fn enable(label: &str, program: &[String], log: &str, env: &[(String, String)]) {
    let dir = paths::launch_agents_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = plist_path(label);
    let _ = std::fs::write(&path, plist_xml(label, program, log, env));

    let target = target(label);
    let _ = proc::run(&["/bin/launchctl", "bootout", &target]);
    let boot = proc::run(&[
        "/bin/launchctl",
        "bootstrap",
        &gui_domain(),
        &path.to_string_lossy(),
    ]);
    if !boot.ok() {
        eprintln!(
            "adi: launchctl bootstrap {label} failed ({}): {}",
            boot.status, boot.text
        );
    }
    let _ = proc::run(&["/bin/launchctl", "enable", &target]);
}

/// Stop and uninstall the `LaunchAgent`.
pub fn disable(label: &str) {
    let _ = proc::run(&["/bin/launchctl", "bootout", &target(label)]);
    let _ = std::fs::remove_file(plist_path(label));
}

/// Loaded == the plist exists and `launchctl print` can address the service (i.e. the
/// `LaunchAgent` is bootstrapped).
#[must_use]
pub fn is_loaded(label: &str) -> bool {
    plist_path(label).exists() && proc::run(&["/bin/launchctl", "print", &target(label)]).ok()
}

/// Identical XML for a per-user `LaunchAgent` and a root `LaunchDaemon` — only the
/// install location differs — so the privileged landing daemon reuses this.
#[must_use]
pub fn plist_xml(label: &str, program: &[String], log: &str, env: &[(String, String)]) -> String {
    let args_xml = program
        .iter()
        .map(|a| format!("        <string>{}</string>", xml_escape(a)))
        .collect::<Vec<_>>()
        .join("\n");
    let env_xml = if env.is_empty() {
        String::new()
    } else {
        let entries = env
            .iter()
            .map(|(k, v)| {
                format!(
                    "        <key>{}</key><string>{}</string>",
                    xml_escape(k),
                    xml_escape(v)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("    <key>EnvironmentVariables</key>\n    <dict>\n{entries}\n    </dict>\n")
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
{args_xml}
    </array>
{env_xml}    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>"#,
        label = xml_escape(label),
        log = xml_escape(log),
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_contains_label_and_program() {
        let xml = plist_xml(
            "family.adi.app.dns",
            &["/opt/adi-dns".to_string(), "/cfg.toml".to_string()],
            "/tmp/log",
            &[("RUST_LOG".to_string(), "info".to_string())],
        );
        assert!(xml.contains("<string>family.adi.app.dns</string>"));
        assert!(xml.contains("<string>/opt/adi-dns</string>"));
        assert!(xml.contains("<string>/cfg.toml</string>"));
        assert!(xml.contains("<key>RUST_LOG</key><string>info</string>"));
        assert!(xml.contains("<key>KeepAlive</key>"));
    }

    #[test]
    fn plist_omits_env_dict_when_empty() {
        let xml = plist_xml("l", &["/bin/x".to_string()], "/tmp/log", &[]);
        assert!(!xml.contains("EnvironmentVariables"));
    }

    #[test]
    fn xml_escapes_markup() {
        assert_eq!(xml_escape("a & b < c > d"), "a &amp; b &lt; c &gt; d");
    }
}
