use crate::storage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

pub struct ModelEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub descriptor: &'static str,
}

pub struct ModelGroup {
    pub provider: &'static str,
    pub models: &'static [ModelEntry],
}

pub const MODEL_GROUPS: &[ModelGroup] = &[
    ModelGroup {
        provider: "OpenAI",
        models: &[
            ModelEntry {
                id: "openai/gpt-5.6-sol",
                name: "GPT-5.6 Sol",
                descriptor: "SOTA reasoning",
            },
            ModelEntry {
                id: "openai/gpt-5.6-terra",
                name: "GPT-5.6 Terra",
                descriptor: "Opus-level thinking",
            },
            ModelEntry {
                id: "openai/gpt-5.6-luna",
                name: "GPT-5.6 Luna",
                descriptor: "Quick, smart responses",
            },
        ],
    },
    ModelGroup {
        provider: "Google",
        models: &[
            ModelEntry {
                id: "google/gemini-3.1-pro-preview",
                name: "Gemini 3.1 Pro",
                descriptor: "Deep thinking",
            },
            ModelEntry {
                id: "google/gemini-3.7-flash",
                name: "Gemini 3.7 Flash",
                descriptor: "Balanced model",
            },
            ModelEntry {
                id: "google/gemini-3.5-flash-lite",
                name: "Gemini 3.5 Flash-Lite",
                descriptor: "Cheap, quick thinker",
            },
        ],
    },
    ModelGroup {
        provider: "Anthropic",
        models: &[
            ModelEntry {
                id: "anthropic/claude-fable-5",
                name: "Claude Fable 5",
                descriptor: "Extreme cost Mythos-level reasoning model",
            },
            ModelEntry {
                id: "anthropic/claude-opus-5",
                name: "Claude Opus 5",
                descriptor: "State-of-the-art reasoning model",
            },
            ModelEntry {
                id: "anthropic/claude-sonnet-5",
                name: "Claude Sonnet 5",
                descriptor: "Balanced, adaptive model",
            },
        ],
    },
    ModelGroup {
        provider: "SpaceXAI",
        models: &[
            ModelEntry {
                id: "x-ai/grok-4.6",
                name: "Grok 4.6",
                descriptor: "Uncensored superpowered reasoning",
            },
            ModelEntry {
                id: "x-ai/grok-4.5",
                name: "Grok 4.5",
                descriptor: "Uncensored advanced reasoning",
            },
            ModelEntry {
                id: "x-ai/grok-build-0.1",
                name: "Grok Build 0.1",
                descriptor: "Fast agentic coding model",
            },
        ],
    },
    ModelGroup {
        provider: "Alibaba",
        models: &[
            ModelEntry {
                id: "qwen/qwen3.8-max",
                name: "Qwen3.8-Max",
                descriptor: "Extreme thinking model",
            },
            ModelEntry {
                id: "qwen/qwen3.7-plus",
                name: "Qwen3.7-Plus",
                descriptor: "Adaptive reasoning model",
            },
            ModelEntry {
                id: "qwen/qwen3.7-flash",
                name: "Qwen3.7-Flash",
                descriptor: "Cheap, quick model",
            },
        ],
    },
    ModelGroup {
        provider: "DeepSeek",
        models: &[
            ModelEntry {
                id: "deepseek/deepseek-v4-pro-0813",
                name: "DeepSeek V4 Pro",
                descriptor: "DeepSeek's latest advanced model",
            },
            ModelEntry {
                id: "deepseek/deepseek-v4-flash-0731",
                name: "DeepSeek V4 Flash",
                descriptor: "DeepSeek's latest fast model",
            },
            ModelEntry {
                id: "deepseek/deepseek-v3.2",
                name: "DeepSeek V3.2",
                descriptor: "DeepSeek's legacy model",
            },
        ],
    },
    ModelGroup {
        provider: "Z.ai",
        models: &[
            ModelEntry {
                id: "z-ai/glm-5.2",
                name: "GLM-5.2",
                descriptor: "Z.ai's latest reasoning model",
            },
            ModelEntry {
                id: "z-ai/glm-5.1",
                name: "GLM-5.1",
                descriptor: "Z.ai's previous reasoning model",
            },
            ModelEntry {
                id: "z-ai/glm-5v-turbo",
                name: "GLM-5V-Turbo",
                descriptor: "Z.ai's latest multimodal model",
            },
        ],
    },
    ModelGroup {
        provider: "Moonshot AI",
        models: &[
            ModelEntry {
                id: "moonshotai/kimi-k3",
                name: "Kimi K3",
                descriptor: "Moonshot's ultra reasoning model",
            },
            ModelEntry {
                id: "moonshotai/kimi-k2.7-code",
                name: "Kimi K2.7 Code",
                descriptor: "Moonshot's latest coding model",
            },
            ModelEntry {
                id: "moonshotai/kimi-k2.6",
                name: "Kimi K2.6",
                descriptor: "Moonshot's previous reasoning model",
            },
        ],
    },
    ModelGroup {
        provider: "MiniMax",
        models: &[
            ModelEntry {
                id: "minimax/minimax-m3",
                name: "MiniMax-M3",
                descriptor: "MiniMax's latest agentic model",
            },
            ModelEntry {
                id: "minimax/minimax-m2.7",
                name: "MiniMax-M2.7",
                descriptor: "MiniMax's previous agentic model",
            },
            ModelEntry {
                id: "minimax/minimax-m2-her",
                name: "MiniMax-M2-her",
                descriptor: "MiniMax's conversational model",
            },
        ],
    },
    ModelGroup {
        provider: "Xiaomi",
        models: &[
            ModelEntry {
                id: "xiaomi/mimo-v2.5-pro",
                name: "MiMo-V2.5-Pro",
                descriptor: "Xiaomi's latest advanced model",
            },
            ModelEntry {
                id: "xiaomi/mimo-v2.5",
                name: "MiMo-V2.5",
                descriptor: "Xiaomi's latest balanced model",
            },
            ModelEntry {
                id: "xiaomi/mimo-v2-flash",
                name: "MiMo-V2-Flash",
                descriptor: "Xiaomi's previous ultra-low cost model",
            },
        ],
    },
    ModelGroup {
        provider: "Coding",
        models: &[
            ModelEntry {
                id: "openai/gpt-5.6-sol-pro",
                name: "GPT-5.6 Sol Pro",
                descriptor: "Extreme cost, ultra intelligent OpenAI model",
            },
            ModelEntry {
                id: "anthropic/claude-opus-4.8",
                name: "Claude Opus 4.8",
                descriptor: "Anthropic's previous SOTA coding model",
            },
            ModelEntry {
                id: "kwaipilot/kat-coder-pro-v2.5",
                name: "KAT-Coder-Pro V2.5",
                descriptor: "Kwai's latest advanced coding model",
            },
        ],
    },
    ModelGroup {
        provider: "Legacy",
        models: &[
            ModelEntry {
                id: "openai/gpt-4o-2024-11-20",
                name: "GPT-4o",
                descriptor: "OpenAI's legacy chat model",
            },
            ModelEntry {
                id: "google/gemini-2.5-pro",
                name: "Gemini 2.5 Pro",
                descriptor: "Google's legacy Pro model",
            },
            ModelEntry {
                id: "x-ai/grok-4.20-multi-agent",
                name: "Grok 4.20 Multi-Agent",
                descriptor: "SpaceXAI's older multi-agent orchestrator",
            },
        ],
    },
];

