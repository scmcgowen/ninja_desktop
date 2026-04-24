use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub dark_mode: bool,
    pub font_size: f32,
    pub trim_whitespace_on_save: bool,
    /// Prod server host (no protocol). Overridden by `SAMARI_DEV=1` env → uses
    /// localhost:8080 with ws:// instead.
    pub server_host: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark_mode: true,
            font_size: 14.0,
            trim_whitespace_on_save: true,
            server_host: "ninja.reconnected.cc".into(),
        }
    }
}
