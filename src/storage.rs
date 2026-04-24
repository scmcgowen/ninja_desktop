use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::settings::Settings;
use crate::token::{check_token, Token};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppData {
    #[serde(default)]
    pub tokens: Vec<Token>,
    #[serde(default)]
    pub active: Option<Token>,
    #[serde(default)]
    pub settings: Settings,
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("cc", "minecartchris", "samari-catcher")
}

pub fn config_path() -> Option<PathBuf> {
    project_dirs().map(|p| p.config_dir().join("config.json"))
}

pub fn load() -> AppData {
    let Some(path) = config_path() else { return AppData::default(); };
    let Ok(bytes) = fs::read(&path) else { return AppData::default(); };
    match serde_json::from_slice::<AppData>(&bytes) {
        Ok(data) => data,
        Err(e) => {
            log::warn!("config at {} is corrupt ({e}); starting fresh", path.display());
            AppData::default()
        }
    }
}

pub fn save(data: &AppData) -> Result<()> {
    let path = config_path().context("no ProjectDirs available on this platform")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir -p {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(data)?;
    fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("renaming {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

// Import / export ---------------------------------------------------------

/// Shape used on disk for import/export. Intentionally a small stable schema
/// so this file can be hand-edited or shared.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenBundle {
    pub tokens: Vec<String>,
}

pub fn export_tokens(path: &std::path::Path, tokens: &[Token]) -> Result<()> {
    let bundle = TokenBundle { tokens: tokens.iter().map(|t| t.as_str().into()).collect() };
    let bytes = serde_json::to_vec_pretty(&bundle)?;
    fs::write(path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Read the file, return valid tokens. Skips anything malformed without failing
/// the whole operation — users are pasting tokens by hand, meet them halfway.
pub fn import_tokens(path: &std::path::Path) -> Result<Vec<Token>> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let bundle: TokenBundle = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {} as TokenBundle JSON", path.display()))?;
    Ok(bundle.tokens.into_iter()
        .filter(|s| check_token(s))
        .filter_map(|s| Token::new(s))
        .collect())
}
