//! Wire protocol matching `src/network.ts`.
//!
//! Packets are JSON objects with a numeric `packet` discriminator. serde's
//! built-in tagging doesn't accept numeric tags cleanly, so we hand-write the
//! `Packet` Deserialize / Serialize via an intermediate `serde_json::Value`.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diff_patch::Fragment;

/// The max payload the server will accept; see `MAX_PACKET_SIZE` in
/// `src/network.ts`. Outgoing frames larger than this should fall back to
/// `Replace` or fail loudly.
pub const MAX_PACKET_SIZE: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketCode {
    ConnectionUpdate = 0x00,
    ConnectionAbuse = 0x01,
    ConnectionPing = 0x02,
    TerminalContents = 0x10,
    TerminalEvents = 0x11,
    TerminalInfo = 0x12,
    FileListing = 0x20,
    FileRequest = 0x21,
    FileAction = 0x22,
    FileConsume = 0x23,
}

impl PacketCode {
    fn from_u8(n: u8) -> Option<Self> {
        Some(match n {
            0x00 => Self::ConnectionUpdate,
            0x01 => Self::ConnectionAbuse,
            0x02 => Self::ConnectionPing,
            0x10 => Self::TerminalContents,
            0x11 => Self::TerminalEvents,
            0x12 => Self::TerminalInfo,
            0x20 => Self::FileListing,
            0x21 => Self::FileRequest,
            0x22 => Self::FileAction,
            0x23 => Self::FileConsume,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub enum Capability {
    TerminalHost,
    TerminalView,
    FileHost,
    FileEdit,
}

impl Capability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TerminalHost => "terminal:host",
            Self::TerminalView => "terminal:view",
            Self::FileHost => "file:host",
            Self::FileEdit => "file:edit",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "terminal:host" => Self::TerminalHost,
            "terminal:view" => Self::TerminalView,
            "file:host" => Self::FileHost,
            "file:edit" => Self::FileEdit,
            _ => return None,
        })
    }
}

// File-related types -------------------------------------------------------

