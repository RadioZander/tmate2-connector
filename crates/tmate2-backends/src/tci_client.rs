//! TCI WebSocket client: connects, keeps [`SharedState`] in sync, sends
//! commands, negotiates TCI-X if configured, logs in when the server asks
//! (TCI-X §4.3), and reconnects on failure.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tokio::time::timeout_at;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

use tmate2_core::config::{RadioConfig, TcixMode, PASSWORD_ENV};
use tmate2_core::{Command, RadioState, Target};

use crate::pacing::Pacer;
use crate::tci::{self, TcixNegotiation};
use crate::{login, SharedState};

const RECONNECT_DELAY: Duration = Duration::from_secs(2);
/// After a refused or impossible login: the reference server blocks an
/// address after five failures in a minute, so don't hammer it.
const LOGIN_RETRY_DELAY: Duration = Duration::from_secs(30);
const TCIX_TIMEOUT: Duration = Duration::from_secs(2);
const IDLE_POLL: Duration = Duration::from_millis(250);
/// Periodic meter reports (TCI's, and TCI-X `meter`), not logged line by
/// line at -v.
const METERS: [&str; 4] = ["rx_sensors", "rx_channel_sensors", "tx_sensors", "meter"];
/// Why a connection ended, which decides when to try again.
#[derive(Debug, PartialEq)]
enum Ended {
    /// Lost, closed or settings changed: retry soon.
    Closed(String),
    /// The server wants a login we can't give or refused ours.
    Login(String),
    /// The server closed us for being idle (TCI-X §4.4): wait for the user.
    Idle,
}

/// The login key, kept so scrypt runs once per user and password.
type KeyCache = Option<(String, String, [u8; 32])>;

