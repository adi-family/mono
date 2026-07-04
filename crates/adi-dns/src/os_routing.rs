//! Self-registration of the OS "route `*.{domain}` to this resolver" integration.
//!
//! Same concept everywhere, different mechanism per OS:
//!   * macOS   → `/etc/resolver/<domain>`      (mDNSResponder scoped resolver; supports a port)
//!   * Linux   → systemd-resolved drop-in       (`Domains=~<domain>` split DNS; supports `ip:port`)
//!   * Windows → NRPT rule for `.<domain>`       (port-less — the resolver must be on `:53`)
//!
//! Only `.<domain>` is ever routed; the default resolver and every other name are
//! left untouched, so this never disturbs another local resolver (e.g. ADI DNS on
//! `.test`). The string-building helpers below are always compiled and unit-tested;
//! only the filesystem / command side effects are gated per target.

use std::net::SocketAddr;

/// Route `*.{domain}` to the resolver bound at `addr`. Needs admin/root.
pub fn install(domain: &str, addr: SocketAddr) -> anyhow::Result<()> {
    platform::install(domain, addr)
}

/// Remove a route previously installed by [`install`].
pub fn uninstall(domain: &str) -> anyhow::Result<()> {
    platform::uninstall(domain)
}

/// A copy-pasteable manual command, shown when auto-install can't run.
pub fn describe_manual(domain: &str, addr: SocketAddr) -> String {
    platform::describe_manual(domain, addr)
}

// --- OS-independent content builders (always compiled + tested) --------------

/// Contents of a macOS `/etc/resolver/<domain>` file pointing at `addr`.
#[cfg(any(target_os = "macos", test))]
fn macos_resolver_contents(addr: SocketAddr) -> String {
    let ip = addr.ip();
    // The `port` directive is only needed when the resolver isn't on 53.
    if addr.port() == 53 {
        format!("nameserver {ip}\n")
    } else {
        format!("nameserver {ip}\nport {}\n", addr.port())
    }
}

/// Contents of a Linux systemd-resolved drop-in for split-DNS on `domain`.
#[cfg(any(target_os = "linux", test))]
fn linux_resolved_contents(domain: &str, addr: SocketAddr) -> String {
    let dns = if addr.port() == 53 {
        addr.ip().to_string()
    } else {
        format!("{}:{}", addr.ip(), addr.port())
    };
    format!(
        "# Managed by adi-dns. Split-DNS: route only .{domain} to this resolver.\n\
         [Resolve]\n\
         DNS={dns}\n\
         Domains=~{domain}\n"
    )
}

/// The NRPT namespace string for a domain on Windows (`adi` → `.adi`).
#[cfg(any(target_os = "windows", test))]
fn windows_namespace(domain: &str) -> String {
    format!(".{domain}")
}

// --- macOS -------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod platform {
    use std::fs;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::process::Command;

    use anyhow::Context;

    fn resolver_path(domain: &str) -> PathBuf {
        PathBuf::from("/etc/resolver").join(domain)
    }

    pub fn install(domain: &str, addr: SocketAddr) -> anyhow::Result<()> {
        fs::create_dir_all("/etc/resolver").context("creating /etc/resolver")?;
        let path = resolver_path(domain);
        fs::write(&path, super::macos_resolver_contents(addr))
            .with_context(|| format!("writing {}", path.display()))?;
        flush_cache();
        Ok(())
    }

    pub fn uninstall(domain: &str) -> anyhow::Result<()> {
        let path = resolver_path(domain);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("removing {}", path.display())))
            }
        }
        flush_cache();
        Ok(())
    }

    pub fn describe_manual(domain: &str, addr: SocketAddr) -> String {
        format!(
            "sudo mkdir -p /etc/resolver && printf '{}' | sudo tee /etc/resolver/{domain} \
             && sudo dscacheutil -flushcache && sudo killall -HUP mDNSResponder",
            super::macos_resolver_contents(addr).replace('\n', "\\n"),
        )
    }

    fn flush_cache() {
        let _ = Command::new("dscacheutil").arg("-flushcache").status();
        let _ = Command::new("killall").args(["-HUP", "mDNSResponder"]).status();
    }
}

