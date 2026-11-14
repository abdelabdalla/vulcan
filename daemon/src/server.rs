use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::Response;
use axum::routing::get;
use axum_server::tls_rustls::RustlsConfig;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::auth::DeviceStore;
use crate::config::{Config, Paths};
use crate::input::Input;
use crate::{actions, usb};
use crate::protocol::{ClientMsg, CommandInfo, PROTOCOL_VERSION, ServerMsg};
use crate::tls::Tls;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub connected: Vec<String>,
    pub paused: bool,
}

pub struct AppState {
    pub paths: Paths,
    pub cfg: Config,
    pub fingerprint: String,
    pub host: String,
    pub input: Mutex<Input>,
    pub status: watch::Sender<Status>,
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
}

pub async fn run(st: Arc<AppState>, tls: &Tls) -> Result<()> {
    let rustls = RustlsConfig::from_pem(tls.cert_pem.clone(), tls.key_pem.clone()).await?;
    let addr: SocketAddr = format!("{}:{}", st.cfg.bind, st.cfg.port).parse()?;
    let app = Router::new()
        .route("/", get(|| async { "vulcand" }))
        .route("/ws", get(ws_handler))
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
    ws.max_message_size(1 << 20).on_upgrade(move |socket| async move {
        if let Err(e) = session(socket, st, addr).await {
            warn!(%addr, "session ended with error: {e:#}");
        }
    })
}

async fn send(socket: &mut WebSocket, msg: &ServerMsg) -> Result<()> {
    socket.send(Message::Text(serde_json::to_string(msg)?.into())).await?;
    Ok(())
}

async fn session(mut socket: WebSocket, st: Arc<AppState>, addr: SocketAddr) -> Result<()> {
    // The first frame must be a hello carrying a paired token.
    let token = match tokio::time::timeout(Duration::from_secs(5), socket.recv()).await {
        Ok(Some(Ok(Message::Text(t)))) => match serde_json::from_str::<ClientMsg>(&t) {
            Ok(ClientMsg::Hello { token, .. }) => token,
            _ => {
                send(&mut socket, &ServerMsg::Error { msg: "expected hello".into() }).await?;
                return Ok(());
            }
        },
        _ => return Ok(()),
    };
    let name = DeviceStore::load(&st.paths.devices())?.verify(&token).map(str::to_string);
    let Some(name) = name else {
        warn!(%addr, "rejected connection with unknown token");
        tokio::time::sleep(Duration::from_secs(1)).await;
        send(&mut socket, &ServerMsg::Error { msg: "not paired".into() }).await?;
        return Ok(());
    };

    info!(%addr, device = %name, "connected");
    let mut caps = vec!["input".to_string(), "media".into(), "volume".into(), "display".into(), "commands".into()];
    if st.cfg.power.enabled {
        caps.push("power".into());
    }
    if !st.cfg.usb.is_empty() {
        caps.push("usb".into());
    }
    let commands = st
        .cfg
        .commands
        .iter()
        .map(|c| CommandInfo { id: c.name.clone(), name: c.name.clone(), icon: c.icon.clone() })
        .collect();
    send(&mut socket, &ServerMsg::Welcome {
        v: PROTOCOL_VERSION,
        host: st.host.clone(),
        device: name.clone(),
        caps,
        commands,
    })
    .await?;
    if st.paused() {
        send(&mut socket, &ServerMsg::Paused { paused: true }).await?;
    }
    st.status.send_modify(|s| s.connected.push(name.clone()));

    let result = run_session(&mut socket, &st, &token).await;

    st.input.lock().unwrap().release_all();
    st.status.send_modify(|s| {
        if let Some(i) = s.connected.iter().position(|n| n == &name) {
            s.connected.remove(i);
        }
    });
    info!(%addr, device = %name, "disconnected");
    result
}

async fn run_session(socket: &mut WebSocket, st: &AppState, token: &str) -> Result<()> {
    let mut recheck = tokio::time::interval(Duration::from_secs(5));
    let mut status_rx = st.status.subscribe();
    let mut last_paused = status_rx.borrow_and_update().paused;
    loop {
        tokio::select! {
            msg = socket.recv() => {
                // Phones vanish without a clean close (sleep, Wi-Fi drop); that's a normal end.
                let Some(Ok(msg)) = msg else { return Ok(()) };
                match msg {
                    Message::Text(t) => {
                        for reply in handle(st, &t).await {
                            send(socket, &reply).await?;
                        }
                    }
                    Message::Close(_) => return Ok(()),
                    _ => {}
                }
            }
            // Drop the session promptly if the device is revoked (tray or CLI).
            _ = recheck.tick() => {
                if DeviceStore::load(&st.paths.devices())?.verify(token).is_none() {
                    send(socket, &ServerMsg::Error { msg: "pairing revoked".into() }).await?;
                    return Ok(());
                }
            }
            changed = status_rx.changed() => {
                if changed.is_err() { return Ok(()) }
                let paused = status_rx.borrow_and_update().paused;
                if paused != last_paused {
                    last_paused = paused;
                    send(socket, &ServerMsg::Paused { paused }).await?;
                }
            }
        }
    }
}

async fn handle(st: &AppState, text: &str) -> Vec<ServerMsg> {
    let msg = match serde_json::from_str::<ClientMsg>(text) {
        Ok(m) => m,
        Err(e) => return vec![ServerMsg::Error { msg: format!("bad message: {e}") }],
    };
    match msg {
        ClientMsg::Hello { .. } => return vec![],
        ClientMsg::Ping => return vec![ServerMsg::Pong],
        ClientMsg::Refresh => return refresh(st).await,
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
        ClientMsg::Move { dx, dy } => input.move_by(dx, dy)?,
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
