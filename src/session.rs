//! Per-tab WebSocket session.
//!
//! One `Session` lives in `AppState` per open token. It owns a mpsc pair:
//!   - inbound  (tokio task → UI): `SessionEvent` items produced by the WS loop
//!   - outbound (UI → tokio task): encoded JSON strings to send as WS frames
//!
//! Keeping these as *synchronous* std mpsc for inbound (so `egui::App::update`
//! can drain them without an async context) and tokio::mpsc for outbound (so
//! the UI's `.send(...)` is still synchronous but the task can `.recv().await`)
//! is deliberate — see PLAN.md "egui + tokio".

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::sync::mpsc as std_mpsc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::mpsc as tokio_mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::diff_patch::{compute_diff, Fragment};
use crate::protocol::{
    self, file_flags, FileActionEntry, FileConsumeEntry, FileConsumeResult, Packet,
};
use crate::settings::Settings;
use crate::token::{fletcher32, Token};

#[derive(Debug)]
pub enum SessionEvent {
    Connected,
    Packet(Packet),
    Closed(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Connecting,
    /// Handshake complete but no `file:host` / `terminal:host` peer yet.
    Waiting,
    /// At least one host peer is present.
    Connected,
    LostConnection,
    Errored(String),
}

#[derive(Clone, Debug, Default)]
pub struct SessionInfo {
    pub computer_id: Option<i64>,
    pub label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FileModel {
    pub name: String,
    pub read_only: bool,
    pub is_new: bool,
    /// Last contents we know the remote has.
    pub remote_contents: String,
    pub remote_checksum: u32,
    /// The text the user is editing. Starts equal to `remote_contents`.
    pub buffer: String,
    /// While a save is in flight, these hold what we sent. Cleared on
    /// `FileConsume::Ok`.
    pub pending_update: Option<PendingUpdate>,
}

#[derive(Clone, Debug)]
pub struct PendingUpdate {
    pub contents: String,
    pub checksum: u32,
}

impl FileModel {
    pub fn modified(&self) -> bool { self.buffer != self.remote_contents }
}

#[derive(Clone, Debug)]
pub struct Notification {
    pub id: String,
    pub kind: NotificationKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationKind { Ok, Warn, Error }

pub struct Session {
    pub token: Token,
    pub status: SessionStatus,
    pub info: SessionInfo,
    pub files: Vec<FileModel>,
    pub active_file: Option<String>,
    pub notifications: Vec<Notification>,

    inbound_rx: std_mpsc::Receiver<SessionEvent>,
    outbound_tx: tokio_mpsc::UnboundedSender<String>,
    _task: JoinHandle<()>,

    /// Set once we've seen a host peer in a ConnectionUpdate. Used to decide
    /// whether to render the "lost connection" vs "waiting for computer" view.
    pub has_connected: bool,
}

impl Session {
    pub fn spawn(
        token: Token,
        settings: &Settings,
        runtime: &Handle,
        egui_ctx: egui::Context,
    ) -> Self {
        let url = build_url(&token, &settings.server_host);

        let (in_tx, in_rx) = std_mpsc::channel();
        let (out_tx, out_rx) = tokio_mpsc::unbounded_channel();

        let task = runtime.spawn(run_ws(url, in_tx, out_rx, egui_ctx));

        Self {
            token,
            status: SessionStatus::Connecting,
            info: SessionInfo::default(),
            files: Vec::new(),
            active_file: None,
            notifications: Vec::new(),
            inbound_rx: in_rx,
            outbound_tx: out_tx,
            _task: task,
            has_connected: false,
        }
    }

    /// Apply any pending WS events to session state. Call once per egui frame
    /// for each session. Returns true if the session changed (caller may want
    /// to re-persist).
    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        while let Ok(ev) = self.inbound_rx.try_recv() {
            changed = true;
            match ev {
                SessionEvent::Connected => {
                    self.status = SessionStatus::Waiting;
                }
                SessionEvent::Closed(reason) => {
                    self.status = if self.has_connected {
                        SessionStatus::LostConnection
                    } else {
                        SessionStatus::Errored(reason)
                    };
                }
                SessionEvent::Error(reason) => {
                    self.status = SessionStatus::Errored(reason);
                }
                SessionEvent::Packet(p) => self.handle_packet(p),
            }
        }
        changed
    }

