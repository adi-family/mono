//! Supervises a bundled binary as a per-user background service.
//!
//! - **macOS:** a launchd `LaunchAgent` (`gui/$UID`, `RunAtLoad` + `KeepAlive`), driven through
//!   `launchctl`. Mirrors Swift's `Launchd`.
//! - **Windows:** a per-user **Task Scheduler** task (logon trigger + run-now, restart-on-failure),
//!   driven through `schtasks.exe`. A scheduled task is the unprivileged analog of a per-user
//!   LaunchAgent: it runs as the interactive user with no stored credentials and no admin rights.
//!
//! Both back-ends expose the same surface — [`enable`], [`enable_periodic`], [`disable`],
//! [`is_loaded`], [`kickstart`] — so the [`crate::service`] layer above never learns which OS it
//! is on. The active implementation is re-exported from the per-OS submodule below.

#[cfg(unix)]
pub use macos::*;
#[cfg(windows)]
pub use windows::*;

// ── macOS: launchd via launchctl ────────────────────────────────────────────────────────────
#[cfg(unix)]
mod macos {
    use std::path::PathBuf;
    use std::sync::OnceLock;

    use crate::paths;
    use crate::proc;

    /// The current uid, cached; resolved via `id -u` to avoid an `unsafe` `getuid` call.
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

    /// Install and start the `LaunchAgent`: write the plist, boot out any stale instance (so `bootstrap` can't dupe-fail), bootstrap, then enable.
    pub fn enable(label: &str, program: &[String], log: &str, env: &[(String, String)]) {
        install(label, &plist_xml(label, program, log, env));
    }

    /// Like [`enable`] but for a periodic one-shot job: runs at load and then every
    /// `interval_secs`, with no `KeepAlive` (the job exits between runs).
    pub fn enable_periodic(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
        interval_secs: u32,
    ) {
        install(
            label,
            &plist_xml_periodic(label, program, log, env, interval_secs),
        );
    }

    fn install(label: &str, plist: &str) {
        let dir = paths::launch_agents_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = plist_path(label);
        let _ = std::fs::write(&path, plist);

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

    /// Loaded == the plist exists and `launchctl print` can address the service.
    #[must_use]
    pub fn is_loaded(label: &str) -> bool {
        plist_path(label).exists() && proc::run(&["/bin/launchctl", "print", &target(label)]).ok()
    }

    /// Atomically kill-and-restart a loaded `LaunchAgent` so it picks up a replaced binary
    /// (`kickstart -k` — no bootout/bootstrap race). A no-op if the service isn't loaded.
    pub fn kickstart(label: &str) {
        let _ = proc::run(&["/bin/launchctl", "kickstart", "-k", &target(label)]);
    }

    /// Identical XML for a per-user `LaunchAgent` and a root `LaunchDaemon`; only the install location differs.
    #[must_use]
    pub fn plist_xml(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
    ) -> String {
        render_plist(
            label,
            program,
            log,
            env,
            "    <key>RunAtLoad</key>\n    <true/>\n    <key>KeepAlive</key>\n    <true/>",
        )
    }

    /// Plist for a periodic one-shot job: fires at load and every `interval_secs`; no `KeepAlive`.
    #[must_use]
    pub fn plist_xml_periodic(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
        interval_secs: u32,
    ) -> String {
        let lifecycle = format!(
            "    <key>RunAtLoad</key>\n    <true/>\n    <key>StartInterval</key>\n    <integer>{interval_secs}</integer>"
        );
        render_plist(label, program, log, env, &lifecycle)
    }

    fn render_plist(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
        lifecycle: &str,
    ) -> String {
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
{env_xml}{lifecycle}
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
        fn periodic_plist_swaps_keepalive_for_a_start_interval() {
            let xml = plist_xml_periodic(
                "family.adi.app.updater",
                &["/opt/adi-mono".to_string(), "update".to_string()],
                "/tmp/log",
                &[],
                21600,
            );
            assert!(xml.contains("<key>StartInterval</key>"));
            assert!(xml.contains("<integer>21600</integer>"));
            assert!(xml.contains("<key>RunAtLoad</key>"));
            assert!(!xml.contains("KeepAlive"));
        }

        #[test]
        fn xml_escapes_markup() {
            assert_eq!(xml_escape("a & b < c > d"), "a &amp; b &lt; c &gt; d");
        }
    }
}

// ── Windows: Task Scheduler via schtasks.exe ────────────────────────────────────────────────
#[cfg(windows)]
mod windows {
    use std::path::PathBuf;