// --- Linux -------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod platform {
    use std::fs;
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::process::Command;

    use anyhow::Context;

    fn drop_in_path(domain: &str) -> PathBuf {
        PathBuf::from("/etc/systemd/resolved.conf.d").join(format!("adi-dns-{domain}.conf"))
    }

    pub fn install(domain: &str, addr: SocketAddr) -> anyhow::Result<()> {
        fs::create_dir_all("/etc/systemd/resolved.conf.d")
            .context("creating /etc/systemd/resolved.conf.d")?;
        let path = drop_in_path(domain);
        fs::write(&path, super::linux_resolved_contents(domain, addr))
            .with_context(|| format!("writing {}", path.display()))?;
        restart_resolved()
    }

    pub fn uninstall(domain: &str) -> anyhow::Result<()> {
        let path = drop_in_path(domain);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("removing {}", path.display())))
            }
        }
        restart_resolved()
    }

    pub fn describe_manual(domain: &str, addr: SocketAddr) -> String {
        format!(
            "sudo mkdir -p /etc/systemd/resolved.conf.d && printf '{}' | \
             sudo tee /etc/systemd/resolved.conf.d/adi-dns-{domain}.conf \
             && sudo systemctl restart systemd-resolved",
            super::linux_resolved_contents(domain, addr).replace('\n', "\\n"),
        )
    }

    fn restart_resolved() -> anyhow::Result<()> {
        let status = Command::new("systemctl")
            .args(["restart", "systemd-resolved"])
            .status()
            .context("running systemctl restart systemd-resolved")?;
        anyhow::ensure!(status.success(), "systemctl restart systemd-resolved failed");
        Ok(())
    }
}

// --- Windows -----------------------------------------------------------------

#[cfg(target_os = "windows")]
mod platform {
    use std::net::SocketAddr;
    use std::process::Command;

    use anyhow::Context;

    pub fn install(domain: &str, addr: SocketAddr) -> anyhow::Result<()> {
        anyhow::ensure!(
            addr.port() == 53,
            "Windows NRPT cannot target port {}; adi-dns must bind :53 on Windows",
            addr.port()
        );
        let ns = super::windows_namespace(domain);
        // Replace any existing rule for this namespace, then add ours.
        let script = format!(
            "Get-DnsClientNrptRule | Where-Object {{ $_.Namespace -contains '{ns}' }} | \
             ForEach-Object {{ Remove-DnsClientNrptRule -Name $_.Name -Force }}; \
             Add-DnsClientNrptRule -Namespace '{ns}' -NameServers '127.0.0.1'; \
             Clear-DnsClientCache"
        );
        run_powershell(&script)
    }

    pub fn uninstall(domain: &str) -> anyhow::Result<()> {
        let ns = super::windows_namespace(domain);
        let script = format!(
            "Get-DnsClientNrptRule | Where-Object {{ $_.Namespace -contains '{ns}' }} | \
             ForEach-Object {{ Remove-DnsClientNrptRule -Name $_.Name -Force }}; \
             Clear-DnsClientCache"
        );
        run_powershell(&script)
    }

    pub fn describe_manual(domain: &str, _addr: SocketAddr) -> String {
        let ns = super::windows_namespace(domain);
        format!("Add-DnsClientNrptRule -Namespace '{ns}' -NameServers '127.0.0.1'  (elevated PowerShell)")
    }

    fn run_powershell(script: &str) -> anyhow::Result<()> {
        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .status()
            .context("running powershell")?;
        anyhow::ensure!(status.success(), "powershell NRPT command failed");
        Ok(())
    }
}

// --- Other platforms ---------------------------------------------------------

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod platform {
    use std::net::SocketAddr;

    pub fn install(_domain: &str, _addr: SocketAddr) -> anyhow::Result<()> {
        anyhow::bail!("automatic OS DNS routing is not supported on this platform")
    }

    pub fn uninstall(_domain: &str) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn describe_manual(_domain: &str, _addr: SocketAddr) -> String {
        "automatic OS routing unsupported on this platform".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{linux_resolved_contents, macos_resolver_contents, windows_namespace};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    #[test]
    fn macos_omits_port_line_for_53() {
        assert_eq!(macos_resolver_contents(addr(53)), "nameserver 127.0.0.1\n");
    }

    #[test]
    fn macos_includes_high_port() {
        assert_eq!(
            macos_resolver_contents(addr(10053)),
            "nameserver 127.0.0.1\nport 10053\n"
        );
    }

    #[test]
    fn linux_encodes_port_and_routing_domain() {
        let c = linux_resolved_contents("adi", addr(10053));
        assert!(c.contains("DNS=127.0.0.1:10053"), "got: {c}");
        assert!(c.contains("Domains=~adi"), "got: {c}");
    }

    #[test]
    fn linux_omits_port_for_53() {
        let c = linux_resolved_contents("adi", addr(53));
        assert!(c.contains("DNS=127.0.0.1\n"), "got: {c}");
    }

    #[test]
    fn windows_namespace_has_leading_dot() {
        assert_eq!(windows_namespace("adi"), ".adi");
    }
}