/// Bitmask flags on file action entries. See `FileActionFlags` in
/// `src/network.ts`.
pub mod file_flags {
    pub const READ_ONLY: u32 = 0x1;
    pub const FORCE: u32 = 0x2;
    pub const OPEN: u32 = 0x4;
    pub const NEW: u32 = 0x8;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum FileConsumeResult {
    Ok,
    Reject,
    Failure,
}
impl From<FileConsumeResult> for u8 {
    fn from(v: FileConsumeResult) -> u8 {
        match v { FileConsumeResult::Ok => 1, FileConsumeResult::Reject => 2, FileConsumeResult::Failure => 3 }
    }
}
impl TryFrom<u8> for FileConsumeResult {
    type Error = String;
    fn try_from(n: u8) -> Result<Self, Self::Error> {
        Ok(match n {
            1 => Self::Ok, 2 => Self::Reject, 3 => Self::Failure,
            other => return Err(format!("unknown FileConsume result: {other}")),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub file: String,
    pub checksum: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileConsumeEntry {
    pub file: String,
    pub checksum: u32,
    pub result: FileConsumeResult,
}

/// One entry in a `FileAction` packet. Shape depends on `action`:
///   action=0 (Replace) → `contents`
///   action=1 (Patch)   → `delta`
///   action=2 (Delete)  → (no extra fields)
///
/// We flatten this into a single struct with optional fields because the web
/// protocol is permissive (extra fields are tolerated server-side).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileActionEntry {
    pub file: String,
    pub checksum: u32,
    pub flags: u32,
    pub action: u8, // 0 Replace, 1 Patch, 2 Delete
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub contents: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub delta: Option<Vec<Fragment>>,
}

// Inbound / outbound packets ----------------------------------------------

#[derive(Clone, Debug)]
pub enum Packet {
    ConnectionUpdate { clients: u32, capabilities: Vec<String> },
    ConnectionAbuse { message: String },
    ConnectionPing,
    TerminalContents(Value), // opaque for now — not used in file-editor MVP
    TerminalInfo { id: Option<i64>, label: Option<String> },
    FileListing { id: u32, files: Vec<FileEntry> },
    FileRequest { id: u32, file: Vec<FileEntry> },
    FileAction { id: u32, actions: Vec<FileActionEntry> },
    FileConsume { id: u32, files: Vec<FileConsumeEntry> },
    /// Outbound-only: terminal events. Kept here so outbound encoding is
    /// symmetric with the web client even though the file editor doesn't
    /// generate these in v1.
    TerminalEvents { events: Vec<TerminalEvent> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TerminalEvent {
    pub name: String,
    pub args: Vec<Value>,
}

impl Packet {
    pub fn code(&self) -> PacketCode {
        match self {
            Self::ConnectionUpdate { .. } => PacketCode::ConnectionUpdate,
            Self::ConnectionAbuse { .. } => PacketCode::ConnectionAbuse,
            Self::ConnectionPing => PacketCode::ConnectionPing,
            Self::TerminalContents(_) => PacketCode::TerminalContents,
            Self::TerminalEvents { .. } => PacketCode::TerminalEvents,
            Self::TerminalInfo { .. } => PacketCode::TerminalInfo,
            Self::FileListing { .. } => PacketCode::FileListing,
            Self::FileRequest { .. } => PacketCode::FileRequest,
            Self::FileAction { .. } => PacketCode::FileAction,
            Self::FileConsume { .. } => PacketCode::FileConsume,
        }
    }
}

pub fn encode(packet: &Packet) -> Result<String> {
    let value = encode_to_value(packet)?;
    Ok(serde_json::to_string(&value)?)
}

fn encode_to_value(packet: &Packet) -> Result<Value> {
    let code = packet.code() as u8;
    let mut obj = serde_json::Map::new();
    obj.insert("packet".into(), Value::from(code));
    match packet {
        Packet::ConnectionUpdate { clients, capabilities } => {
            obj.insert("clients".into(), Value::from(*clients));
            obj.insert("capabilities".into(), serde_json::to_value(capabilities)?);
        }
        Packet::ConnectionAbuse { message } => {
            obj.insert("message".into(), Value::from(message.clone()));
        }
        Packet::ConnectionPing => {}
        Packet::TerminalContents(v) => {
            if let Value::Object(m) = v {
                for (k, v) in m { if k != "packet" { obj.insert(k.clone(), v.clone()); } }
            }
        }
        Packet::TerminalEvents { events } => {
            obj.insert("events".into(), serde_json::to_value(events)?);
        }
        Packet::TerminalInfo { id, label } => {
            if let Some(id) = id { obj.insert("id".into(), Value::from(*id)); }
            if let Some(label) = label { obj.insert("label".into(), Value::from(label.clone())); }
        }
        Packet::FileListing { id, files } => {
            obj.insert("id".into(), Value::from(*id));
            obj.insert("files".into(), serde_json::to_value(files)?);
        }
        Packet::FileRequest { id, file } => {
            obj.insert("id".into(), Value::from(*id));
            obj.insert("file".into(), serde_json::to_value(file)?);
        }
        Packet::FileAction { id, actions } => {
            obj.insert("id".into(), Value::from(*id));
            obj.insert("actions".into(), serde_json::to_value(actions)?);
        }
        Packet::FileConsume { id, files } => {
            obj.insert("id".into(), Value::from(*id));
            obj.insert("files".into(), serde_json::to_value(files)?);
        }
    }
    Ok(Value::Object(obj))
}

pub fn decode(s: &str) -> Result<Packet> {
    let value: Value = serde_json::from_str(s).context("parsing packet JSON")?;
    let obj = value.as_object().ok_or_else(|| anyhow!("packet not a JSON object"))?;
    let code_n = obj.get("packet").and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("missing/non-integer `packet` field"))?;
    let code = PacketCode::from_u8(code_n as u8)
        .ok_or_else(|| anyhow!("unknown packet code {code_n}"))?;

    Ok(match code {
        PacketCode::ConnectionUpdate => Packet::ConnectionUpdate {
            clients: obj.get("clients").and_then(Value::as_u64).unwrap_or(0) as u32,
            capabilities: obj.get("capabilities").and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default(),
        },
        PacketCode::ConnectionAbuse => Packet::ConnectionAbuse {
            message: obj.get("message").and_then(Value::as_str).unwrap_or("").into(),
        },
        PacketCode::ConnectionPing => Packet::ConnectionPing,
        PacketCode::TerminalContents => Packet::TerminalContents(value.clone()),
        PacketCode::TerminalEvents => Packet::TerminalEvents {
            events: serde_json::from_value(
                obj.get("events").cloned().unwrap_or(Value::Array(vec![]))
            ).context("decoding TerminalEvents.events")?,
        },
        PacketCode::TerminalInfo => Packet::TerminalInfo {
            id: obj.get("id").and_then(Value::as_i64),
            label: obj.get("label").and_then(Value::as_str).map(String::from),
        },
        PacketCode::FileListing => Packet::FileListing {
            id: obj.get("id").and_then(Value::as_u64).unwrap_or(0) as u32,
            files: serde_json::from_value(
                obj.get("files").cloned().unwrap_or(Value::Array(vec![]))
            )?,
        },
        PacketCode::FileRequest => Packet::FileRequest {
            id: obj.get("id").and_then(Value::as_u64).unwrap_or(0) as u32,
            file: serde_json::from_value(
                obj.get("file").cloned().unwrap_or(Value::Array(vec![]))
            )?,
        },
        PacketCode::FileAction => Packet::FileAction {
            id: obj.get("id").and_then(Value::as_u64).unwrap_or(0) as u32,
            actions: serde_json::from_value(
                obj.get("actions").cloned().unwrap_or(Value::Array(vec![]))
            )?,
        },
        PacketCode::FileConsume => Packet::FileConsume {
            id: obj.get("id").and_then(Value::as_u64).unwrap_or(0) as u32,
            files: serde_json::from_value(
                obj.get("files").cloned().unwrap_or(Value::Array(vec![]))
            )?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_ping() {
        let s = encode(&Packet::ConnectionPing).unwrap();
        assert_eq!(s, r#"{"packet":2}"#);
        let p = decode(&s).unwrap();
        assert!(matches!(p, Packet::ConnectionPing));
    }

    #[test]
    fn round_trip_connection_update() {
        let p = Packet::ConnectionUpdate { clients: 2, capabilities: vec!["file:host".into()] };
        let s = encode(&p).unwrap();
        let p2 = decode(&s).unwrap();
        match p2 {
            Packet::ConnectionUpdate { clients, capabilities } => {
                assert_eq!(clients, 2);
                assert_eq!(capabilities, vec!["file:host".to_string()]);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn decode_unknown_code_fails_loudly() {
        let err = decode(r#"{"packet":99}"#).unwrap_err();
        assert!(err.to_string().contains("unknown packet code"));
    }
}
