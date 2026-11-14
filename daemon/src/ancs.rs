//! iPhone notifications on the desktop via Apple Notification Center Service (ANCS) over
//! Bluetooth LE, the protocol smartwatches use. No iOS app involvement.
//!
//! The PC advertises as a peripheral that solicits ANCS. The iPhone connects (and bonds)
//! from Settings → Bluetooth; the PC then acts as a GATT client of the phone's ANCS service:
//! Notification Source says what happened, the Control Point asks for details, and the
//! Data Source returns them. Pairing is only accepted during a short window opened from the tray.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bluer::adv::Advertisement;
use bluer::agent::{Agent, ReqError};
use bluer::gatt::remote::Characteristic;
use bluer::{Address, Device, Uuid};
use futures_util::{Stream, StreamExt};
use tracing::{debug, info, warn};

pub const ANCS_SERVICE: Uuid = Uuid::from_u128(0x7905F431_B5CE_4E99_A40F_4B1E122D00D0);
const NOTIFICATION_SOURCE: Uuid = Uuid::from_u128(0x9FBF120D_6301_42D9_8C58_25E699A21DBD);
const CONTROL_POINT: Uuid = Uuid::from_u128(0x69D1D8F3_45E1_49A8_9821_9BBDFDAAD9D9);
const DATA_SOURCE: Uuid = Uuid::from_u128(0x22EAC6E9_24D6_4BB5_BE44_B36ACE7C7BFB);

const PAIRING_WINDOW: Duration = Duration::from_secs(120);

// MARK: Protocol

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Added,
    Modified,
    Removed,
}

pub const FLAG_SILENT: u8 = 1 << 0;
pub const FLAG_PRE_EXISTING: u8 = 1 << 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceEvent {
    pub kind: EventKind,
    pub flags: u8,
    pub category: u8,
    pub uid: u32,
}

/// An 8-byte Notification Source packet.
pub fn parse_source(b: &[u8]) -> Option<SourceEvent> {
    if b.len() < 8 {
        return None;
    }
    let kind = match b[0] {
        0 => EventKind::Added,
        1 => EventKind::Modified,
        2 => EventKind::Removed,
        _ => return None,
    };
    Some(SourceEvent { kind, flags: b[1], category: b[2], uid: u32::from_le_bytes([b[4], b[5], b[6], b[7]]) })
}

const ATTR_APP_ID: u8 = 0;
const ATTR_TITLE: u8 = 1;
const ATTR_SUBTITLE: u8 = 2;
const ATTR_MESSAGE: u8 = 3;

/// Control Point: GetNotificationAttributes for app id, title, subtitle and message.
pub fn notification_request(uid: u32) -> Vec<u8> {
    let mut v = vec![0];
    v.extend(uid.to_le_bytes());
    v.push(ATTR_APP_ID);
    for (attr, max) in [(ATTR_TITLE, 128u16), (ATTR_SUBTITLE, 128), (ATTR_MESSAGE, 1024)] {
        v.push(attr);
        v.extend(max.to_le_bytes());
    }
    v
}

/// Control Point: GetAppAttributes for the display name.
pub fn app_request(app_id: &str) -> Vec<u8> {
    let mut v = vec![1];
    v.extend(app_id.as_bytes());
    v.push(0);
    v.push(0); // DisplayName
    v
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NotificationAttrs {
    pub uid: u32,
    pub app_id: String,
    pub title: String,
    pub subtitle: String,
    pub message: String,
}

/// `count` (id, u16 length, value) tuples starting at `at`; None until all bytes have arrived.
fn parse_tuples(b: &[u8], mut at: usize, count: usize) -> Option<(Vec<(u8, String)>, usize)> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let id = *b.get(at)?;
        let len = u16::from_le_bytes([*b.get(at + 1)?, *b.get(at + 2)?]) as usize;
        let value = b.get(at + 3..at + 3 + len)?;
        out.push((id, String::from_utf8_lossy(value).into_owned()));
        at += 3 + len;
    }
    Some((out, at))
}

/// Data Source reply to `notification_request` (may span several packets).
pub fn parse_notification_reply(b: &[u8]) -> Option<(NotificationAttrs, usize)> {
    if *b.first()? != 0 || b.len() < 5 {
        return None;
    }
    let uid = u32::from_le_bytes([b[1], b[2], b[3], b[4]]);
    let (tuples, used) = parse_tuples(b, 5, 4)?;
    let mut n = NotificationAttrs { uid, ..Default::default() };
    for (id, v) in tuples {
        match id {
            ATTR_APP_ID => n.app_id = v,
            ATTR_TITLE => n.title = v,
            ATTR_SUBTITLE => n.subtitle = v,
            ATTR_MESSAGE => n.message = v,
            _ => {}
        }
    }
    Some((n, used))
}