/// Run forever: connect, serve, reconnect. Commands arriving while
/// disconnected are dropped (they would be stale by the time we reconnect).
/// A change of settings on `cfg_rx` reconnects with the new settings.
pub async fn run(
    mut cfg_rx: watch::Receiver<RadioConfig>,
    state: SharedState,
    mut cmds: mpsc::UnboundedReceiver<Command>,
) {
    let mut keys: KeyCache = None;
    loop {
        let cfg = cfg_rx.borrow_and_update().clone();
        let url = format!("ws://{}:{}/", cfg.host, cfg.port);
        info!("connecting to TCI server {url}");
        let ended = match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((ws, _)) => {
                info!("connected to {url}");
                serve(ws, &cfg, &state, &mut cmds, &mut cfg_rx, &mut keys).await
            }
            Err(e) => {
                debug!("TCI connect failed: {e}");
                Ended::Closed(e.to_string())
            }
        };
        let delay = {
            let mut s = state.lock().unwrap();
            s.connected = false;
            s.tcix = None;
            s.control = None;
            // Nothing asked for on this connection will be answered now, and
            // the next server may answer reads where this one didn't.
            s.pending.clear();
            s.reading.clear();
            s.heard.clear();
            s.unreadable.clear();
            s.last_report.clear();
            match &ended {
                Ended::Closed(why) => {
                    warn!("TCI connection closed: {why}");
                    s.notice = None;
                    s.idle_closed = false;
                    RECONNECT_DELAY
                }
                Ended::Login(why) => {
                    error!("TCI-X login: {why}");
                    s.notice = Some("login".into());
                    LOGIN_RETRY_DELAY
                }
                Ended::Idle => {
                    warn!("the server closed the connection after the idle time; touch the TMATE-2 to reconnect");
                    s.notice = Some("idle".into());
                    s.idle_closed = true;
                    Duration::ZERO
                }
            }
        };
        // Wait for the user after an idle close (any input on the TMATE-2
        // clears `idle_closed`), or for new settings.
        while state.lock().unwrap().idle_closed && !cfg_rx.has_changed().unwrap_or(false) {
            while let Ok(Some(_)) = timeout_at(tokio::time::Instant::now() + IDLE_POLL, cmds.recv()).await {}
        }
        // Drop anything queued while we were down, then wait before retrying
        // (cut short by new settings).
        let deadline = tokio::time::Instant::now() + delay;
        let mut cfg_alive = true;
        loop {
            tokio::select! {
                r = timeout_at(deadline, cmds.recv()) => {
                    if !matches!(r, Ok(Some(_))) {
                        break;
                    }
                }
                c = cfg_rx.changed(), if cfg_alive => match c {
                    Ok(()) => break,
                    // Nobody can change settings any more: just wait.
                    Err(_) => cfg_alive = false,
                },
            }
        }
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The login key for `cfg`'s account, from the cache or computed (slowly,
/// off the async threads). `None` with no user or password set.
async fn login_key(cfg: &RadioConfig, keys: &mut KeyCache) -> Option<(String, [u8; 32])> {
    let (user, password) = cfg.credentials()?;
    if let Some((u, p, k)) = keys {
        if *u == user && *p == password {
            return Some((user, *k));
        }
    }
    let (u, p) = (user.clone(), password.clone());
    let key = tokio::task::spawn_blocking(move || login::key(&u, &p))
        .await
        .ok()?;
    *keys = Some((user.clone(), password, key));
    Some((user, key))
}

async fn serve(
    ws: Ws,
    cfg: &RadioConfig,
    state: &SharedState,
    cmds: &mut mpsc::UnboundedReceiver<Command>,
    cfg_rx: &mut watch::Receiver<RadioConfig>,
    keys: &mut KeyCache,
) -> Ended {
    let (mut tx, mut rx) = ws.split();
    let mut ready = false;
    let mut tcix = TcixNegotiation::default();
    let mut tcix_deadline: Option<tokio::time::Instant> = None;
    let mut cfg_alive = true;
    let mut user: Option<String> = None;
    let mut pacer = Pacer::new(cfg.pacing, &cfg.pacing_tuning);
    // Values read by the once-a-second poll and not yet answered: their
    // answers, like the reads, are left out of the -v log unless they
    // change something.
    let mut polled: HashSet<Target> = HashSet::new();

    macro_rules! send_or_end {
        ($line:expr) => {
            if let Err(e) = send(&mut tx, $line, true).await {
                return Ended::Closed(e);
            }
        };
        ($line:expr, $logged:expr) => {
            if let Err(e) = send(&mut tx, $line, $logged).await {
                return Ended::Closed(e);
            }
        };
    }

    loop {
        let tcix_wait = async {
            match tcix_deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending().await,
            }
        };
        let pace_wait = async {
            match pacer.next_due() {
                Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            msg = rx.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return Ended::Closed(e.to_string()),
                    None => return Ended::Closed("server closed the connection".into()),
                };
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => return Ended::Closed("server closed the connection".into()),
                    _ => continue, // binary audio / IQ / spectrum, ping/pong
                };
                let now = Instant::now();
                // With -v: what came back, so the echoes can be read against
                // the "TCI >" lines. The meter reports, several a second,
                // are left out, and so are answers to the poll that change
                // nothing.
                let logged = {
                    let s = state.lock().unwrap();
                    tci::split(&text)
                        .any(|m| !METERS.contains(&m.name.as_str()) && !quiet_poll_answer(&s, &mut polled, &m))
                };
                if logged {
                    debug!("TCI < {}", text.trim());
                }
                let mut out: Vec<String> = Vec::new();
                let mut nonce: Option<String> = None;
                let mut end: Option<Ended> = None;
                {
                    let mut s = state.lock().unwrap();
                    for m in tci::split(&text) {
                        pacer.echoed(&m.name, &m.args, now);
                        if tcix.feed(&m) {
                            if tcix.done {
                                if s.tcix.is_none() {
                                    info!("TCI-X negotiated: {} capabilities", tcix.manifest.len());
                                }
                                s.tcix = Some(tcix.manifest.clone());
                                tcix_deadline = None;
                            }
                            continue;
                        }
                        let arg = |i: usize| m.args.get(i).map(String::as_str).unwrap_or("");
                        match (m.name.as_str(), arg(0), arg(1)) {
                            ("login_required", n, _) => {
                                nonce = Some(n.to_string());
                                continue;
                            }
                            ("login", "ok", role) => {
                                info!("TCI-X login accepted as {} ({role})", user.as_deref().unwrap_or("?"));
                                s.login_user = user.clone();
                                s.role = Some(role.to_string());
                                continue;
                            }
                            ("error", "login", _) => {
                                end = Some(Ended::Login(format!(
                                    "the server refused the login: {}",
                                    tci::pct_decode(&m.args.get(2..).unwrap_or_default().join(","))
                                )));
                                break;
                            }
                            ("error", "control", "idle") => {
                                end = Some(Ended::Idle);
                                break;
                            }
                            _ => {}
                        }
                        let was_ready = s.connected;
                        tci::apply(&mut s, &m);
                        s.note_heard(&m.name, &m.args, now);
                        if m.name == "ready" && !was_ready {
                            ready = true;
                            s.notice = None;
                            s.idle_closed = false;
                            info!(
                                "TCI ready: {} ({})",
                                s.device.as_deref().unwrap_or("unknown device"),
                                s.protocol.as_deref().unwrap_or("unknown protocol")
                            );
                            out.extend(tci::encode(&Command::EnableMeters));
                            out.extend(tci::encode(&Command::EnableTxMeters));
                            if cfg.tcix != TcixMode::Off {
                                out.push("tcix:1.0;".into());
                                tcix_deadline = Some(tokio::time::Instant::now() + TCIX_TIMEOUT);
                            }
                        }
                    }
                }
                if let Some(end) = end {
                    return end;
                }
                if let Some(nonce) = nonce {
                    state.lock().unwrap().notice = Some("login".into());
                    let Some((u, key)) = login_key(cfg, keys).await else {
                        return Ended::Login(format!(
                            "the server asks for a login: set user and password under [radio] \
                             (or the password in {PASSWORD_ENV})"
                        ));
                    };
                    debug!("logging in as {u}");
                    user = Some(u.clone());
                    // Not through `send`: the line holds the proof.
                    if let Err(e) = tx.send(Message::text(login::login_line(&u, &key, &nonce))).await {
                        return Ended::Closed(e.to_string());
                    }
                }
                for line in out {
                    send_or_end!(line);
                }
            }
            cmd = cmds.recv() => {
                let Some(first) = cmd else { return Ended::Closed("command channel closed".into()) };
                if !ready {
                    continue;
                }
                let now = Instant::now();
                let lines = {
                    let mut s = state.lock().unwrap();
                    let go: Vec<Command> = newest(first, cmds, &s)
                        .into_iter()
                        .filter_map(|c| pacer.offer(c, now))
                        .collect();
                    lines_for(&go, &mut s, &mut polled)
                };
                for (line, logged) in lines {
                    send_or_end!(line, logged);
                }
            }
            _ = pace_wait => {
                let lines = {
                    let mut s = state.lock().unwrap();
                    let go = pacer.due(Instant::now());
                    lines_for(&go, &mut s, &mut polled)
                };
                for (line, logged) in lines {
                    send_or_end!(line, logged);
                }
            }
            changed = cfg_rx.changed(), if cfg_alive => {
                match changed {
                    Ok(()) => return Ended::Closed("radio settings changed; reconnecting".into()),
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

async fn send(
    tx: &mut futures_util::stream::SplitSink<Ws, Message>,
    line: String,
    logged: bool,
) -> Result<(), String> {
    if logged {
        debug!("TCI > {line}");
    }
    tx.send(Message::text(line)).await.map_err(|e| e.to_string())
}

/// The lines for commands about to go, each with whether -v logs it (not
/// the poll's reads), noting each as sent so its report can be told from
/// an echo still on its way (see [`RadioState::sent`]).
fn lines_for(cmds: &[Command], state: &mut RadioState, polled: &mut HashSet<Target>) -> Vec<(String, bool)> {
    cmds.iter()
        .filter_map(|c| {
            let line = tci::encode_for(c, state)?;
            state.sent(c);
            if let Command::Poll(t) = c {
                polled.insert(*t);
                return Some((line, false));
            }
            Some((line, true))
        })
        .collect()
}

/// `m` answers a poll read and says what we already have: not worth a log
/// line. A poll answer that changes something is logged.
fn quiet_poll_answer(state: &RadioState, polled: &mut HashSet<Target>, m: &tci::Msg) -> bool {
    let Some(t) = Target::of_report(&m.name, &m.args) else {
        return false;
    };
    if !polled.remove(&t) {
        return false;
    }
    let mut probe = state.clone();
    tci::apply(&mut probe, m);
    probe == *state
}

/// Take `first` plus everything already queued, keeping only the newest VFO
/// frequency and the newest of each knob value (AF gain, filter width, …)
/// per transceiver, so fast turning never builds a backlog. With
/// TCI-X, anything the server's manifest doesn't take is dropped here too.
fn newest(first: Command, cmds: &mut mpsc::UnboundedReceiver<Command>, state: &RadioState) -> Vec<Command> {
    let mut batch = vec![first];
    while let Ok(c) = cmds.try_recv() {
        batch.push(c);
    }
    let mut out: Vec<Command> = Vec::new();
    for (i, c) in batch.iter().enumerate() {
        if matches!(c, Command::Vfo { .. }) || c.knob_value() {
            let target = c.target();
            if batch[i + 1..].iter().any(|later| later.target() == target) {
                continue;
            }
        }
        if !state.allows(c) {
            debug!("not sent (not in the TCI-X manifest): {c:?}");
            continue;
        }
        out.push(c.clone());
    }
    out
}

/// [`newest`], as the lines to send.
#[cfg(test)]
fn coalesce(first: Command, cmds: &mut mpsc::UnboundedReceiver<Command>, state: &RadioState) -> Vec<String> {
    newest(first, cmds, state)
        .iter()
        .filter_map(|c| tci::encode_for(c, state))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use tmate2_core::config::Backend;
    use tokio::net::TcpListener;

    /// The worked example in TCI-X §4.3.
    const NONCE: &str = "00112233445566778899aabbccddeeff";
    const PROOF: &str = "6a83548bc3fdc8eb8f11f1d33bee6884231769102e96a05c0d188d3b2ec66974";

    fn cfg(port: u16, password: &str) -> RadioConfig {
        RadioConfig {
            backend: Backend::Tci,
            host: "127.0.0.1".into(),
            port,
            tcix: TcixMode::Auto,
            pacing: tmate2_core::config::Pacing::Auto,
            poll_ms: 1000,
            pacing_tuning: Default::default(),
            trx: 0,
            user: Some("g0kad".into()),
            password: Some(password.into()),
        }
    }

    async fn wait_for(state: &SharedState, what: &str, f: impl Fn(&RadioState) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !f(&state.lock().unwrap()) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    type ServerWs = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    async fn accept(l: &TcpListener) -> ServerWs {
        let (tcp, _) = l.accept().await.unwrap();
        tokio_tungstenite::accept_async(tcp).await.unwrap()
    }

    async fn say(ws: &mut ServerWs, text: &str) {
        ws.send(Message::text(text)).await.unwrap();
    }

    /// Next text frame from the client.
    async fn hear(ws: &mut ServerWs) -> String {
        loop {
            match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => return t.to_string(),
                Ok(Some(Ok(_))) => continue,
                other => panic!("no text from the client: {other:?}"),
            }
        }
    }

    /// Start the client against a fake server on a free local port.
    async fn start(password: &str) -> (TcpListener, SharedState, watch::Sender<RadioConfig>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cfg_tx, cfg_rx) = watch::channel(cfg(l.local_addr().unwrap().port(), password));
        let state: SharedState = Arc::new(Mutex::new(RadioState::default()));
        let (_cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Command>();
        tokio::spawn({
            let state = state.clone();
            async move {
                // Keep the command channel open for the client's lifetime.
                let _cmd_tx = _cmd_tx;
                run(cfg_rx, state, cmd_rx).await
            }
        });
        (l, state, cfg_tx)
    }

    #[tokio::test]
    async fn logs_in_takes_part_in_control_and_waits_after_idle() {
        let (l, state, _cfg) = start("correct horse").await;
        let mut ws = accept(&l).await;
        say(
            &mut ws,
            &format!("protocol:kadrad-hub,2.0;login_required:{NONCE};"),
        )
        .await;
        assert_eq!(hear(&mut ws).await, format!("login:g0kad,{PROOF};"));
        say(
            &mut ws,
            "login:ok,operate;device:Icom IC-7100;vfo:0,0,14074000;ready;",
        )
        .await;
        // Meters first, then the TCI-X opt-in.
        assert_eq!(hear(&mut ws).await, "rx_sensors_enable:true,200;");
        assert_eq!(hear(&mut ws).await, "tx_sensors_enable:true,200;");
        assert_eq!(hear(&mut ws).await, "tcix:1.0;");
        say(
            &mut ws,
            "tcix:1.0;cap:trx,rw,bool;cap:control_request,w,action;cap_end;control:g0kad,1760729000;",
        )
        .await;
        wait_for(&state, "control", |s| s.control.is_some() && s.tcix.is_some()).await;
        {
            let s = state.lock().unwrap();
            assert!(s.connected && s.in_control());
            assert_eq!(s.role.as_deref(), Some("operate"));
        }

        // Idle: the server closes us, and we stay away until the user is back.
        say(&mut ws, "error:control,idle,Idle for 15 minutes;").await;
        drop(ws);
        wait_for(&state, "idle", |s| {
            s.idle_closed && s.notice.as_deref() == Some("idle")
        })
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(1500), l.accept())
                .await
                .is_err(),
            "reconnected by itself after an idle close"
        );
        // The engine clears this on any TMATE-2 input.
        state.lock().unwrap().idle_closed = false;
        let mut ws = tokio::time::timeout(Duration::from_secs(5), accept(&l))
            .await
            .expect("reconnects once the user is back");
        say(
            &mut ws,
            &format!("protocol:kadrad-hub,2.0;login_required:{NONCE};"),
        )
        .await;
        assert_eq!(hear(&mut ws).await, format!("login:g0kad,{PROOF};"));
    }

    #[tokio::test]
    async fn a_refused_login_backs_off() {
        let (l, state, _cfg) = start("wrong").await;
        let mut ws = accept(&l).await;
        say(
            &mut ws,
            &format!("protocol:kadrad-hub,2.0;login_required:{NONCE};"),
        )
        .await;
        assert!(hear(&mut ws).await.starts_with("login:g0kad,"));
        say(&mut ws, "error:login,access,Unknown user or wrong password;").await;
        wait_for(&state, "login notice", |s| {
            s.notice.as_deref() == Some("login") && !s.connected
        })
        .await;
        drop(ws);
        assert!(
            tokio::time::timeout(Duration::from_secs(3), l.accept())
                .await
                .is_err(),
            "retried a refused login within 3 s"
        );
    }

    #[tokio::test]
    async fn no_credentials_for_a_login_server() {
        let (l, state, cfg_tx) = start("").await;
        let mut ws = accept(&l).await;
        say(
            &mut ws,
            &format!("protocol:kadrad-hub,2.0;login_required:{NONCE};"),
        )
        .await;
        wait_for(&state, "login notice", |s| s.notice.as_deref() == Some("login")).await;
        // New settings retry at once, without waiting out the back-off.
        let mut c = cfg(l.local_addr().unwrap().port(), "correct horse");
        c.trx = 0;
        cfg_tx.send(c).unwrap();
        let mut ws2 = tokio::time::timeout(Duration::from_secs(5), accept(&l))
            .await
            .expect("new settings reconnect at once");
        say(
            &mut ws2,
            &format!("protocol:kadrad-hub,2.0;login_required:{NONCE};"),
        )
        .await;
        assert_eq!(hear(&mut ws2).await, format!("login:g0kad,{PROOF};"));
        let _ = ws.close(None).await;
    }

    #[test]
    fn a_dial_is_paced_by_the_servers_echoes() {
        let tuning = tmate2_core::config::PacingTuning::default();
        let pace = Duration::from_millis(tuning.min_interval_ms);
        let echo_wait = Duration::from_millis(tuning.echo_wait_ms);
        use tmate2_core::config::Pacing;
        let vfo = |hz| Command::Vfo { trx: 0, vfo: 0, hz };
        let ms = Duration::from_millis;
        let t0 = Instant::now();
        // Pacing "on": what "auto" does once a server falls behind.
        let mut p = Pacer::new(Pacing::On, &tuning);
        let echoed = |p: &mut Pacer, line: &str| {
            let m = tci::parse(line).unwrap();
            p.echoed(&m.name, &m.args, t0);
        };
        // The first step goes at once; the rest of a fast spin waits, and
        // only the newest is kept.
        assert_eq!(p.offer(vfo(10), t0), Some(vfo(10)));
        assert_eq!(p.offer(vfo(20), t0 + ms(5)), None);
        assert_eq!(p.offer(vfo(30), t0 + ms(10)), None);
        assert!(p.due(t0 + ms(100)).is_empty(), "not before the echo");
        assert_eq!(p.next_due(), Some(t0 + echo_wait));
        // The echo of the first lets the newest go, once the pace allows.
        echoed(&mut p, "vfo:0,1,7000000;");
        assert!(p.due(t0 + ms(100)).is_empty(), "the other VFO's echo");
        echoed(&mut p, "vfo:0,0,10;");
        assert_eq!(p.due(t0 + ms(100)), vec![vfo(30)]);
        assert_eq!(p.next_due(), None);
        // A server that never echoes: one value every echo_wait.
        assert_eq!(p.offer(vfo(40), t0 + ms(120)), None);
        assert!(p.due(t0 + ms(300)).is_empty());
        assert_eq!(p.due(t0 + ms(100) + echo_wait), vec![vfo(40)]);
        // Echoed at once, the next still waits out the pace.
        echoed(&mut p, "vfo:0,0,40;");
        let t1 = t0 + ms(100) + echo_wait;
        assert_eq!(p.offer(vfo(50), t1 + ms(10)), None);
        assert_eq!(p.due(t1 + pace), vec![vfo(50)]);
        // A knob has a slot of its own; keys and keying aren't paced.
        let af = |pct| Command::AfGain { trx: 0, pct };
        assert_eq!(p.offer(af(1), t1), Some(af(1)));
        assert_eq!(p.offer(af(2), t1), None);
        echoed(&mut p, "af_gain:0,1;");
        assert_eq!(p.due(t1 + pace), vec![af(2)]);
        let key = Command::Mox { trx: 0, on: true };
        assert_eq!(p.offer(key.clone(), t1), Some(key.clone()));
        assert_eq!(p.offer(key.clone(), t1), Some(key));
    }

    #[test]
    fn coalesce_keeps_each_receivers_values() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(Command::RxVolume { trx: 1, db: -20.0 }).unwrap();
        tx.send(Command::RxVolume { trx: 0, db: -11.0 }).unwrap();
        let out = coalesce(
            Command::RxVolume { trx: 0, db: -10.0 },
            &mut rx,
            &RadioState::default(),
        );
        assert_eq!(out, vec!["rx_volume:1,0,-20.00;", "rx_volume:0,0,-11.00;"]);
    }

    #[test]
    fn coalesce_keeps_latest_knob_value_only() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(Command::AfGain { trx: 0, pct: 12 }).unwrap();
        tx.send(Command::Mute(true)).unwrap();
        tx.send(Command::RfGain { trx: 0, pct: 90 }).unwrap();
        tx.send(Command::AfGain { trx: 0, pct: 13 }).unwrap();
        let out = coalesce(
            Command::AfGain { trx: 0, pct: 11 },
            &mut rx,
            &RadioState::default(),
        );
        assert_eq!(out, vec!["mute:true;", "rf_gain:0,90;", "af_gain:0,13;"]);
    }

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
            &RadioState::default(),
        );
        assert_eq!(out, vec!["vfo:0,0,3;", "mute:true;", "vfo:0,1,9;"]);
    }
}