    fn handle_packet(&mut self, p: Packet) {
        match p {
            Packet::ConnectionPing => {
                // Echo right back — the server hangs up clients that don't.
                let _ = self.send(&Packet::ConnectionPing);
            }
            Packet::ConnectionUpdate { capabilities, .. } => {
                let has_host = capabilities.iter().any(|c| c == "terminal:host" || c == "file:host");
                if has_host {
                    self.status = SessionStatus::Connected;
                    self.has_connected = true;
                } else if self.has_connected {
                    self.status = SessionStatus::LostConnection;
                    self.info = SessionInfo::default();
                } else {
                    self.status = SessionStatus::Waiting;
                }
            }
            Packet::ConnectionAbuse { message } => {
                self.push_notification(NotificationKind::Warn, "abuse", message);
            }
            Packet::TerminalInfo { id, label } => {
                self.info = SessionInfo { computer_id: id, label };
            }
            Packet::TerminalContents(_) => { /* ignored in file-editor MVP */ }
            Packet::FileAction { actions, .. } => self.apply_file_actions(actions),
            Packet::FileConsume { files, .. } => self.apply_file_consume(files),
            Packet::FileListing { .. } | Packet::FileRequest { .. } => {
                // We don't act as a file:host peer.
            }
            Packet::TerminalEvents { .. } => { /* outbound-only; ignore echoes */ }
        }
    }

    fn apply_file_actions(&mut self, actions: Vec<FileActionEntry>) {
        for a in actions {
            match a.action {
                0 => {
                    // Replace — either new file or force-update.
                    let read_only = a.flags & file_flags::READ_ONLY != 0;
                    let is_new = a.flags & file_flags::NEW != 0;
                    let open_flag = a.flags & file_flags::OPEN != 0;
                    let force = a.flags & file_flags::FORCE != 0;
                    let contents = a.contents.unwrap_or_default();
                    let checksum = fletcher32(&contents);

                    if let Some(file) = self.files.iter_mut().find(|f| f.name == a.file) {
                        if force || file.remote_checksum == a.checksum {
                            file.remote_contents = contents.clone();
                            file.remote_checksum = checksum;
                            file.buffer = contents;
                            file.is_new = false;
                            file.read_only = read_only;
                            self.remove_notification(&a.file, "update");
                        } else {
                            self.push_notification(
                                NotificationKind::Warn,
                                &format!("{}\0update", a.file),
                                format!("{} changed on the remote.", a.file),
                            );
                        }
                    } else {
                        let file = FileModel {
                            name: a.file.clone(),
                            read_only,
                            is_new,
                            remote_contents: contents.clone(),
                            remote_checksum: checksum,
                            buffer: contents,
                            pending_update: None,
                        };
                        self.files.push(file);
                    }
                    self.files.sort_by(|x, y| x.name.cmp(&y.name));
                    if open_flag { self.active_file = Some(a.file); }
                }
                2 => {
                    self.files.retain(|f| f.name != a.file);
                    if self.active_file.as_deref() == Some(a.file.as_str()) {
                        self.active_file = None;
                    }
                }
                _ => {
                    // action=1 (Patch) from server→client is possible per protocol
                    // but the CC host only sends Replace. If we start seeing
                    // Patch packets we'll need to add the applier; see PLAN.md.
                    log::warn!("file action {} not yet implemented", a.action);
                }
            }
        }
    }

    fn apply_file_consume(&mut self, files: Vec<FileConsumeEntry>) {
        for info in files {
            let Some(file) = self.files.iter_mut().find(|f| f.name == info.file) else { continue; };
            match info.result {
                FileConsumeResult::Ok => {
                    if let Some(pending) = file.pending_update.take() {
                        if pending.checksum == info.checksum {
                            file.remote_contents = pending.contents;
                            file.remote_checksum = pending.checksum;
                            self.remove_notification(&info.file, "update");
                        } else {
                            self.push_notification(
                                NotificationKind::Warn,
                                &format!("{}\0update", info.file),
                                format!("{} changed on the remote.", info.file),
                            );
                        }
                    }
                }
                FileConsumeResult::Reject => self.push_notification(
                    NotificationKind::Error,
                    &format!("{}\0update", info.file),
                    format!("{} couldn't be saved (remote was changed).", info.file),
                ),
                FileConsumeResult::Failure => self.push_notification(
                    NotificationKind::Error,
                    &format!("{}\0update", info.file),
                    format!("{} failed to save (read only?).", info.file),
                ),
            }
        }
    }

    pub fn save_active(&mut self, trim_whitespace: bool) -> Result<()> {
        let Some(name) = self.active_file.clone() else { return Ok(()) };
        self.save_file(&name, trim_whitespace)
    }

