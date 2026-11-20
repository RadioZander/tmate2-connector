//! TCI WebSocket client: connects, keeps [`SharedState`] in sync, sends
//! commands, negotiates TCI-X if configured, and reconnects on failure.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tokio::time::timeout_at;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use tmate2_core::config::{RadioConfig, TcixMode};
use tmate2_core::Command;

use crate::tci::{self, TcixNegotiation};
use crate::SharedState;

const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const TCIX_TIMEOUT: Duration = Duration::from_secs(2);

/// Run forever: connect, serve, reconnect. Commands arriving while
/// disconnected are dropped (they would be stale by the time we reconnect).
/// A change of settings on `cfg_rx` reconnects with the new settings.
pub async fn run(
    mut cfg_rx: watch::Receiver<RadioConfig>,
    state: SharedState,
    mut cmds: mpsc::UnboundedReceiver<Command>,
) {
    loop {
        let cfg = cfg_rx.borrow_and_update().clone();
        let url = format!("ws://{}:{}/", cfg.host, cfg.port);
        info!("connecting to TCI server {url}");
        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((ws, _)) => {
                info!("connected to {url}");
                if let Err(e) = serve(ws, &cfg, &state, &mut cmds, &mut cfg_rx).await {
                    warn!("TCI connection closed: {e}");
                }
            }
            Err(e) => debug!("TCI connect failed: {e}"),
        }
        {
            let mut s = state.lock().unwrap();
            s.connected = false;
            s.tcix = None;
        }
        // Drop anything queued while we were down, then wait before retrying.
        let deadline = tokio::time::Instant::now() + RECONNECT_DELAY;
        while let Ok(Some(_)) = timeout_at(deadline, cmds.recv()).await {}
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(
    ws: Ws,
    cfg: &RadioConfig,
    state: &SharedState,
    cmds: &mut mpsc::UnboundedReceiver<Command>,
    cfg_rx: &mut watch::Receiver<RadioConfig>,
) -> Result<(), String> {
    let (mut tx, mut rx) = ws.split();
    let mut ready = false;
    let mut tcix = TcixNegotiation::default();
    let mut tcix_deadline: Option<tokio::time::Instant> = None;
    let mut cfg_alive = true;

    loop {
        let tcix_wait = async {
            match tcix_deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            msg = rx.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return Err(e.to_string()),
                    None => return Err("server closed the connection".into()),
                };
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => return Err("server closed the connection".into()),
                    _ => continue, // binary audio / IQ / spectrum, ping/pong
                };
                let now = Instant::now();
                let mut out: Vec<String> = Vec::new();
                {
                    let mut s = state.lock().unwrap();
                    for m in tci::split(&text) {
                        if tcix.feed(&m) {
                            if tcix.done {
                                info!("TCI-X negotiated: {} capabilities", tcix.caps.len());
                                s.tcix = Some(tcix.caps.clone());
                                tcix_deadline = None;
                            }
                            continue;
                        }
                        let was_ready = s.connected;
                        tci::apply(&mut s, &m, now);
                        if m.name == "ready" && !was_ready {
                            ready = true;
                            info!(
                                "TCI ready: {} ({})",
                                s.device.as_deref().unwrap_or("unknown device"),
                                s.protocol.as_deref().unwrap_or("unknown protocol")
                            );
                            out.extend(tci::encode(&Command::EnableMeters));
                            if cfg.tcix != TcixMode::Off {
                                out.push("tcix:1.0;".into());
                                tcix_deadline = Some(tokio::time::Instant::now() + TCIX_TIMEOUT);
                            }
                        }
                    }
                }
                for line in out {
                    send(&mut tx, line).await?;
                }
            }
            cmd = cmds.recv() => {
                let Some(first) = cmd else { return Err("command channel closed".into()) };
                if !ready {
                    continue;
                }
                for line in coalesce(first, cmds) {
                    send(&mut tx, line).await?;
                }
            }
            changed = cfg_rx.changed(), if cfg_alive => {
                match changed {
                    Ok(()) => return Err("radio settings changed; reconnecting".into()),
                    // Nobody can change settings any more: keep running.
                    Err(_) => cfg_alive = false,
                }
            }
            _ = tcix_wait => {
                tcix_deadline = None;
                if cfg.tcix == TcixMode::On {
                    warn!("server did not answer the TCI-X handshake; using plain TCI");
                } else {
                    debug!("no TCI-X reply; using plain TCI");
                }
            }
        }
    }
}

async fn send(tx: &mut futures_util::stream::SplitSink<Ws, Message>, line: String) -> Result<(), String> {
    debug!("TCI > {line}");
    tx.send(Message::text(line)).await.map_err(|e| e.to_string())
}

/// Take `first` plus everything already queued, keeping only the newest VFO
/// frequency per (trx, vfo) so fast tuning never builds a backlog.
fn coalesce(first: Command, cmds: &mut mpsc::UnboundedReceiver<Command>) -> Vec<String> {
    let mut batch = vec![first];
    while let Ok(c) = cmds.try_recv() {
        batch.push(c);
    }
    let mut out: Vec<String> = Vec::new();
    for (i, c) in batch.iter().enumerate() {
        if let Command::Vfo { trx, vfo, .. } = c {
            let superseded = batch[i + 1..]
                .iter()
                .any(|later| matches!(later, Command::Vfo { trx: t, vfo: v, .. } if t == trx && v == vfo));
            if superseded {
                continue;
            }
        }
        out.extend(tci::encode(c));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesce_keeps_latest_vfo_only() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        for hz in [1, 2, 3] {
            tx.send(Command::Vfo { trx: 0, vfo: 0, hz }).unwrap();
        }
        tx.send(Command::Mute(true)).unwrap();
        tx.send(Command::Vfo {
            trx: 0,
            vfo: 1,
            hz: 9,
        })
        .unwrap();
        let out = coalesce(
            Command::Vfo {
                trx: 0,
                vfo: 0,
                hz: 0,
            },
            &mut rx,
        );
        assert_eq!(out, vec!["vfo:0,0,3;", "mute:true;", "vfo:0,1,9;"]);
    }
}