    use crate::paths;
    use crate::proc;

    /// Path of the task-definition XML `schtasks /Create /XML` imports for `label`.
    #[must_use]
    pub fn task_xml_path(label: &str) -> PathBuf {
        paths::launch_agents_dir().join(format!("{label}.xml"))
    }

    /// Install and start a long-running, auto-restarting task (the `KeepAlive` analog).
    pub fn enable(label: &str, program: &[String], log: &str, env: &[(String, String)]) {
        install(label, &task_xml(label, program, log, env, None));
    }

    /// Install a periodic task: fires at logon, then repeats every `interval_secs`; the process
    /// exits between runs, so there is no restart-on-failure supervision (matches launchd's
    /// `StartInterval` job with no `KeepAlive`).
    pub fn enable_periodic(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
        interval_secs: u32,
    ) {
        install(label, &task_xml(label, program, log, env, Some(interval_secs)));
    }

    fn install(label: &str, xml: &str) {
        let dir = paths::launch_agents_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = task_xml_path(label);
        // Task Scheduler is happiest with UTF-16LE; write a BOM + UTF-16 so `schtasks /XML`
        // parses non-ASCII (paths, env values) correctly on any locale.
        let _ = std::fs::write(&path, utf16le_with_bom(xml));

        // Replace any stale registration, then start it now (the logon trigger covers future
        // sessions; `/Run` is the `RunAtLoad`-now analog).
        let create = proc::run(&[
            "schtasks",
            "/Create",
            "/TN",
            label,
            "/XML",
            &path.to_string_lossy(),
            "/F",
        ]);
        if !create.ok() {
            eprintln!(
                "adi: schtasks /Create {label} failed ({}): {}",
                create.status, create.text
            );
        }
        let _ = proc::run(&["schtasks", "/Run", "/TN", label]);
    }

    /// Stop and unregister the task.
    pub fn disable(label: &str) {
        let _ = proc::run(&["schtasks", "/End", "/TN", label]);
        let _ = proc::run(&["schtasks", "/Delete", "/TN", label, "/F"]);
        let _ = std::fs::remove_file(task_xml_path(label));
    }

    /// Loaded == the task is registered (`schtasks /Query` addresses it).
    #[must_use]
    pub fn is_loaded(label: &str) -> bool {
        proc::run(&["schtasks", "/Query", "/TN", label]).ok()
    }

    /// Stop-and-restart the task so it picks up a replaced binary. A no-op if not registered.
    pub fn kickstart(label: &str) {
        if !is_loaded(label) {
            return;
        }
        let _ = proc::run(&["schtasks", "/End", "/TN", label]);
        let _ = proc::run(&["schtasks", "/Run", "/TN", label]);
    }

    /// Build a Task Scheduler 1.2 task definition. `repeat_secs = None` ⇒ a long-running service
    /// (logon trigger + restart-on-failure); `Some(n)` ⇒ a job that repeats every `n` seconds.
    #[must_use]
    fn task_xml(
        label: &str,
        program: &[String],
        log: &str,
        env: &[(String, String)],
        repeat_secs: Option<u32>,
    ) -> String {
        let (command, arguments) = split_program(program);
        // Env vars have no first-class slot in a task action, so thread them through a `cmd /C
        // set VAR=.. && "prog" args` wrapper, redirecting stdout+stderr to the log (the launchd
        // StandardOut/ErrPath analog). Everything is quoted/escaped for both `cmd` and XML.
        let mut inner = String::new();
        for (k, v) in env {
            inner.push_str(&format!("set \"{}={}\" && ", cmd_escape(k), cmd_escape(v)));
        }
        inner.push_str(&quote_cmd(&command));
        if !arguments.is_empty() {
            inner.push(' ');
            inner.push_str(&arguments);
        }
        inner.push_str(&format!(" > {} 2>&1", quote_cmd(log)));
        let comspec_args = format!("/C {}", inner);

        let repetition = repeat_secs.map_or(String::new(), |secs| {
            format!(
                "\n      <Repetition>\n        <Interval>{}</Interval>\n        <StopAtDurationEnd>false</StopAtDurationEnd>\n      </Repetition>",
                iso8601_duration(secs)
            )
        });
        // A long-running service restarts on failure; a periodic job does not (it is meant to exit).
        let restart = if repeat_secs.is_none() {
            "\n    <RestartOnFailure>\n      <Interval>PT1M</Interval>\n      <Count>999</Count>\n    </RestartOnFailure>"
        } else {
            ""
        };

        format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>ADI service {desc}</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>{repetition}
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>{restart}
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{comspec}</Command>
      <Arguments>{args}</Arguments>
    </Exec>
  </Actions>
</Task>"#,
            desc = xml_escape(label),
            comspec = xml_escape("cmd.exe"),
            args = xml_escape(&comspec_args),
        )
    }

