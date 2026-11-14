use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::Response;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum_server::tls_rustls::RustlsConfig;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{info, warn};

use crate::auth::DeviceStore;
use crate::config::{Config, Paths};
use crate::input::Input;
use crate::clipboard::{self, Clip, ClipMemory};
use crate::config::RuntimeState;
use crate::protocol::{ClientMsg, CommandInfo, PROTOCOL_VERSION, ServerMsg};
use crate::files::{self, Offers};
use crate::{actions, usb};
use crate::tls::Tls;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub connected: Vec<String>,
    pub paused: bool,
    pub runtime: RuntimeState,
}

pub struct AppState {
    pub paths: Paths,
    pub cfg: Config,
    pub fingerprint: String,
    pub host: String,
    pub input: Mutex<Input>,
    pub status: watch::Sender<Status>,
    /// New PC clipboard content, from the watcher.
    pub clip_tx: broadcast::Sender<Arc<Clip>>,
    pub clip_memory: Mutex<ClipMemory>,
    /// Files queued for the phone.
    pub offers: Offers,
    pub cursor: Option<crate::cursor::CursorBoost>,
    /// iPhone notifications over Bluetooth (None when disabled).
    pub phone_link: Option<Arc<crate::ancs::PhoneLink>>,
}

impl AppState {
    pub fn set_paused(&self, paused: bool) {
        if paused {
            self.input.lock().unwrap().release_all();
        }
        self.status.send_modify(|s| s.paused = paused);
        info!(paused, "input pause toggled");
    }

    pub fn paused(&self) -> bool {
        self.status.borrow().paused
    }

    pub fn runtime(&self) -> RuntimeState {
        self.status.borrow().runtime
    }

    /// Change clipboard auto-sync settings (from tray or phone) and persist them.
    pub fn set_clip_sync(&self, to_phone: Option<bool>, to_pc: Option<bool>) {
        self.status.send_modify(|s| {
            if let Some(v) = to_phone {
                s.runtime.clip_to_phone = v;
            }
            if let Some(v) = to_pc {
                s.runtime.clip_to_pc = v;
            }
        });
        let rt = self.runtime();
        if let Err(e) = rt.save(&self.paths) {
            warn!("saving state: {e:#}");
        }
        info!(to_phone = rt.clip_to_phone, to_pc = rt.clip_to_pc, "clipboard sync settings");
    }
}

pub async fn run(st: Arc<AppState>, tls: &Tls) -> Result<()> {
    let rustls = RustlsConfig::from_pem(tls.cert_pem.clone(), tls.key_pem.clone()).await?;
    let addr: SocketAddr = format!("{}:{}", st.cfg.bind, st.cfg.port).parse()?;
    let app = Router::new()
        .route("/", get(|| async { "vulcand" }))
        .route("/ws", get(ws_handler))
        .route("/files/upload", post(files::upload).layer(DefaultBodyLimit::disable()))
        .route("/files/{id}", get(files::download_offer))
        .route("/fs/list", get(files::fs_list))
        .route("/fs/download", get(files::fs_download))
        .route("/fs/walk", get(files::fs_walk))
        .route("/fs/thumb", get(files::fs_thumb))
        .with_state(st);
    info!(%addr, "listening");
    axum_server::bind_rustls(addr, rustls)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await?;
    Ok(())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(st): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    ws.max_message_size(32 << 20).max_frame_size(32 << 20).on_upgrade(move |socket| async move {
        let (link, pump) = websocket_link(socket, addr.to_string());
        tokio::spawn(pump);
        if let Err(e) = session(link, st).await {
            warn!(%addr, "session ended with error: {e:#}");
        }
    })
}

/// How a phone is connected. Large transfers (files, clipboard images) only go over Wi-Fi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    WiFi,
    Bluetooth,
}

/// A connected phone as a pair of text-message channels, whatever the transport.
pub struct Link {
    pub incoming: mpsc::Receiver<String>,
    pub outgoing: mpsc::Sender<String>,
    pub transport: Transport,
    pub peer: String,
}

