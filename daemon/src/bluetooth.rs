//! Remote control over Bluetooth LE, for when the phone can't reach the PC over Wi-Fi.
//!
//! The PC serves a GATT service with two characteristics: the phone writes protocol messages
//! to RX and subscribes to TX for replies. Messages are the same JSON as the WebSocket,
//! split into packets with a one-byte header (1 = more follows, 0 = last). The link is
//! encrypted by the phone's existing bond, and sessions still start with the pairing token.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bluer::adv::Advertisement;
use bluer::gatt::local::{
    Application, Characteristic, CharacteristicControlEvent, CharacteristicNotify, CharacteristicNotifyMethod,
    CharacteristicWrite, CharacteristicWriteMethod, Service, characteristic_control,
};
use bluer::gatt::{CharacteristicReader, CharacteristicWriter};
use bluer::Uuid;
use futures_util::{StreamExt, future};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::server::{AppState, Link, Transport, session};

pub const SERVICE: Uuid = Uuid::from_u128(0x8f3e0001_5c2a_4e6b_9c1d_7a5b3e9f2c01);
const RX: Uuid = Uuid::from_u128(0x8f3e0002_5c2a_4e6b_9c1d_7a5b3e9f2c01);
const TX: Uuid = Uuid::from_u128(0x8f3e0003_5c2a_4e6b_9c1d_7a5b3e9f2c01);

const MORE: u8 = 1;
const LAST: u8 = 0;

/// Split a message into packets of at most `max` bytes, each starting with a header byte.
pub fn fragment(msg: &[u8], max: usize) -> Vec<Vec<u8>> {
    let payload = max.saturating_sub(1).max(1);
    let chunks: Vec<&[u8]> = if msg.is_empty() { vec![&[][..]] } else { msg.chunks(payload).collect() };
    let n = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let mut p = Vec::with_capacity(c.len() + 1);
            p.push(if i + 1 < n { MORE } else { LAST });
            p.extend_from_slice(c);
            p
        })
        .collect()
}

/// Joins packets back into messages.
#[derive(Default)]
pub struct Reassembler {
    buf: Vec<u8>,
}

impl Reassembler {
    /// Feed one packet; returns a message once its last packet arrives.
    pub fn push(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        let (&header, body) = packet.split_first()?;
        self.buf.extend_from_slice(body);
        // Guard against a runaway sender.
        if self.buf.len() > 1 << 20 {
            self.buf.clear();
            return None;
        }
        (header == LAST).then(|| std::mem::take(&mut self.buf))
    }
}

