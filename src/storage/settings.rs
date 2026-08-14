//! App-wide settings persisted next to `chats.json`.

use super::chats::data_dir;
use crate::providers::catalog::DEFAULT_MODEL;
use crate::session::DEFAULT_SESSION_BUDGET_USD;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const SETTINGS_FILE: &str = "settings.json";

pub const DEFAULT_MAX_ITERATIONS: u32 = 25;
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 120;
pub const MIN_FONT_SCALE: f32 = 0.8;
pub const MAX_FONT_SCALE: f32 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemePreference {
    #[default]
    Dark,
    Light,
    System,
}

/// Controls non-essential UI transitions. Full motion is the default for a
/// more expressive operational console; Reduced keeps status feedback but
/// removes decorative reveals and pulses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MotionPreference {
    #[default]
    Full,
    Reduced,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_model")]
    pub chat_default_model: String,
    #[serde(default = "default_model")]
    pub coder_default_model: String,
    #[serde(default = "default_budget")]
    pub session_budget_usd: f64,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    #[serde(default = "default_timeout")]
    pub request_timeout_secs: u64,
    #[serde(default)]
    pub theme: ThemePreference,
    #[serde(default = "default_font_scale")]
    pub font_scale: f32,
    #[serde(default)]
    pub motion: MotionPreference,
    #[serde(default = "default_recent_keep")]
    pub context_recent_messages: usize,
}

fn default_model() -> String {
    DEFAULT_MODEL.to_string()
}

fn default_budget() -> f64 {
    DEFAULT_SESSION_BUDGET_USD
}

fn default_max_iterations() -> u32 {
    DEFAULT_MAX_ITERATIONS
}

fn default_timeout() -> u64 {
    DEFAULT_REQUEST_TIMEOUT_SECS
}

fn default_font_scale() -> f32 {
    1.0
}

fn default_recent_keep() -> usize {
    crate::session::context_window::DEFAULT_RECENT_KEEP
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            chat_default_model: default_model(),
            coder_default_model: default_model(),
            session_budget_usd: default_budget(),
            max_iterations: default_max_iterations(),
            request_timeout_secs: default_timeout(),
            theme: ThemePreference::Dark,
            font_scale: default_font_scale(),
            motion: MotionPreference::Full,
            context_recent_messages: default_recent_keep(),
        }
    }
}

impl AppSettings {
    pub fn sanitized(mut self) -> Self {
        if self.chat_default_model.trim().is_empty() {
            self.chat_default_model = default_model();
        }
        if self.coder_default_model.trim().is_empty() {
            self.coder_default_model = default_model();
        }
        if !self.session_budget_usd.is_finite() || self.session_budget_usd <= 0.0 {
            self.session_budget_usd = default_budget();
        }
        if self.max_iterations == 0 {
            self.max_iterations = default_max_iterations();
        }
        if self.request_timeout_secs == 0 {
            self.request_timeout_secs = default_timeout();
        }
        if !self.font_scale.is_finite() {
            self.font_scale = default_font_scale();
        }
        self.font_scale = self.font_scale.clamp(MIN_FONT_SCALE, MAX_FONT_SCALE);
        if self.context_recent_messages == 0 {
            self.context_recent_messages = default_recent_keep();
        }
        self
    }
}

fn settings_path() -> PathBuf {
    data_dir()
        .map(|dir| dir.join(SETTINGS_FILE))
        .unwrap_or_else(|| PathBuf::from(SETTINGS_FILE))
}

pub fn load_settings() -> AppSettings {
    let path = settings_path();
    if !path.exists() {
        return AppSettings::default();
    }
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<AppSettings>(&bytes) {
            Ok(settings) => settings.sanitized(),
            Err(e) => {
                tracing::warn!("couldn't parse {}: {e:#}", path.display());
                AppSettings::default()
            }
        },
        Err(e) => {
            tracing::warn!("couldn't read {}: {e:#}", path.display());
            AppSettings::default()
        }
    }
}

pub fn save_settings(settings: &AppSettings) -> Result<()> {
    let path = settings_path();
    let json = serde_json::to_vec_pretty(settings)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_defaults() {
        let settings = AppSettings::default();
        assert_eq!(settings.chat_default_model, DEFAULT_MODEL);
        assert_eq!(settings.max_iterations, DEFAULT_MAX_ITERATIONS);
        assert_eq!(settings.theme, ThemePreference::Dark);
        assert!((settings.font_scale - 1.0).abs() < f32::EPSILON);
        assert_eq!(settings.motion, MotionPreference::Full);
    }

    #[test]
    fn partial_json_fills_new_fields() {
        let parsed: AppSettings =
            serde_json::from_str(r#"{"chat_default_model":"openai/gpt-4.1"}"#).unwrap();
        let settings = parsed.sanitized();
        assert_eq!(settings.chat_default_model, "openai/gpt-4.1");
        assert_eq!(settings.coder_default_model, DEFAULT_MODEL);
        assert_eq!(settings.request_timeout_secs, DEFAULT_REQUEST_TIMEOUT_SECS);
        assert_eq!(settings.motion, MotionPreference::Full);
    }

    #[test]
    fn sanitize_rejects_empty_and_non_finite() {
        let settings = AppSettings {
            chat_default_model: "  ".into(),
            coder_default_model: String::new(),
            session_budget_usd: f64::NAN,
            max_iterations: 0,
            request_timeout_secs: 0,
            theme: ThemePreference::Light,
            font_scale: 9.0,
            motion: MotionPreference::Reduced,
            context_recent_messages: 0,
        }
        .sanitized();
        assert_eq!(settings.chat_default_model, DEFAULT_MODEL);
        assert_eq!(settings.coder_default_model, DEFAULT_MODEL);
        assert_eq!(settings.session_budget_usd, DEFAULT_SESSION_BUDGET_USD);
        assert_eq!(settings.max_iterations, DEFAULT_MAX_ITERATIONS);
        assert_eq!(settings.request_timeout_secs, DEFAULT_REQUEST_TIMEOUT_SECS);
        assert!((settings.font_scale - MAX_FONT_SCALE).abs() < f32::EPSILON);
    }
}