/// Data Source reply to `app_request`: the app's display name.
pub fn parse_app_reply(b: &[u8]) -> Option<(String, usize)> {
    if *b.first()? != 1 {
        return None;
    }
    let nul = b[1..].iter().position(|&c| c == 0)? + 1;
    let (tuples, used) = parse_tuples(b, nul + 1, 1)?;
    Some((tuples.into_iter().next()?.1, used))
}

// MARK: Desktop notifications

/// Desktop notifications, keyed by the iPhone's notification UID so they can be replaced and closed.
#[derive(Default)]
struct Desktop {
    shown: Mutex<HashMap<u32, u32>>,
}

impl Desktop {
    fn show(self: &Arc<Self>, n: NotificationAttrs, app: String) {
        let me = self.clone();
        tokio::task::spawn_blocking(move || {
            let summary = match (n.title.is_empty(), app.is_empty()) {
                (false, false) if n.title != app => format!("{app}: {}", n.title),
                (false, _) => n.title.clone(),
                (true, _) => app.clone(),
            };
            let body = [n.subtitle.as_str(), n.message.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("\n");
            let mut note = notify_rust::Notification::new();
            note.appname("iPhone").summary(&summary).body(&body).icon("smartphone");
            if let Some(&id) = me.shown.lock().unwrap().get(&n.uid) {
                note.id(id);
            }
            match note.show() {
                Ok(handle) => {
                    me.shown.lock().unwrap().insert(n.uid, handle.id());
                }
                Err(e) => warn!("desktop notification failed: {e}"),
            }
        });
    }

    fn close(self: &Arc<Self>, uid: u32) {
        let Some(id) = self.shown.lock().unwrap().remove(&uid) else { return };
        tokio::task::spawn_blocking(move || {
            // Replace with an expiring empty notification: notify-rust can only close via a handle.
            let _ = std::process::Command::new("gdbus")
                .args([
                    "call",
                    "--session",
                    "--dest=org.freedesktop.Notifications",
                    "--object-path=/org/freedesktop/Notifications",
                    "--method=org.freedesktop.Notifications.CloseNotification",
                    &id.to_string(),
                ])
                .output();
        });
    }
}

// MARK: Bluetooth

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LinkState {
    #[default]
    Starting,
    Unavailable(String),
    /// Advertising; no iPhone connected.
    Waiting,
    Connected(String),
}

pub struct PhoneLink {
    pairing_until: Mutex<Option<Instant>>,
    state: Mutex<LinkState>,
    adapter: Mutex<Option<bluer::Adapter>>,
    runtime: tokio::runtime::Handle,
}

impl PhoneLink {
    pub fn spawn() -> Arc<Self> {
        let link = Arc::new(Self {
            pairing_until: Mutex::new(None),
            state: Mutex::new(LinkState::Starting),
            adapter: Mutex::new(None),
            runtime: tokio::runtime::Handle::current(),
        });
        let l = link.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = l.run().await {
                    warn!("phone notifications: {e:#}");
                    l.set_state(LinkState::Unavailable(format!("{e:#}")));
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
        link
    }

    pub fn state(&self) -> LinkState {
        self.state.lock().unwrap().clone()
    }

    fn set_state(&self, s: LinkState) {
        *self.state.lock().unwrap() = s;
    }

    fn pairing_open(&self) -> bool {
        self.pairing_until.lock().unwrap().is_some_and(|t| Instant::now() < t)
    }

    /// Accept a new iPhone pairing for the next two minutes (tray menu).
    pub fn open_pairing(&self) {
        *self.pairing_until.lock().unwrap() = Some(Instant::now() + PAIRING_WINDOW);
        info!("phone notification pairing open for 2 minutes");
        // iOS only lists the PC in Settings → Bluetooth while it's discoverable.
        if let Some(adapter) = self.adapter.lock().unwrap().clone() {
            self.runtime.spawn(async move {
                let _ = adapter.set_discoverable_timeout(PAIRING_WINDOW.as_secs() as u32).await;
                if let Err(e) = adapter.set_discoverable(true).await {
                    warn!("making Bluetooth discoverable: {e}");
                }
            });
        }
        let _ = std::process::Command::new("notify-send")
            .args([
                "--app-name=Vulcan",
                "--icon=smartphone",
                "Pair your iPhone for notifications",
                "On the iPhone: Settings → Bluetooth → tap “Vulcan” under Other Devices, accept the code and \
                 allow notifications. Then tap ⓘ next to it and turn on Share System Notifications.",
            ])
            .spawn();
    }

    async fn run(self: &Arc<Self>) -> Result<()> {
        let session = bluer::Session::new().await.context("connecting to BlueZ")?;
        let adapter = session.default_adapter().await.context("no Bluetooth adapter")?;
        adapter.set_powered(true).await?;
        adapter.set_pairable(true).await?;
        *self.adapter.lock().unwrap() = Some(adapter.clone());

        // Pairing is only accepted while the tray's pairing window is open.
        let agent = {
            let confirm = self.clone();
            let authorize = self.clone();
            Agent {
                request_default: true,
                request_confirmation: Some(Box::new(move |req| {
                    let open = confirm.pairing_open();
                    Box::pin(async move {
                        if !open {
                            return Err(ReqError::Rejected);
                        }
                        let _ = std::process::Command::new("notify-send")
                            .args(["--app-name=Vulcan", "--icon=smartphone", "Bluetooth pairing", &format!("Code {:06}: accept it on the iPhone", req.passkey)])
                            .spawn();
                        Ok(())
                    })
                })),
                request_authorization: Some(Box::new(move |_| {
                    let open = authorize.pairing_open();
                    Box::pin(async move { if open { Ok(()) } else { Err(ReqError::Rejected) } })
                })),
                ..Default::default()
            }
        };
        let _agent = session.register_agent(agent).await.context("registering pairing agent")?;

        let _adv = adapter
            .advertise(Advertisement {
                solicit_uuids: [ANCS_SERVICE].into(),
                local_name: Some("Vulcan".into()),
                discoverable: Some(true),
                ..Default::default()
            })
            .await
            .context("starting Bluetooth advertising")?;
        info!("phone notifications: advertising for ANCS");
        self.set_state(LinkState::Waiting);

        let desktop = Arc::new(Desktop::default());
        let active: Arc<Mutex<HashSet<Address>>> = Default::default();
        loop {
            for addr in adapter.device_addresses().await? {
                if active.lock().unwrap().contains(&addr) {
                    continue;
                }
                let device = adapter.device(addr)?;
                if !device.is_connected().await.unwrap_or(false) {
                    continue;
                }
                active.lock().unwrap().insert(addr);
                let (link, desktop, active) = (self.clone(), desktop.clone(), active.clone());
                tokio::spawn(async move {
                    let name = device.alias().await.unwrap_or_else(|_| addr.to_string());
                    info!(device = %name, "Bluetooth device connected; looking for ANCS");
                    // Pairing and GATT discovery can finish after the connection appears, so
                    // keep checking for a while before deciding it isn't an iPhone.
                    for attempt in 0..12 {
                        if !device.is_connected().await.unwrap_or(false) {
                            break;
                        }
                        match serve(&device, &link, &desktop, &name).await {
                            Ok(true) => {
                                info!(device = %name, "iPhone notifications disconnected");
                                break;
                            }
                            Ok(false) if attempt == 11 => info!(device = %name, "no ANCS service; ignoring"),
                            Ok(false) => {}
                            Err(e) => warn!(device = %name, attempt, "iPhone notifications: {e:#}"),
                        }
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                    if matches!(link.state(), LinkState::Connected(ref n) if *n == name) {
                        link.set_state(LinkState::Waiting);
                    }
                    // Allow a reconnect to be picked up once this one has gone.
                    while device.is_connected().await.unwrap_or(false) {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                    active.lock().unwrap().remove(&addr);
                });
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

/// Handle one connected device. Returns Ok(false) if it isn't an iPhone offering ANCS.
async fn serve(device: &Device, link: &PhoneLink, desktop: &Arc<Desktop>, name: &str) -> Result<bool> {
    // Wait for GATT discovery.
    for _ in 0..30 {
        if device.is_services_resolved().await.unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let mut ancs = None;
    for service in device.services().await? {
        if service.uuid().await? == ANCS_SERVICE {
            ancs = Some(service);
        }
    }
    let Some(ancs) = ancs else { return Ok(false) };
    let (mut source, mut control, mut data) = (None, None, None);
    for c in ancs.characteristics().await? {
        match c.uuid().await? {
            u if u == NOTIFICATION_SOURCE => source = Some(c),
            u if u == CONTROL_POINT => control = Some(c),
            u if u == DATA_SOURCE => data = Some(c),
            _ => {}
        }
    }
    let (Some(source), Some(control), Some(data)) = (source, control, data) else {
        bail!("ANCS characteristics missing");
    };

    // Subscribing needs an encrypted link, which makes BlueZ pair if needed (the agent decides).
    let data_stream = data.notify().await.context("subscribing to Data Source (paired, notifications allowed?)")?;
    let mut source_stream = Box::pin(source.notify().await.context("subscribing to Notification Source")?);
    info!(device = %name, "iPhone notifications connected");
    link.set_state(LinkState::Connected(name.to_string()));

    let mut data_stream = Box::pin(data_stream);
    let mut buf = Vec::new();
    let mut app_names: HashMap<String, String> = HashMap::new();
    while let Some(packet) = source_stream.next().await {
        let Some(ev) = parse_source(&packet) else { continue };
        debug!(?ev, "ANCS event");
        match ev.kind {
            EventKind::Removed => desktop.close(ev.uid),
            // Skip what was already on the phone when it connected, and silent ones.
            _ if ev.flags & (FLAG_PRE_EXISTING | FLAG_SILENT) != 0 => {}
            EventKind::Added | EventKind::Modified => {
                let attrs = match request(&control, &mut data_stream, &mut buf, &notification_request(ev.uid), parse_notification_reply).await {
                    Ok(a) => a,
                    Err(e) => {
                        warn!("notification details: {e:#}");
                        continue;
                    }
                };
                let app = match app_names.get(&attrs.app_id) {
                    Some(n) => n.clone(),
                    None => {
                        let name = request(&control, &mut data_stream, &mut buf, &app_request(&attrs.app_id), parse_app_reply)
                            .await
                            .unwrap_or_else(|_| attrs.app_id.clone());
                        app_names.insert(attrs.app_id.clone(), name.clone());
                        name
                    }
                };
                desktop.show(attrs, app);
            }
        }
    }
    Ok(true)
}

/// Write a Control Point command and collect the (possibly fragmented) Data Source reply.
async fn request<T>(
    control: &Characteristic,
    data: &mut (impl Stream<Item = Vec<u8>> + Unpin),
    buf: &mut Vec<u8>,
    command: &[u8],
    parse: impl Fn(&[u8]) -> Option<(T, usize)>,
) -> Result<T> {
    buf.clear();
    // Drop anything stale from an earlier timed-out request.
    while let Some(Some(stale)) = futures_util::FutureExt::now_or_never(data.next()) {
        debug!(bytes = stale.len(), "ANCS: discarding stale data");
    }
    // Plain write-with-response: the Control Point also advertises reliable writes, which
    // BlueZ would otherwise use, and iOS accepts those without ever replying.
    let req = bluer::gatt::remote::CharacteristicWriteRequest {
        op_type: bluer::gatt::WriteOp::Request,
        ..Default::default()
    };
    control.write_ext(command, &req).await.context("writing Control Point")?;
    debug!(command = %hex::encode(command), "ANCS: control point written");
    loop {
        if let Some((value, used)) = parse(buf) {
            buf.drain(..used);
            return Ok(value);
        }
        let chunk = tokio::time::timeout(Duration::from_secs(3), data.next())
            .await
            .context("no reply from the iPhone")?
            .context("Data Source closed")?;
        debug!(bytes = chunk.len(), "ANCS: data");
        buf.extend(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_events() {
        let ev = parse_source(&[0, 0b10, 4, 1, 0x2a, 0, 0, 0]).unwrap();
        assert_eq!(ev, SourceEvent { kind: EventKind::Added, flags: 2, category: 4, uid: 42 });
        assert_eq!(parse_source(&[2, 0, 0, 0, 1, 0, 0, 0]).unwrap().kind, EventKind::Removed);
        assert!(parse_source(&[9, 0, 0, 0, 0, 0, 0, 0]).is_none());
        assert!(parse_source(&[0, 0]).is_none());
    }

    #[test]
    fn requests() {
        assert_eq!(notification_request(42), [0, 42, 0, 0, 0, 0, 1, 128, 0, 2, 128, 0, 3, 0, 4]);
        assert_eq!(app_request("a.b"), [1, b'a', b'.', b'b', 0, 0]);
    }

    fn tuple(id: u8, s: &str) -> Vec<u8> {
        let mut v = vec![id];
        v.extend((s.len() as u16).to_le_bytes());
        v.extend(s.as_bytes());
        v
    }

    #[test]
    fn notification_reply_in_fragments() {
        let mut reply = vec![0, 7, 0, 0, 0];
        for (id, s) in [(0, "com.apple.MobileSMS"), (1, "Alice"), (2, ""), (3, "See you at 8 🙂")] {
            reply.extend(tuple(id, s));
        }
        reply.extend([0xff, 0xff]); // start of a later packet stays in the buffer
        for cut in [3, 10, 30] {
            assert!(parse_notification_reply(&reply[..cut]).is_none(), "incomplete at {cut}");
        }
        let (n, used) = parse_notification_reply(&reply).unwrap();
        assert_eq!(used, reply.len() - 2);
        assert_eq!(n.uid, 7);
        assert_eq!(n.app_id, "com.apple.MobileSMS");
        assert_eq!(n.title, "Alice");
        assert_eq!(n.message, "See you at 8 🙂");
    }

    #[test]
    fn app_reply() {
        let mut reply = vec![1];
        reply.extend(b"com.apple.MobileSMS\0");
        reply.extend(tuple(0, "Messages"));
        assert_eq!(parse_app_reply(&reply).unwrap(), ("Messages".to_string(), reply.len()));
        assert!(parse_app_reply(&reply[..reply.len() - 2]).is_none());
    }
}
