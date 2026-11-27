# netwatch dependency feature patch

Source: crates.io `netwatch` 0.19.3, upstream repository
https://github.com/n0-computer/net-tools at commit
`0030e57fc70981895bb86212ff0f059cc41c8a4e` (the `netwatch/` directory).
`.cargo_vcs_info.json` preserves the published crate's provenance.

Only functional change: the normalized `Cargo.toml` sets
`default-features = false` for the Windows-only `wmi` dependency.
`Cargo.toml.orig` is retained unchanged as the upstream manifest.

The Windows implementation queries only `Win32_IP4RouteTable.Name`, a string.
It uses no `WMIDateTime` values and needs no Chrono date parsing. Disabling that
optional feature removes the otherwise unused, unmaintained Chrono dependency
(RUSTSEC-2026-0543) without changing network monitoring or route discovery.
No Rust source files are modified.

Remove this patch when a compatible upstream netwatch release disables WMI's
default Chrono feature or otherwise removes the dependency. Keep the Windows
networking build and the existing native/browser mesh checks passing when updating.

The upstream code remains licensed under MIT OR Apache-2.0; both license texts
are preserved alongside it. The registry's generated Cargo.lock and cache marker
are omitted because this package is resolved by the root Cargo.lock.