pub const DEFAULT_MODEL: &str = "deepseek/deepseek-v4-flash-0731";

/// Class used by N3.10 Auto model selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelClass {
    StrongReasoning,
    CostPerformance,
    StrongVerification,
}

pub fn classify_model(id: &str, descriptor: &str) -> ModelClass {
    let blob = format!("{id} {descriptor}").to_ascii_lowercase();
    if blob.contains("coding") || blob.contains("agentic") || blob.contains("build") {
        return ModelClass::CostPerformance;
    }
    if blob.contains("flash")
        || blob.contains("luna")
        || blob.contains("cheap")
        || blob.contains("fast")
        || blob.contains("lite")
    {
        return ModelClass::CostPerformance;
    }
    if blob.contains("sota")
        || blob.contains("opus")
        || blob.contains("sol")
        || blob.contains("fable")
        || blob.contains("ultra")
        || blob.contains("extreme")
    {
        return ModelClass::StrongReasoning;
    }
    if blob.contains("reason") || blob.contains("think") || blob.contains("pro") {
        return ModelClass::StrongReasoning;
    }
    if blob.contains("sonnet") || blob.contains("terra") || blob.contains("balanced") {
        return ModelClass::StrongVerification;
    }
    ModelClass::StrongVerification
}

/// Planner → strongest reasoning, Coder → cost/performance, Reviewer → verification.
pub fn auto_model_for(stage: crate::pipeline::contract::StageKind) -> &'static str {
    let want = match stage {
        crate::pipeline::contract::StageKind::Planner => ModelClass::StrongReasoning,
        crate::pipeline::contract::StageKind::Coder => ModelClass::CostPerformance,
        crate::pipeline::contract::StageKind::Reviewer => ModelClass::StrongVerification,
        crate::pipeline::contract::StageKind::Verify
        | crate::pipeline::contract::StageKind::GitGate => ModelClass::CostPerformance,
    };
    MODEL_GROUPS
        .iter()
        .flat_map(|g| g.models)
        .find(|m| classify_model(m.id, m.descriptor) == want)
        .map(|m| m.id)
        .unwrap_or(DEFAULT_MODEL)
}