    /// Split `[program, arg, ...]` into the command and a single quoted arguments string.
    fn split_program(program: &[String]) -> (String, String) {
        match program.split_first() {
            Some((cmd, rest)) => (cmd.clone(), join_args(rest)),
            None => (String::new(), String::new()),
        }
    }

    fn join_args(args: &[String]) -> String {
        args.iter()
            .map(|a| quote_cmd(a))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Quote an argument for a `cmd /C` command line: wrap in double quotes if it contains a
    /// space or is empty, doubling any embedded quote.
    fn quote_cmd(s: &str) -> String {
        if s.is_empty() || s.contains([' ', '\t']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    }

    /// Escape a value destined for `cmd`'s `set VAR=value` (the risky metacharacters inside a
    /// double-quoted `set`).
    fn cmd_escape(s: &str) -> String {
        s.replace('%', "%%").replace('"', "\"\"")
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    /// Seconds → an ISO-8601 duration (`PT6H`, `PT90S`, …) for a task `<Interval>`.
    fn iso8601_duration(mut secs: u32) -> String {
        let h = secs / 3600;
        secs %= 3600;
        let m = secs / 60;
        let s = secs % 60;
        let mut out = String::from("PT");
        if h > 0 {
            out.push_str(&format!("{h}H"));
        }
        if m > 0 {
            out.push_str(&format!("{m}M"));
        }
        if s > 0 || (h == 0 && m == 0) {
            out.push_str(&format!("{s}S"));
        }
        out
    }

    fn utf16le_with_bom(s: &str) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in s.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn service_task_has_logon_trigger_and_restart() {
            let xml = task_xml(
                "family.adi.app.dns",
                &["C:\\adi\\adi-dns.exe".to_string(), "C:\\cfg.toml".to_string()],
                "C:\\log.txt",
                &[("RUST_LOG".to_string(), "info".to_string())],
                None,
            );
            assert!(xml.contains("<LogonTrigger>"));
            assert!(xml.contains("<RestartOnFailure>"));
            assert!(xml.contains("family.adi.app.dns"));
            // Program, its arg, the log redirect, and the env var all ride in the cmd wrapper.
            assert!(xml.contains("adi-dns.exe"));
            assert!(xml.contains("set &quot;RUST_LOG=info&quot;"));
            assert!(xml.contains("2&gt;&amp;1"));
        }

        #[test]
        fn periodic_task_repeats_and_does_not_restart() {
            let xml = task_xml(
                "family.adi.app.updater",
                &["C:\\adi\\adi-mono.exe".to_string(), "update".to_string()],
                "C:\\log.txt",
                &[],
                Some(21600),
            );
            assert!(xml.contains("<Repetition>"));
            assert!(xml.contains("<Interval>PT6H</Interval>"));
            assert!(!xml.contains("RestartOnFailure"));
        }

        #[test]
        fn iso8601_formats_durations() {
            assert_eq!(iso8601_duration(21600), "PT6H");
            assert_eq!(iso8601_duration(90), "PT1M30S");
            assert_eq!(iso8601_duration(45), "PT45S");
            assert_eq!(iso8601_duration(0), "PT0S");
        }

        #[test]
        fn utf16_output_starts_with_bom() {
            let bytes = utf16le_with_bom("A");
            assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
            assert_eq!(&bytes[2..], &[0x41, 0x00]);
        }
    }
}
