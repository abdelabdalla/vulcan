use anyhow::Result;
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};

pub const SERVICE_TYPE: &str = "_vulcan._tcp.local.";

/// Advertise vulcand so the phone can find it after the PC's IP changes.
/// The TXT record carries the cert fingerprint, which is how the phone recognises this PC.
pub fn advertise(hostname: &str, port: u16, fingerprint: &str) -> Result<ServiceDaemon> {
    let mdns = ServiceDaemon::new()?;
    // The server listens on IPv4 only.
    mdns.disable_interface(IfKind::IPv6)?;
    let host = format!("{hostname}.local.");
    let props = [("fp", fingerprint), ("v", "1")];
    let info = ServiceInfo::new(SERVICE_TYPE, hostname, &host, "", port, &props[..])?.enable_addr_auto();
    mdns.register(info)?;
    Ok(mdns)
}