#[allow(dead_code)]
pub fn find_model(id: &str) -> Option<&'static ModelEntry> {
    MODEL_GROUPS
        .iter()
        .flat_map(|g| g.models.iter())
        .find(|m| m.id == id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub descriptor: Option<String>,
    pub context_length: Option<u32>,
    pub prompt_price: Option<f64>,
    pub completion_price: Option<f64>,
    pub supports_tools: bool,
    #[serde(default)]
    pub supports_vision: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedCatalog {
    fetched_at: DateTime<Utc>,
    models: Vec<ModelInfo>,
}

#[derive(Debug, Clone)]
pub struct ModelCatalog {
    pub highlights: Vec<ModelInfo>,
    pub all: Vec<ModelInfo>,
    pub fetched_at: Option<DateTime<Utc>>,
}

impl ModelCatalog {
    pub fn curated() -> Self {
        let highlights = curated_models();
        Self {
            all: highlights.clone(),
            highlights,
            fetched_at: None,
        }
    }

    pub fn load_cached() -> Option<Self> {
        let path = storage::models_cache_path();
        let bytes = std::fs::read(&path).ok()?;
        let cached: CachedCatalog = serde_json::from_slice(&bytes).ok()?;
        let age = Utc::now().signed_duration_since(cached.fetched_at);
        if age.to_std().ok()? > CACHE_TTL {
            tracing::info!("model catalog cache expired ({age})");
            return Some(Self::from_remote(cached.models, cached.fetched_at).stale());
        }
        Some(Self::from_remote(cached.models, cached.fetched_at))
    }

    fn stale(mut self) -> Self {
        self.fetched_at = None;
        self
    }

    pub fn from_remote(models: Vec<ModelInfo>, fetched_at: DateTime<Utc>) -> Self {
        let highlights = curated_models()
            .into_iter()
            .map(|curated| {
                if let Some(remote) = models.iter().find(|m| m.id == curated.id) {
                    ModelInfo {
                        descriptor: curated.descriptor,
                        ..remote.clone()
                    }
                } else {
                    curated
                }
            })
            .collect();
        Self {
            highlights,
            all: models,
            fetched_at: Some(fetched_at),
        }
    }

    pub fn save_cache(&self) -> anyhow::Result<()> {
        let Some(fetched_at) = self.fetched_at else {
            return Ok(());
        };
        let path = storage::models_cache_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let cached = CachedCatalog {
            fetched_at,
            models: self.all.clone(),
        };
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&cached)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn is_fresh(&self) -> bool {
        self.fetched_at.is_some()
    }

    pub fn find(&self, id: &str) -> Option<&ModelInfo> {
        self.highlights
            .iter()
            .chain(self.all.iter())
            .find(|m| m.id == id)
    }

    pub fn display_name(&self, id: &str) -> String {
        self.find(id)
            .map(|m| m.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    pub fn search_all(&self, query: &str) -> Vec<&ModelInfo> {
        let q = query.trim().to_ascii_lowercase();
        self.all
            .iter()
            .filter(|m| {
                q.is_empty()
                    || m.id.to_ascii_lowercase().contains(&q)
                    || m.name.to_ascii_lowercase().contains(&q)
            })
            .collect()
    }
}

fn curated_models() -> Vec<ModelInfo> {
    MODEL_GROUPS
        .iter()
        .flat_map(|g| g.models.iter())
        .map(|m| ModelInfo {
            id: m.id.to_string(),
            name: m.name.to_string(),
            descriptor: Some(m.descriptor.to_string()),
            context_length: None,
            prompt_price: None,
            completion_price: None,
            supports_tools: false,
            supports_vision: looks_multimodal(m.id),
        })
        .collect()
}

fn looks_multimodal(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.contains("gpt-4o")
        || id.contains("gpt-5")
        || id.contains("gemini")
        || id.contains("claude")
        || id.contains("grok")
        || id.contains("vision")
}

#[cfg(test)]
mod tests {
    use super::ModelCatalog;

    #[test]
    fn search_filters_by_name_or_id() {
        let catalog = ModelCatalog::curated();
        let hits = catalog.search_all("claude");
        assert!(hits.iter().any(|m| m.id.contains("claude")));
        assert!(hits.iter().all(|m| {
            m.id.to_ascii_lowercase().contains("claude")
                || m.name.to_ascii_lowercase().contains("claude")
        }));
    }

    #[test]
    fn unknown_model_falls_back_to_raw_id() {
        let catalog = ModelCatalog::curated();
        assert_eq!(
            catalog.display_name("vendor/missing-model"),
            "vendor/missing-model"
        );
    }

    #[test]
    fn auto_policy_picks_a_class_per_stage() {
        use super::{ModelClass, auto_model_for, classify_model, find_model};
        use crate::pipeline::contract::StageKind;

        let planner = auto_model_for(StageKind::Planner);
        let coder = auto_model_for(StageKind::Coder);
        let reviewer = auto_model_for(StageKind::Reviewer);
        let p = find_model(planner).expect("planner model in catalog");
        let c = find_model(coder).expect("coder model in catalog");
        let r = find_model(reviewer).expect("reviewer model in catalog");
        assert_eq!(
            classify_model(p.id, p.descriptor),
            ModelClass::StrongReasoning
        );
        assert_eq!(
            classify_model(c.id, c.descriptor),
            ModelClass::CostPerformance
        );
        assert_eq!(
            classify_model(r.id, r.descriptor),
            ModelClass::StrongVerification
        );
    }
}
