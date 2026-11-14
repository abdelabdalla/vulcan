mod actions;
mod auth;
mod config;
mod input;
mod mdns;
mod pairing;
mod protocol;
mod server;
mod tls;
mod tray;
mod usb;
mod util;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::auth::DeviceStore;
use crate::config::{Config, Paths};
use crate::input::{Input, text::TextMapper};
use crate::server::{AppState, Status};

#[derive(Parser)]
#[command(name = "vulcand", version, about = "Control this PC from your iPhone")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon (default)
    Serve {
        /// Don't show the tray icon
        #[arg(long)]
        no_tray: bool,
    },
    /// Pair a phone: prints a QR code to scan in the Vulcan app
    Pair {
        /// Name for the device (default: iPhone, iPhone 2, ...)
        #[arg(long)]
        name: Option<String>,
    },
    /// List paired devices
    Devices,
    /// Unpair a device
    Revoke { name: String },
    /// Print the udev rule that lets vulcand switch the [[usb]] devices in config.toml
    UdevRule,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let cli = Cli::parse();
    let paths = Paths::new()?;
    let cfg = Config::load(&paths)?;
    let host = util::hostname();

    match cli.cmd.unwrap_or(Cmd::Serve { no_tray: false }) {
        Cmd::Serve { no_tray } => serve(paths, cfg, host, no_tray).await,
        Cmd::Pair { name } => {
            let tls = tls::load_or_create(&paths, &host)?;
            let p = pairing::create(&paths, &cfg, &tls.fingerprint, name.as_deref())?;
            println!("{}", pairing::terminal_qr(&p.url)?);
            println!("Scan this in the Vulcan app to pair “{}”.", p.name);
            println!("Or enter manually: {}", p.url);
            Ok(())
        }
        Cmd::Devices => {
            let store = DeviceStore::load(&paths.devices())?;
            if store.devices.is_empty() {
                println!("No paired devices. Run `vulcand pair`.");
            }
            for d in store.devices {
                println!("{}", d.name);
            }
            Ok(())
        }
        Cmd::UdevRule => {
            let out = std::process::Command::new("id").arg("-gn").output()?;
            let group = String::from_utf8_lossy(&out.stdout).trim().to_string();
            anyhow::ensure!(!group.is_empty(), "couldn't determine your group");
            print!("{}", usb::udev_rule(&cfg.usb, &group)?);
            Ok(())
        }
        Cmd::Revoke { name } => {
            let mut store = DeviceStore::load(&paths.devices())?;
            if store.revoke(&name) {
                store.save(&paths.devices())?;
                println!("Revoked “{name}”.");
            } else {
                println!("No device named “{name}”.");
            }
            Ok(())
        }
    }
}

async fn serve(paths: Paths, cfg: Config, host: String, no_tray: bool) -> Result<()> {
    let tls = tls::load_or_create(&paths, &host)?;
    let (layout, variant, options) = config::resolve_keyboard(&cfg.keyboard);
    info!(%layout, %variant, %options, "keyboard layout");
    let text = TextMapper::new(&layout, &variant, &options)?;
    let input = Input::new(cfg.pointer.clone(), text)?;

    let (status, _) = watch::channel(Status::default());
    let st = Arc::new(AppState {
        paths,
        fingerprint: tls.fingerprint.clone(),
        host: host.clone(),
        input: Mutex::new(input),
        status,
        cfg,
    });

    let _mdns = mdns::advertise(&host, st.cfg.port, &tls.fingerprint)
        .inspect_err(|e| warn!("mDNS advertisement failed: {e:#}"))
        .ok();
    if !no_tray {
        tray::spawn(st.clone());
    }
    info!(fingerprint = %tls.fingerprint, "vulcand ready");

    tokio::select! {
        r = server::run(st, &tls) => r,
        _ = tokio::signal::ctrl_c() => Ok(()),
    }
}