pub fn spawn(st: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = run(&st).await {
                warn!("Bluetooth remote: {e:#}");
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

async fn run(st: &Arc<AppState>) -> Result<()> {
    let session_bt = bluer::Session::new().await.context("connecting to BlueZ")?;
    let adapter = session_bt.default_adapter().await.context("no Bluetooth adapter")?;
    adapter.set_powered(true).await?;

    let (rx_control, rx_handle) = characteristic_control();
    let (tx_control, tx_handle) = characteristic_control();
    let app = Application {
        services: vec![Service {
            uuid: SERVICE,
            primary: true,
            characteristics: vec![
                Characteristic {
                    uuid: RX,
                    write: Some(CharacteristicWrite {
                        write: true,
                        write_without_response: true,
                        encrypt_write: true,
                        method: CharacteristicWriteMethod::Io,
                        ..Default::default()
                    }),
                    control_handle: rx_handle,
                    ..Default::default()
                },
                Characteristic {
                    uuid: TX,
                    notify: Some(CharacteristicNotify {
                        notify: true,
                        method: CharacteristicNotifyMethod::Io,
                        ..Default::default()
                    }),
                    control_handle: tx_handle,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let _app = adapter.serve_gatt_application(app).await.context("serving GATT service")?;
    // A separate advertising set: the ANCS one has no room for another 128-bit UUID.
    let _adv = adapter
        .advertise(Advertisement { service_uuids: [SERVICE].into(), ..Default::default() })
        .await
        .context("advertising the remote-control service")?;
    info!("Bluetooth remote control ready");

    let mut rx_control = Box::pin(rx_control);
    let mut tx_control = Box::pin(tx_control);
    let mut reader: Option<CharacteristicReader> = None;
    let mut writer: Option<CharacteristicWriter> = None;
    let mut reassembler = Reassembler::default();
    // The current session's channels.
    let mut to_session: Option<mpsc::Sender<String>> = None;
    let mut from_session: Option<mpsc::Receiver<String>> = None;

    loop {
        tokio::select! {
            evt = rx_control.next() => match evt {
                Some(CharacteristicControlEvent::Write(req)) => {
                    debug!(mtu = req.mtu(), device = %req.device_address(), "Bluetooth RX opened");
                    reader = Some(req.accept()?);
                    reassembler = Reassembler::default();
                }
                Some(_) => {}
                None => anyhow::bail!("GATT application stopped"),
            },
            evt = tx_control.next() => match evt {
                Some(CharacteristicControlEvent::Notify(notifier)) => {
                    debug!(mtu = notifier.mtu(), device = %notifier.device_address(), "Bluetooth TX subscribed");
                    writer = Some(notifier);
                    // A new subscription is a new app connection. The phone's Bluetooth link to the
                    // PC can outlive the app's (it stays up for notifications), so end any old session.
                    to_session = None;
                    from_session = None;
                    reassembler = Reassembler::default();
                }
                Some(_) => {}
                None => anyhow::bail!("GATT application stopped"),
            },
            read = async {
                match reader.as_mut() {
                    Some(r) => r.recv().await.map(Some),
                    None => future::pending().await,
                }
            } => {
                let packet = match read {
                    Ok(Some(p)) if !p.is_empty() => p,
                    _ => {
                        // Phone unsubscribed or went away: end the session.
                        debug!("Bluetooth RX closed");
                        reader = None;
                        to_session = None;
                        from_session = None;
                        continue;
                    }
                };
                let Some(msg) = reassembler.push(&packet) else { continue };
                let Ok(text) = String::from_utf8(msg) else { continue };
                // A hello always starts a fresh session (it checks the token); so does the first
                // message after the old one ended.
                let is_hello = text.contains(r#""t":"hello""#);
                if is_hello || to_session.as_ref().is_none_or(|t| t.is_closed()) {
                    let (in_tx, in_rx) = mpsc::channel(64);
                    let (out_tx, out_rx) = mpsc::channel(64);
                    let peer = writer.as_ref().map_or_else(|| "Bluetooth".to_string(), |w| format!("bt:{}", w.device_address()));
                    let link = Link { incoming: in_rx, outgoing: out_tx, transport: Transport::Bluetooth, peer };
                    let st = st.clone();
                    tokio::spawn(async move {
                        if let Err(e) = session(link, st).await {
                            warn!("Bluetooth session ended with error: {e:#}");
                        }
                    });
                    to_session = Some(in_tx);
                    from_session = Some(out_rx);
                }
                if let Some(tx) = &to_session {
                    let _ = tx.send(text).await;
                }
            },
            out = async {
                match from_session.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => future::pending().await,
                }
            } => {
                let Some(text) = out else {
                    // Session finished (e.g. bad token); wait for the phone to reconnect.
                    from_session = None;
                    to_session = None;
                    continue;
                };
                let Some(w) = writer.as_mut() else { continue };
                let max = w.mtu().saturating_sub(3).max(20);
                let mut failed = false;
                for packet in fragment(text.as_bytes(), max) {
                    if w.send(&packet).await.is_err() {
                        failed = true;
                        break;
                    }
                }
                if failed {
                    debug!("Bluetooth TX closed");
                    writer = None;
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_round_trip() {
        let msg: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let packets = fragment(&msg, 182);
        assert!(packets.iter().all(|p| p.len() <= 182));
        assert_eq!(packets.last().unwrap()[0], LAST);
        assert!(packets[..packets.len() - 1].iter().all(|p| p[0] == MORE));
        let mut r = Reassembler::default();
        let mut out = None;
        for p in &packets {
            out = r.push(p);
        }
        assert_eq!(out.unwrap(), msg);
    }

    #[test]
    fn small_and_empty() {
        assert_eq!(fragment(b"hi", 20), vec![vec![LAST, b'h', b'i']]);
        assert_eq!(fragment(b"", 20), vec![vec![LAST]]);
        let mut r = Reassembler::default();
        assert_eq!(r.push(&[LAST, b'h', b'i']).unwrap(), b"hi");
        assert!(r.push(&[]).is_none());
    }
}
