//! Project-declared lifecycle hooks. Policy convenience, not a security boundary.

pub mod runner;
pub mod trust;

pub use runner::{HookPayload, HookRun, run_hook};
pub use trust::{HookConfig, load_hooks};

use crate::security::declared::{self, MachineTrust};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct HookLastResult {
    pub decision: String,
    pub reason: Option<String>,
}

fn last_results() -> &'static Mutex<HashMap<String, HookLastResult>> {
    static LAST: OnceLock<Mutex<HashMap<String, HookLastResult>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn record_last(fingerprint: &str, decision: impl Into<String>, reason: Option<String>) {
    if let Ok(mut map) = last_results().lock() {
        map.insert(
            fingerprint.to_string(),
            HookLastResult {
                decision: decision.into(),
                reason,
            },
        );
    }
}

pub fn last_result(fingerprint: &str) -> Option<HookLastResult> {
    last_results()
        .lock()
        .ok()
        .and_then(|map| map.get(fingerprint).cloned())
}

pub fn is_trusted(hook: &HookConfig) -> bool {
    MachineTrust::HOOKS.is_trusted(&hook.fingerprint())
}

pub fn trust_on_this_machine(hook: &HookConfig) -> Result<(), String> {
    MachineTrust::HOOKS.trust(&hook.fingerprint())
}

pub fn is_enabled(hook: &HookConfig) -> bool {
    declared::hook_enabled(&hook.fingerprint())
}

pub fn set_enabled(hook: &HookConfig, enabled: bool) -> Result<(), String> {
    declared::set_hook_enabled(&hook.fingerprint(), enabled)
}