    pub fn save_file(&mut self, name: &str, trim_whitespace: bool) -> Result<()> {
        let Some(file) = self.files.iter_mut().find(|f| f.name == name) else { return Ok(()) };
        if file.read_only { return Ok(()) }

        let mut contents = file.buffer.clone();
        if trim_whitespace {
            contents = contents.lines()
                .map(|l| l.trim_end())
                .collect::<Vec<_>>()
                .join("\n");
            // Preserve trailing newline if the buffer had one.
            if file.buffer.ends_with('\n') && !contents.ends_with('\n') { contents.push('\n'); }
        }
        let new_checksum = fletcher32(&contents);

        let entry = if file.is_new {
            FileActionEntry {
                file: file.name.clone(),
                checksum: file.remote_checksum,
                flags: 0,
                action: 0, // Replace
                contents: Some(contents.clone()),
                delta: None,
            }
        } else {
            let delta = compute_diff(&file.remote_contents, &contents);
            FileActionEntry {
                file: file.name.clone(),
                checksum: file.remote_checksum,
                flags: 0,
                action: 1, // Patch
                contents: None,
                delta: Some(delta),
            }
        };

        file.pending_update = Some(PendingUpdate { contents, checksum: new_checksum });
        // Update buffer to the possibly-trimmed version so dirty state clears
        // once we get the Ok back.
        file.buffer = file.pending_update.as_ref().unwrap().contents.clone();
        let packet = Packet::FileAction { id: 0, actions: vec![entry] };
        self.send(&packet)
    }

    pub fn send(&self, packet: &Packet) -> Result<()> {
        let s = protocol::encode(packet)?;
        if s.len() > protocol::MAX_PACKET_SIZE {
            log::warn!("outbound packet {} bytes exceeds MAX_PACKET_SIZE ({}); server may drop it",
                s.len(), protocol::MAX_PACKET_SIZE);
        }
        self.outbound_tx.send(s).map_err(|_| anyhow::anyhow!("session task is gone"))?;
        Ok(())
    }

    pub fn push_notification(&mut self, kind: NotificationKind, id: &str, message: impl Into<String>) {
        self.notifications.retain(|n| n.id != id);
        self.notifications.push(Notification { id: id.into(), kind, message: message.into() });
    }

    pub fn remove_notification(&mut self, file: &str, category: &str) {
        let id = format!("{file}\0{category}");
        self.notifications.retain(|n| n.id != id);
    }

    #[allow(dead_code)]
    pub fn pending_patch_bytes(_delta: &[Fragment]) -> usize { 0 }
}

fn build_url(token: &Token, server_host: &str) -> String {
    if std::env::var("SAMARI_DEV").ok().as_deref() == Some("1") {
        format!("ws://localhost:8080/connect?id={}&capabilities=file:edit", token)
    } else {
        format!("wss://{}/connect?id={}&capabilities=file:edit", server_host, token)
    }
}

async fn run_ws(
    url: String,
    in_tx: std_mpsc::Sender<SessionEvent>,
    mut out_rx: tokio_mpsc::UnboundedReceiver<String>,
    egui_ctx: egui::Context,
) {
    let ws = match tokio_tungstenite::connect_async(&url).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            let _ = in_tx.send(SessionEvent::Error(format!("connect: {e}")));
            egui_ctx.request_repaint();
            return;
        }
    };
    let _ = in_tx.send(SessionEvent::Connected);
    egui_ctx.request_repaint();

    let (mut sink, mut stream) = ws.split();
    let mut ping_interval = tokio::time::interval(Duration::from_secs(30));
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Skip the first tick (fires immediately).
    ping_interval.tick().await;

    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    match protocol::decode(&text) {
                        Ok(p) => {
                            let _ = in_tx.send(SessionEvent::Packet(p));
                            egui_ctx.request_repaint();
                        }
                        Err(e) => log::warn!("decode failed: {e}; frame: {text}"),
                    }
                }
                Some(Ok(Message::Binary(_))) => { /* server never sends binary */ }
                Some(Ok(Message::Ping(data))) => {
                    // tungstenite answers control pings for us, but being
                    // defensive doesn't hurt.
                    let _ = sink.send(Message::Pong(data)).await;
                }
                Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => {}
                Some(Ok(Message::Close(frame))) => {
                    let reason = frame.map(|f| f.reason.to_string()).unwrap_or_default();
                    let _ = in_tx.send(SessionEvent::Closed(reason));
                    egui_ctx.request_repaint();
                    break;
                }
                Some(Err(e)) => {
                    let _ = in_tx.send(SessionEvent::Error(format!("stream: {e}")));
                    egui_ctx.request_repaint();
                    break;
                }
                None => {
                    let _ = in_tx.send(SessionEvent::Closed("stream ended".into()));
                    egui_ctx.request_repaint();
                    break;
                }
            },
            out = out_rx.recv() => match out {
                Some(s) => {
                    if let Err(e) = sink.send(Message::Text(s)).await {
                        let _ = in_tx.send(SessionEvent::Error(format!("send: {e}")));
                        egui_ctx.request_repaint();
                        break;
                    }
                }
                None => {
                    // UI side dropped — close cleanly.
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            },
            _ = ping_interval.tick() => {
                // Application-level ping (the server sends its own every 15s,
                // we echo those. This is just belt-and-braces for NAT keepalive).
                if let Ok(s) = protocol::encode(&Packet::ConnectionPing) {
                    let _ = sink.send(Message::Text(s)).await;
                }
            }
        }
    }
}
