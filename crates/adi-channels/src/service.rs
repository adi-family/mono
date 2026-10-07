//! Stable address of the local channels service. Hive allocates its listening port and
//! routes this internal domain, so clients never discover or persist the daemon's port.
//!
//! Local API callers connect through the existing per-user Hive listener and select the
//! service with the HTTP Host header. This also works on mesh-only Linux installations,
//! where the operator has not installed local domain resolution or a port-80 front door.

/// The service name Hive routes for this installation, including `ADI_DOMAIN` overrides.
#[must_use]
pub fn host() -> String {
    format!("channels.{}", adi_config::Flavor::current().domain)
}

/// The channels daemon's address for this installation, including `ADI_DOMAIN` overrides.
#[must_use]
pub fn url() -> String {
    format!("http://{}", host())
}

/// The existing Hive supervisor's HTTP listener. Send [`host`] as the Host header when
/// connecting here; Hive performs the only lookup of the channels daemon's allocated port.
#[must_use]
pub fn transport_url() -> String {
    format!(
        "http://127.0.0.1:{}",
        adi_config::Flavor::current().supervisor_port
    )
}