impl Link {
    async fn send(&self, msg: &ServerMsg) -> Result<()> {
        self.outgoing.send(serde_json::to_string(msg)?).await.map_err(|_| anyhow::anyhow!("link closed"))
    }
}

/// Bridge a WebSocket to a Link; the returned future pumps until either side closes.
fn websocket_link(mut socket: WebSocket, peer: String) -> (Link, impl std::future::Future<Output = ()>) {
    let (in_tx, in_rx) = mpsc::channel::<String>(64);
    let (out_tx, mut out_rx) = mpsc::channel::<String>(64);
    let pump = async move {
        loop {
            tokio::select! {
                msg = socket.recv() => match msg {
                    // Phones vanish without a clean close (sleep, Wi-Fi drop); that's a normal end.
                    Some(Ok(Message::Text(t))) => {
                        if in_tx.send(t.to_string()).await.is_err() { break }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                },
                out = out_rx.recv() => match out {
                    Some(text) => {
                        if socket.send(Message::Text(text.into())).await.is_err() { break }
                    }
                    None => break,
                },
            }
        }
        let _ = socket.send(Message::Close(None)).await;
    };
    (Link { incoming: in_rx, outgoing: out_tx, transport: Transport::WiFi, peer }, pump)
}

pub async fn session(mut link: Link, st: Arc<AppState>) -> Result<()> {
    let addr = link.peer.clone();
    let wifi = link.transport == Transport::WiFi;
    // The first frame must be a hello carrying a paired token.
    let token = match tokio::time::timeout(Duration::from_secs(5), link.incoming.recv()).await {
        Ok(Some(t)) => match serde_json::from_str::<ClientMsg>(&t) {
            Ok(ClientMsg::Hello { token, .. }) => token,
            _ => {
                link.send(&ServerMsg::Error { msg: "expected hello".into() }).await?;
                return Ok(());
            }
        },
        _ => return Ok(()),
    };
    let name = DeviceStore::load(&st.paths.devices())?.verify(&token).map(str::to_string);
    let Some(name) = name else {
        warn!(%addr, "rejected connection with unknown token");
        tokio::time::sleep(Duration::from_secs(1)).await;
        link.send(&ServerMsg::Error { msg: "not paired".into() }).await?;
        return Ok(());
    };

    info!(%addr, device = %name, transport = ?link.transport, "connected");
    let mut caps =
        vec!["input".to_string(), "media".into(), "volume".into(), "display".into(), "commands".into(), "clipboard".into()];
    if st.cfg.power.enabled {
        caps.push("power".into());
    }
    if !st.cfg.usb.is_empty() {
        caps.push("usb".into());
    }
    // File transfer and the PC browser use HTTPS, so they need Wi-Fi.
    if wifi {
        caps.push("files".into());
    }
    let commands = st
        .cfg
        .commands
        .iter()
        .map(|c| CommandInfo { id: c.name.clone(), name: c.name.clone(), icon: c.icon.clone() })
        .collect();
    link.send(&ServerMsg::Welcome {
        v: PROTOCOL_VERSION,
        host: st.host.clone(),
        device: name.clone(),
        caps,
        commands,
    })
    .await?;
    if st.paused() {
        link.send(&ServerMsg::Paused { paused: true }).await?;
    }
    let rt = st.runtime();
    link.send(&ServerMsg::ClipSettings { to_phone: rt.clip_to_phone, to_pc: rt.clip_to_pc }).await?;
    // The phone reconnects whenever it comes to the front; offer the current PC clipboard.
    // It skips content it already has (by hash), so only new PC copies replace its clipboard.
    if rt.clip_to_phone
        && !st.paused()
        && let Ok(Some(clip)) = clipboard::read().await
        && clip.fits(link.transport)
    {
        link.send(&clip.to_msg()).await?;
    }
    if wifi {
        for offer in st.offers.pending() {
            link.send(&ServerMsg::FileOffer { id: offer.id, name: offer.name, size: offer.size }).await?;
        }
    }
    st.status.send_modify(|s| s.connected.push(name.clone()));

    let result = run_session(&mut link, &st, &token).await;

    st.input.lock().unwrap().release_all();
    st.status.send_modify(|s| {
        if let Some(i) = s.connected.iter().position(|n| n == &name) {
            s.connected.remove(i);
        }
    });
    info!(%addr, device = %name, "disconnected");
    result
}

async fn run_session(link: &mut Link, st: &AppState, token: &str) -> Result<()> {
    let wifi = link.transport == Transport::WiFi;
    let mut recheck = tokio::time::interval(Duration::from_secs(5));
    let mut status_rx = st.status.subscribe();
    let mut last_paused = status_rx.borrow_and_update().paused;
    let mut last_runtime = st.runtime();
    let mut clip_rx = st.clip_tx.subscribe();
    let mut offer_rx = st.offers.tx.subscribe();
    loop {
        tokio::select! {
            msg = link.incoming.recv() => {
                let Some(text) = msg else { return Ok(()) };
                for reply in handle(st, &text, link.transport).await {
                    link.send(&reply).await?;
                }
            }
            clip = clip_rx.recv() => {
                match clip {
                    Ok(clip) => {
                        if st.runtime().clip_to_phone && !st.paused() && clip.fits(link.transport) {
                            link.send(&clip.to_msg()).await?;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
            offer = offer_rx.recv() => {
                match offer {
                    Ok(o) if wifi => link.send(&ServerMsg::FileOffer { id: o.id, name: o.name, size: o.size }).await?,
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
            // Drop the session promptly if the device is revoked (tray or CLI).
            _ = recheck.tick() => {
                if DeviceStore::load(&st.paths.devices())?.verify(token).is_none() {
                    link.send(&ServerMsg::Error { msg: "pairing revoked".into() }).await?;
                    return Ok(());
                }
            }
            changed = status_rx.changed() => {
                if changed.is_err() { return Ok(()) }
                let (paused, runtime) = {
                    let s = status_rx.borrow_and_update();
                    (s.paused, s.runtime)
                };
                if paused != last_paused {
                    last_paused = paused;
                    link.send(&ServerMsg::Paused { paused }).await?;
                }
                if runtime != last_runtime {
                    last_runtime = runtime;
                    link.send(&ServerMsg::ClipSettings { to_phone: runtime.clip_to_phone, to_pc: runtime.clip_to_pc }).await?;
                }
            }
        }
    }
}

async fn handle(st: &AppState, text: &str, transport: Transport) -> Vec<ServerMsg> {
    let msg = match serde_json::from_str::<ClientMsg>(text) {
        Ok(m) => m,
        Err(e) => return vec![ServerMsg::Error { msg: format!("bad message: {e}") }],
    };
    match msg {
        ClientMsg::Hello { .. } => return vec![],
        ClientMsg::Ping => return vec![ServerMsg::Pong],
        ClientMsg::Refresh => return refresh(st).await,
        ClientMsg::ClipSettings { to_phone, to_pc } => {
            st.set_clip_sync(to_phone, to_pc);
            return vec![];
        }
        _ => {}
    }
    // Pause is a kill switch for everything the phone can do.
    if st.paused() {
        return vec![];
    }
    let result: anyhow::Result<Vec<ServerMsg>> = match msg {
        ClientMsg::Media { a } => match actions::media(a).await {
            Ok(()) => {
                // Give the player a moment to report its new state.
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(vec![actions::now_playing().await])
            }
            Err(e) => Err(e),
        },
        ClientMsg::Volume { a, value } => match actions::volume(a, value).await {
            Ok(()) => Ok(actions::get_volume().await.into_iter().collect()),
            Err(e) => Err(e),
        },
        ClientMsg::Power { a } => {
            info!(action = ?a, "power action from phone");
            actions::power(a, &st.cfg.power).await.map(|msg| vec![ServerMsg::Info { msg }])
        }
        ClientMsg::Display { a, monitor } => actions::display(a, monitor.as_deref(), &st.cfg.displays)
            .await
            .map(|(msg, monitors)| vec![ServerMsg::Info { msg }, ServerMsg::Displays { monitors }]),
        ClientMsg::Usb { name, on } => {
            info!(device = %name, ?on, "USB toggle from phone");
            usb::set(&st.cfg.usb, &name, on).map(|msg| {
                vec![ServerMsg::Info { msg }, ServerMsg::UsbDevices { devices: usb::states(&st.cfg.usb) }]
            })
        }
        ClientMsg::ClipSet { kind, text, mime, data, auto } => {
            if auto && !st.runtime().clip_to_pc {
                return vec![];
            }
            match Clip::from_parts(kind, text, mime, data) {
                Ok(clip) => {
                    // Remember it so the watcher doesn't send it straight back to the phone.
                    st.clip_memory.lock().unwrap().from_phone = Some(clip.hash());
                    clipboard::write(&clip).await.map(|()| {
                        info!(auto, kind = ?kind, "clipboard from phone");
                        if auto { vec![] } else { vec![ServerMsg::Info { msg: "Copied to the PC clipboard".into() }] }
                    })
                }
                Err(e) => Err(e),
            }
        }
        ClientMsg::FileDone { id } => {
            st.offers.done(&id);
            Ok(vec![])
        }
        ClientMsg::ClipGet => match clipboard::read().await {
            Ok(Some(clip)) if !clip.fits(transport) => {
                Ok(vec![ServerMsg::Info { msg: "The PC clipboard has an image or is too big; that needs Wi-Fi".into() }])
            }
            Ok(Some(clip)) => Ok(vec![clip.to_msg()]),
            Ok(None) => Ok(vec![ServerMsg::Info { msg: "The PC clipboard is empty".into() }]),
            Err(e) => Err(e),
        },
        ClientMsg::Cmd { id } => {
            info!(command = %id, "custom command from phone");
            actions::custom(&id, &st.cfg.commands).await.map(|msg| vec![ServerMsg::Info { msg }])
        }
        input => handle_input(st, input).map(|r| r.into_iter().collect()),
    };
    result.unwrap_or_else(|e| vec![ServerMsg::Error { msg: format!("{e:#}") }])
}

async fn refresh(st: &AppState) -> Vec<ServerMsg> {
    let (volume, media, monitors) =
        tokio::join!(actions::get_volume(), actions::now_playing(), actions::monitors(&st.cfg.displays));
    let monitors = monitors.ok().map(|monitors| ServerMsg::Displays { monitors });
    let usb = (!st.cfg.usb.is_empty()).then(|| ServerMsg::UsbDevices { devices: usb::states(&st.cfg.usb) });
    volume.into_iter().chain([media]).chain(monitors).chain(usb).collect()
}

fn handle_input(st: &AppState, msg: ClientMsg) -> anyhow::Result<Option<ServerMsg>> {
    let mut input = st.input.lock().unwrap();
    match msg {
        ClientMsg::Move { dx, dy } => {
            input.move_by(dx, dy)?;
            if let Some(c) = &st.cursor {
                c.poke();
            }
        }
        ClientMsg::Scroll { dx, dy } => input.scroll(dx, dy)?,
        ClientMsg::Button { b, s } => input.button(b, s)?,
        ClientMsg::Key { k, mods, s } => input.key(&k, &mods, s)?,
        ClientMsg::Text { s } => {
            let skipped = input.type_text(&s)?;
            if !skipped.is_empty() {
                let s: String = skipped.into_iter().collect();
                return Ok(Some(ServerMsg::Error { msg: format!("can't type {s:?} with this keyboard layout") }));
            }
        }
        _ => {}
    }
    Ok(None)
}
