use crate::coder::{AppMode, CoderState};
use crate::providers::accumulate::AssistantAccumulator;
use crate::providers::catalog::ModelCatalog;
use crate::providers::{
    ANTHROPIC, AiProvider, ChatMessage, ChatRequest, OPENAI_COMPAT, OPENROUTER, ProviderError,
    ProviderEvent, ProviderHub, connect_openrouter_timed, validate_openrouter_key,
};
use crate::search::{
    MAX_SEARCHES_PER_TURN, SearchBackend, TAVILY, TavilySearch, WebSource, format_tool_result,
    hosted_web_search_schema, web_search_schema,
};
use crate::secure_store::SecureStore;
use crate::storage::{self, AppSettings, Db};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub use crate::providers::catalog::{DEFAULT_MODEL, MODEL_GROUPS};

pub enum Screen {
    Onboarding(OnboardingState),
    Main(Box<MainState>),
}

pub struct OnboardingState {
    pub key_input: String,
    pub show_key: bool,
    pub status: OnboardingStatus,
    pub rx: Option<Receiver<ValidationResult>>,
}

#[derive(Default, Clone)]
pub enum OnboardingStatus {
    #[default]
    Idle,
    Validating,
    Error(String),
}

pub enum ValidationResult {
    Ok(String),
    Err(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialState {
    Missing,
    Present,
    Rejected,
}

#[derive(Debug, Clone)]
pub struct CredentialStatus {
    pub state: CredentialState,
    pub masked: String,
}

impl CredentialStatus {
    pub fn from_key(key: Option<&str>) -> Self {
        match key.map(str::trim).filter(|s| !s.is_empty()) {
            Some(key) => Self {
                state: CredentialState::Present,
                masked: mask_api_key(key),
            },
            None => Self {
                state: CredentialState::Missing,
                masked: String::new(),
            },
        }
    }
}

/// Consult the stored credential without a network call.
pub fn credential_state(status: &CredentialStatus) -> CredentialState {
    status.state
}

pub fn resolve_provider(state: &MainState, model: &str) -> Option<Arc<dyn AiProvider>> {
    state
        .providers
        .resolve(model, &state.catalog)
        .or_else(|| state.provider.clone())
}

pub fn build_provider_hub(timeout: std::time::Duration, openai_compat_base: &str) -> ProviderHub {
    let mut hub = ProviderHub::default();
    if let Ok(Some(key)) = SecureStore::load_key_for(OPENROUTER)
        && let Ok(provider) = connect_openrouter_timed(key, timeout)
    {
        hub.insert(provider);
    }
    if let Ok(Some(key)) = SecureStore::load_key_for(ANTHROPIC)
        && let Ok(client) = crate::providers::anthropic::AnthropicClient::new(key, timeout)
    {
        hub.insert(Arc::new(client));
    }
    let base = openai_compat_base.trim();
    if !base.is_empty() {
        let key = SecureStore::load_key_for(OPENAI_COMPAT).ok().flatten();
        if let Ok(client) =
            crate::providers::openai_compat::OpenAiCompatClient::new(key, base.to_string(), timeout)
        {
            hub.insert(Arc::new(client));
        }
    }
    hub
}

pub async fn refresh_catalog(hub: ProviderHub) -> ModelCatalog {
    let mut models = Vec::new();
    for id in [OPENROUTER, ANTHROPIC, OPENAI_COMPAT] {
        let Some(provider) = hub.get(id) else {
            continue;
        };
        match provider.list_models().await {
            Ok(list) => models.extend(list.into_iter().map(Into::into)),
            Err(e) => tracing::warn!("couldn't list models for {id}: {e}"),
        }
    }
    if models.is_empty() {
        return ModelCatalog::curated();
    }
    ModelCatalog::from_remote(models, Utc::now())
}

async fn validate_provider_key(
    provider: &str,
    key: String,
    timeout: std::time::Duration,
    openai_base: &str,
) -> Result<(), ProviderError> {
    match provider {
        id if id == ANTHROPIC => {
            let client = crate::providers::anthropic::AnthropicClient::new(key, timeout)
                .map_err(|e| ProviderError::Message(e.to_string()))?;
            client.validate_key().await
        }
        id if id == OPENAI_COMPAT => {
            let base = if openai_base.trim().is_empty() {
                crate::providers::openai_compat::DEFAULT_LOCAL_BASE_URL.to_string()
            } else {
                openai_base.to_string()
            };
            let client = crate::providers::openai_compat::OpenAiCompatClient::new(
                if key.trim().is_empty() {
                    None
                } else {
                    Some(key)
                },
                base,
                timeout,
            )
            .map_err(|e| ProviderError::Message(e.to_string()))?;
            client.list_models().await.map(|_| ())
        }
        _ => validate_openrouter_key(key).await,
    }
}

pub fn can_create_session(state: CredentialState) -> Result<(), CredentialState> {
    match state {
        CredentialState::Present => Ok(()),
        other => Err(other),
    }
}

/// Always-masked display form, e.g. `sk-or-v1-••••1a2b`.
pub fn mask_api_key(key: &str) -> String {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let suffix: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if let Some(rest) = trimmed.strip_prefix("sk-or-v1-") {
        if rest.is_empty() {
            "sk-or-v1-••••".into()
        } else {
            format!("sk-or-v1-••••{suffix}")
        }
    } else if trimmed.chars().count() <= 4 {
        "••••".into()
    } else {
        format!("••••{suffix}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Credentials,
    Models,
    Limits,
    Appearance,
    Shortcuts,
    Mcp,
    Hooks,
    Anthropic,
    Tavily,
    Local,
    About,
}

#[derive(Debug, Clone, Default)]
pub enum KeyTestStatus {
    #[default]
    Idle,
    Testing,
    Ok,
    Err(String),
}

#[derive(Default)]
pub struct SettingsUi {
    pub open: bool,
    pub tab: SettingsTab,
    pub key_input: String,
    pub show_key: bool,
    pub test_status: KeyTestStatus,
    pub test_rx: Option<Receiver<ValidationResult>>,
    pub confirm_remove: bool,
    #[allow(dead_code)]
    pub test_provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(skip, default)]
    pub appeared_at: Option<Instant>,
    #[serde(default)]
    pub interrupted: bool,
    #[serde(default)]
    pub images: Vec<crate::providers::ImageAttachment>,
    /// Grounding sources belonging to this assistant response.
    #[serde(default)]
    pub sources: Vec<WebSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ChatSearchMode {
    #[default]
    Off,
    Auto,
    Tavily,
}

impl ChatSearchMode {
    pub const fn enabled(self) -> bool {
        !matches!(self, Self::Off)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: Uuid,
    pub title: String,
    pub model: String,
    pub messages: Vec<Message>,
    pub created_at: DateTime<Utc>,
    /// Per-chat system prompt. Kept off the message vector so truncation
    /// never drops it and older `chats.json` files keep loading.
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub context_summary: Option<String>,
    #[serde(default)]
    pub context_summary_upto: usize,
    #[serde(skip)]
    pub context_occupancy: Option<f32>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub web_search: ChatSearchMode,
}

impl Chat {
    pub fn new(model: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: "New chat".into(),
            model,
            messages: Vec::new(),
            created_at: Utc::now(),
            system: None,
            context_summary: None,
            context_summary_upto: 0,
            context_occupancy: None,
            pinned: false,
            web_search: ChatSearchMode::Off,
        }
    }

    /// System prompt sent to the model. Empty or whitespace-only is omitted.
    pub fn request_system(&self) -> Option<String> {
        self.system
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
    }
}

pub use crate::session::AUTH_REJECTED_NOTICE;

pub enum StreamUiEvent {
    Delta(String),
    Done,
    Error(String),
    Unauthorized,
    Cancelled,
    Retrying {
        attempt: u32,
        max_attempts: u32,
        wait_secs: u64,
    },
    Sources(Vec<WebSource>),
    Searching,
}

pub struct PendingResponse {
    pub assistant_index: usize,
    pub rx: Receiver<StreamUiEvent>,
    pub cancel: CancellationToken,
}

enum ChatSearchRoute {
    Hosted,
    Tavily(TavilySearch),
}

impl Drop for PendingResponse {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[derive(Default)]
pub struct ChatUiState {
    pub input: String,
    pub draft_images: Vec<crate::providers::ImageAttachment>,
    pub retry_hint: Option<String>,
    pub editing: Option<MessageEdit>,
    pub auth_rejected: bool,
    pub generation: u64,
    pub search_status: Option<String>,
}

pub enum AuthRetryTarget {
    Chat { chat_id: Uuid, generation: u64 },
    Coder(crate::session::SessionId),
}

pub struct MainState {
    pub providers: ProviderHub,
    pub provider: Option<Arc<dyn AiProvider>>,
    pub chats: Vec<Chat>,
    pub active_chat_id: Option<Uuid>,
    pub temp_chat: Option<Chat>,
    pub temporary_mode: bool,
    pub chat_ui: HashMap<Uuid, ChatUiState>,
    pub pending: HashMap<Uuid, PendingResponse>,
    pub focus_input_next_frame: bool,
    pub confirm_eject: bool,
    pub catalog: ModelCatalog,
    pub catalog_rx: Option<Receiver<ModelCatalog>>,
    pub model_search: String,
    pub mode: AppMode,
    pub coder: CoderState,
    pub settings: AppSettings,
    pub settings_ui: SettingsUi,
    pub credential: CredentialStatus,
    pub banner_key_input: String,
    pub banner_show_key: bool,
    pub retry_after_auth: Option<AuthRetryTarget>,
    // Coder Mode attachments; Chat drafts live in chat_ui.
    pub draft_images: Vec<crate::providers::ImageAttachment>,
    pub lightbox: Option<crate::providers::ImageAttachment>,
    pub chat_search: String,
    pub focus_search_next_frame: bool,
    pub renaming_chat: Option<(Uuid, String)>,
    pub pending_confirm: Option<PendingConfirm>,
    pub editing_coder: Option<MessageEdit>,
}

#[derive(Debug, Clone)]
pub struct MessageEdit {
    pub index: usize,
    pub draft: String,
}

#[derive(Debug, Clone)]
pub enum PendingConfirm {
    DeleteChat {
        chat_id: Uuid,
        index: usize,
        count: usize,
    },
    EditResendChat {
        chat_id: Uuid,
        index: usize,
        text: String,
        count: usize,
    },
    DeleteCoder {
        index: usize,
        count: usize,
    },
    EditResendCoder {
        index: usize,
        text: String,
        count: usize,
    },
}

impl MainState {
    pub fn chat(&self, id: Uuid) -> Option<&Chat> {
        self.chats
            .iter()
            .find(|c| c.id == id)
            .or_else(|| self.temp_chat.as_ref().filter(|c| c.id == id))
    }

    pub fn chat_mut(&mut self, id: Uuid) -> Option<&mut Chat> {
        self.chats
            .iter_mut()
            .find(|c| c.id == id)
            .or_else(|| self.temp_chat.as_mut().filter(|c| c.id == id))
    }

    pub fn active_chat_pending(&self) -> bool {
        self.active_chat()
            .is_some_and(|c| self.pending.contains_key(&c.id))
    }

    pub fn active_chat_ui(&self) -> Option<&ChatUiState> {
        self.chat_ui.get(&self.active_chat()?.id)
    }

    pub fn active_chat_ui_mut(&mut self) -> Option<&mut ChatUiState> {
        let id = self.active_chat()?.id;
        Some(self.chat_ui.entry(id).or_default())
    }

    fn auth_retry_target(&self) -> Option<AuthRetryTarget> {
        let for_chat = |id: Uuid| {
            self.chat_ui
                .get(&id)
                .filter(|s| s.auth_rejected)
                .map(|s| AuthRetryTarget::Chat {
                    chat_id: id,
                    generation: s.generation,
                })
        };
        match self.mode {
            AppMode::Chat => {
                if let Some(target) = self.active_chat().and_then(|c| for_chat(c.id)) {
                    return Some(target);
                }
            }
            AppMode::Coder => {
                if let Some(live) = self.coder.sessions.active()
                    && live.transcript.last().is_some_and(|m| matches!(m, crate::session::TranscriptItem::Assistant(text) if text == AUTH_REJECTED_NOTICE))
                {
                    return Some(AuthRetryTarget::Coder(live.id.clone()));
                }
            }
        }
        // The global banner can describe a background chat. Retry it only when
        // there is one unambiguous origin; otherwise just repair the credential.
        let mut failed = self
            .chat_ui
            .iter()
            .filter(|(id, s)| s.auth_rejected && self.chat(**id).is_some());
        let (&id, _) = failed.next()?;
        if failed.next().is_some() {
            return None;
        }
        for_chat(id)
    }

    fn dispose_chat(&mut self, id: Uuid) {
        self.pending.remove(&id); // Drop cancels the worker before its receiver disappears.
        self.chat_ui.remove(&id);
        if matches!(self.retry_after_auth, Some(AuthRetryTarget::Chat { chat_id, .. }) if chat_id == id)
        {
            self.retry_after_auth = None;
        }
        if matches!(self.pending_confirm,
            Some(PendingConfirm::DeleteChat { chat_id, .. } | PendingConfirm::EditResendChat { chat_id, .. }) if chat_id == id)
        {
            self.pending_confirm = None;
        }
        self.lightbox = None;
    }

    pub fn active_chat_mut(&mut self) -> Option<&mut Chat> {
        if self.temporary_mode {
            self.temp_chat.as_mut()
        } else {
            let id = self.active_chat_id?;
            self.chats.iter_mut().find(|c| c.id == id)
        }
    }

    pub fn active_chat(&self) -> Option<&Chat> {
        if self.temporary_mode {
            self.temp_chat.as_ref()
        } else {
            let id = self.active_chat_id?;
            self.chats.iter().find(|c| c.id == id)
        }
    }
}

pub struct App {
    pub screen: Screen,
    pub rt: Arc<Runtime>,
    pub db: Arc<Db>,
    #[cfg(test)]
    chat_history_path: Option<std::path::PathBuf>,
}

impl App {
    pub fn new() -> anyhow::Result<Self> {
        let rt = Arc::new(Runtime::new()?);
        let db = Arc::new(Db::open()?);

        let screen = match SecureStore::load_key() {
            Ok(Some(key)) => Screen::Main(Box::new(Self::build_main_state(key, &rt)?)),
            Ok(None) => Screen::Onboarding(OnboardingState {
                key_input: String::new(),
                show_key: false,
                status: OnboardingStatus::Idle,
                rx: None,
            }),
            Err(e) => Screen::Onboarding(OnboardingState {
                key_input: String::new(),
                show_key: false,
                status: OnboardingStatus::Error(format!(
                    "Couldn't access {}. Start or unlock it, then try again: {e:#}",
                    SecureStore::display_name()
                )),
                rx: None,
            }),
        };

        Ok(Self {
            screen,
            rt,
            db,
            #[cfg(test)]
            chat_history_path: None,
        })
    }

    fn build_main_state(api_key: String, rt: &Runtime) -> anyhow::Result<MainState> {
        let settings = storage::load_settings();
        let timeout = std::time::Duration::from_secs(settings.request_timeout_secs);
        let providers = build_provider_hub(timeout, &settings.openai_compat_base_url);
        let provider = providers
            .get(OPENROUTER)
            .or_else(|| connect_openrouter_timed(api_key.clone(), timeout).ok());
        let chats = storage::load_chats().unwrap_or_else(|e| {
            tracing::warn!("couldn't load chats.json: {e:#}");
            Vec::new()
        });
        let active_chat_id = chats.first().map(|c| c.id);
        let catalog = ModelCatalog::load_cached().unwrap_or_else(ModelCatalog::curated);
        let catalog_rx = if catalog.is_fresh() {
            None
        } else {
            let (tx, rx) = mpsc::channel();
            let hub = providers.clone();
            rt.spawn(async move {
                let catalog = refresh_catalog(hub).await;
                if let Err(e) = catalog.save_cache() {
                    tracing::warn!("couldn't save model catalog cache: {e:#}");
                }
                let _ = tx.send(catalog);
            });
            Some(rx)
        };

        Ok(MainState {
            providers,
            provider,
            chats,
            active_chat_id,
            temp_chat: None,
            temporary_mode: false,
            chat_ui: HashMap::new(),
            pending: HashMap::new(),
            focus_input_next_frame: false,
            confirm_eject: false,
            catalog,
            catalog_rx,
            model_search: String::new(),
            mode: AppMode::Chat,
            coder: CoderState::default(),
            settings,
            settings_ui: SettingsUi::default(),
            credential: CredentialStatus::from_key(Some(&api_key)),
            banner_key_input: String::new(),
            banner_show_key: false,
            retry_after_auth: None,
            draft_images: Vec::new(),
            lightbox: None,
            chat_search: String::new(),
            focus_search_next_frame: false,
            renaming_chat: None,
            pending_confirm: None,
            editing_coder: None,
        })
    }

    pub fn poll_catalog(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(rx) = &state.catalog_rx else {
            return;
        };
        let Ok(catalog) = rx.try_recv() else {
            return;
        };
        state.catalog = catalog;
        state.catalog_rx = None;
    }

    pub fn start_validation(&mut self) {
        let Screen::Onboarding(state) = &mut self.screen else {
            return;
        };
        let key = state.key_input.trim().to_string();
        if key.is_empty() {
            state.status = OnboardingStatus::Error("Please enter an API key.".into());
            return;
        }

        let (tx, rx): (Sender<ValidationResult>, Receiver<ValidationResult>) = mpsc::channel();
        state.rx = Some(rx);
        state.status = OnboardingStatus::Validating;

        let rt = self.rt.clone();
        let key_for_task = key.clone();
        rt.spawn(async move {
            match validate_openrouter_key(key_for_task.clone()).await {
                Ok(()) => {
                    let _ = tx.send(ValidationResult::Ok(key_for_task));
                }
                Err(ProviderError::Unauthorized) => {
                    let _ = tx.send(ValidationResult::Err(
                        "That key was rejected by OpenRouter.".into(),
                    ));
                }
                Err(e) => {
                    let _ = tx.send(ValidationResult::Err(format!("{e}")));
                }
            }
        });
    }

    pub fn poll_validation(&mut self) {
        let Screen::Onboarding(state) = &mut self.screen else {
            return;
        };
        let Some(rx) = &state.rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        state.rx = None;

        match result {
            ValidationResult::Ok(key) => {
                if let Err(e) = SecureStore::save_key(&key) {
                    state.status =
                        OnboardingStatus::Error(format!("Couldn't save key securely: {e}"));
                    return;
                }
                match Self::build_main_state(key, &self.rt) {
                    Ok(main) => {
                        self.screen = Screen::Main(Box::new(Self::with_initial_chat(main)));
                    }
                    Err(e) => {
                        state.status =
                            OnboardingStatus::Error(format!("Couldn't initialize app: {e}"));
                    }
                }
            }
            ValidationResult::Err(msg) => {
                state.status = OnboardingStatus::Error(msg);
            }
        }
    }

    fn with_initial_chat(mut main: MainState) -> MainState {
        if main.chats.is_empty() {
            let chat = Chat::new(main.settings.chat_default_model.clone());
            main.active_chat_id = Some(chat.id);
            main.chats.push(chat);
        }
        main
    }

    pub fn new_chat(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        let model = state.settings.chat_default_model.clone();
        if state.temporary_mode {
            if let Some(old) = state.temp_chat.take() {
                state.dispose_chat(old.id);
            }
            state.temp_chat = Some(Chat::new(model));
            state.focus_input_next_frame = true;
            return;
        }
        let chat = Chat::new(model);
        state.active_chat_id = Some(chat.id);
        state.chats.insert(0, chat);
        state.focus_input_next_frame = true;
        state.lightbox = None;
        self.persist_open_chats();
    }

    pub fn select_chat(&mut self, id: Uuid) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.temporary_mode || !state.chats.iter().any(|c| c.id == id) {
            return;
        }
        state.active_chat_id = Some(id);
        state.lightbox = None;
        state.focus_input_next_frame = true;
    }

    pub fn delete_chat(&mut self, id: Uuid) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.dispose_chat(id);
        state.chats.retain(|c| c.id != id);
        if state.active_chat_id == Some(id) {
            state.active_chat_id = state.chats.first().map(|c| c.id);
        }
        if state.chats.is_empty() && !state.temporary_mode {
            let chat = Chat::new(state.settings.chat_default_model.clone());
            state.active_chat_id = Some(chat.id);
            state.chats.push(chat);
        }
        self.persist_open_chats();
    }

    pub fn set_temporary_mode(&mut self, on: bool) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.temporary_mode == on {
            return;
        }
        state.temporary_mode = on;
        if on {
            state.temp_chat = Some(Chat::new(state.settings.chat_default_model.clone()));
        } else {
            if let Some(old) = state.temp_chat.take() {
                state.dispose_chat(old.id);
            }
        }
        state.lightbox = None;
        state.focus_input_next_frame = true;
    }

    pub fn send_message(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.active_chat_pending() {
            return;
        }
        let Some(chat) = state.active_chat() else {
            return;
        };
        let chat_id = chat.id;
        let preview_model = chat.model.clone();
        let draft = state.chat_ui.entry(chat_id).or_default();
        let text = draft.input.trim().to_string();
        if text.is_empty() && draft.draft_images.is_empty() {
            return;
        }
        if !draft.draft_images.is_empty()
            && !state
                .catalog
                .find(&preview_model)
                .is_some_and(|m| m.supports_vision)
        {
            draft.retry_hint = Some(
                "This model is text-only. Switch to a vision model or remove the image.".into(),
            );
            return;
        }
        if resolve_provider(state, &preview_model).is_none() {
            state.chat_ui.entry(chat_id).or_default().retry_hint =
                Some("Configure a provider for this model before sending.".into());
            return;
        }
        let draft = state.chat_ui.entry(chat_id).or_default();
        let images = std::mem::take(&mut draft.draft_images);
        draft.input.clear();
        draft.editing = None;
        let Some(chat) = state.active_chat_mut() else {
            return;
        };
        chat.messages.push(Message {
            role: Role::User,
            content: text.clone(),
            appeared_at: Some(Instant::now()),
            interrupted: false,
            images,
            sources: Vec::new(),
        });
        if chat.title == "New chat" {
            chat.title = text.chars().take(40).collect::<String>();
            if text.chars().count() > 40 {
                chat.title.push('…');
            }
        }
        self.start_chat_stream(chat_id);
    }

    pub fn regenerate_chat(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.active_chat_pending() {
            return;
        }
        let Some(chat) = state.active_chat() else {
            return;
        };
        let chat_id = chat.id;
        if resolve_provider(state, &chat.model).is_none() {
            state.chat_ui.entry(chat_id).or_default().retry_hint =
                Some("Configure a provider for this model before retrying.".into());
            return;
        }
        let Some(chat) = state.active_chat_mut() else {
            return;
        };
        if !crate::session::message_ops::discard_last_chat_assistant(&mut chat.messages) {
            return;
        }
        self.invalidate_chat_summary();
        self.start_chat_stream(chat_id);
    }

    pub fn edit_resend_chat(&mut self, index: usize, text: String) {
        let Screen::Main(state) = &self.screen else {
            return;
        };
        let Some(chat) = state.active_chat() else {
            return;
        };
        self.edit_resend_chat_for(chat.id, index, text);
    }

    pub fn edit_resend_chat_for(&mut self, chat_id: Uuid, index: usize, text: String) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.pending.contains_key(&chat_id) {
            return;
        }
        let Some(chat) = state.chat(chat_id) else {
            return;
        };
        if !chat
            .messages
            .get(index)
            .is_some_and(|m| matches!(m.role, Role::User))
        {
            return;
        }
        if resolve_provider(state, &chat.model).is_none() {
            state.chat_ui.entry(chat_id).or_default().retry_hint =
                Some("Configure a provider for this model before retrying.".into());
            return;
        }
        let Some(chat) = state.chat_mut(chat_id) else {
            return;
        };
        crate::session::message_ops::truncate_chat_from(&mut chat.messages, index, text);
        if chat.context_summary_upto > chat.messages.len() {
            chat.context_summary = None;
            chat.context_summary_upto = 0;
        }
        state.chat_ui.entry(chat_id).or_default().editing = None;
        state.pending_confirm = None;
        self.start_chat_stream(chat_id);
    }

    pub fn delete_chat_pair(&mut self, index: usize) {
        let Screen::Main(state) = &self.screen else {
            return;
        };
        let Some(chat) = state.active_chat() else {
            return;
        };
        self.delete_chat_pair_for(chat.id, index);
    }

    pub fn delete_chat_pair_for(&mut self, chat_id: Uuid, index: usize) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.pending.contains_key(&chat_id) {
            return;
        }
        let Some(chat) = state.chat_mut(chat_id) else {
            return;
        };
        crate::session::message_ops::delete_chat_pair(&mut chat.messages, index);
        if chat.context_summary_upto > chat.messages.len() {
            chat.context_summary = None;
            chat.context_summary_upto = 0;
        }
        state.pending_confirm = None;
        let chat_ui = state.chat_ui.entry(chat_id).or_default();
        chat_ui.editing = None;
        chat_ui.auth_rejected = false;
        self.persist_open_chats();
    }

    pub fn rename_chat(&mut self, id: Uuid, title: String) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        let title = title.trim();
        if title.is_empty() {
            state.renaming_chat = None;
            return;
        }
        if let Some(chat) = state.chats.iter_mut().find(|c| c.id == id) {
            chat.title = title.to_string();
        }
        state.renaming_chat = None;
        self.persist_open_chats();
    }

    pub fn toggle_pin_chat(&mut self, id: Uuid) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if let Some(chat) = state.chats.iter_mut().find(|c| c.id == id) {
            chat.pinned = !chat.pinned;
        }
        self.persist_open_chats();
    }

    pub fn cycle_chat(&mut self, delta: i32) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.temporary_mode || state.chats.is_empty() {
            return;
        }
        let current = state
            .active_chat_id
            .and_then(|id| state.chats.iter().position(|c| c.id == id))
            .unwrap_or(0);
        let len = state.chats.len() as i32;
        let next = (current as i32 + delta).rem_euclid(len) as usize;
        state.active_chat_id = Some(state.chats[next].id);
        state.lightbox = None;
        state.focus_input_next_frame = true;
    }

    pub fn nudge_font_scale(&mut self, delta: f32) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.settings.font_scale = (state.settings.font_scale + delta)
            .clamp(storage::MIN_FONT_SCALE, storage::MAX_FONT_SCALE);
        let _ = storage::save_settings(&state.settings);
    }

    pub fn reset_font_scale(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.settings.font_scale = 1.0;
        let _ = storage::save_settings(&state.settings);
    }

    pub fn persist_theme_settings(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        let _ = storage::save_settings(&state.settings);
    }

    pub fn export_active_chat(&self) -> Option<String> {
        let Screen::Main(state) = &self.screen else {
            return None;
        };
        state
            .active_chat()
            .map(crate::session::export::chat_to_markdown)
    }

    pub fn save_markdown(&self, suggested: &str, markdown: &str) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(suggested)
            .add_filter("Markdown", &["md"])
            .save_file()
        else {
            return;
        };
        if let Err(e) = std::fs::write(&path, markdown) {
            tracing::warn!("could not export markdown: {e:#}");
        }
    }

    fn persist_open_chats(&self) {
        let Screen::Main(state) = &self.screen else {
            return;
        };
        #[cfg(test)]
        if let Some(path) = &self.chat_history_path {
            storage::save_chats_at(path, &state.chats).expect("test chat history");
        } else {
            let _ = storage::save_chats(&state.chats);
        }
        #[cfg(not(test))]
        let _ = storage::save_chats(&state.chats);
        let db = self.db.clone();
        let chats = state.chats.clone();
        self.rt.spawn_blocking(move || {
            if let Err(e) = db.reindex_chats(&chats) {
                tracing::warn!("could not reindex chats: {e:#}");
            }
        });
    }

    fn invalidate_chat_summary(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if let Some(chat) = state.active_chat_mut()
            && chat.context_summary_upto > chat.messages.len()
        {
            chat.context_summary = None;
            chat.context_summary_upto = 0;
        }
    }

    fn start_chat_stream(&mut self, chat_id: Uuid) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.pending.contains_key(&chat_id) {
            return;
        }
        let recent_keep = state.settings.context_recent_messages.max(1);
        let preview_model = state
            .chat(chat_id)
            .map(|c| c.model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let context_length = state
            .catalog
            .find(&preview_model)
            .and_then(|m| m.context_length)
            .unwrap_or(crate::session::context_window::DEFAULT_CONTEXT_LENGTH);

        let Some(provider) = resolve_provider(state, &preview_model) else {
            state.chat_ui.entry(chat_id).or_default().retry_hint =
                Some("Configure a provider for this model before retrying.".into());
            return;
        };
        let search = if state
            .chat(chat_id)
            .is_some_and(|chat| chat.web_search.enabled())
        {
            if matches!(provider.id(), OPENROUTER | ANTHROPIC) {
                Some(ChatSearchRoute::Hosted)
            } else {
                if !provider.supports_tools(&preview_model) {
                    state.chat_ui.entry(chat_id).or_default().retry_hint = Some(
                        "Web search needs a tool-capable model. Choose another model or turn search off."
                            .into(),
                    );
                    return;
                }
                let Some(key) = SecureStore::load_key_for(TAVILY).ok().flatten() else {
                    state.chat_ui.entry(chat_id).or_default().retry_hint = Some(
                        "Add a Tavily API key in Settings → Tavily before using web search.".into(),
                    );
                    return;
                };
                match TavilySearch::new(key) {
                    Ok(search) => Some(ChatSearchRoute::Tavily(search)),
                    Err(e) => {
                        state.chat_ui.entry(chat_id).or_default().retry_hint =
                            Some(format!("Could not start web search: {e}"));
                        return;
                    }
                }
            }
        } else {
            None
        };
        let chat_ui = state.chat_ui.entry(chat_id).or_default();
        chat_ui.retry_hint = None;
        chat_ui.auth_rejected = false;
        chat_ui.generation = chat_ui.generation.wrapping_add(1);
        let Some(chat) = state.chat_mut(chat_id) else {
            return;
        };
        if chat.messages.is_empty() {
            return;
        }
        let model = chat.model.clone();
        let system = chat.request_system();

        let raw_history: Vec<ChatMessage> = chat
            .messages
            .iter()
            .map(|m| match m.role {
                Role::User => ChatMessage::User {
                    content: m.content.clone(),
                    images: m.images.clone(),
                },
                Role::Assistant => ChatMessage::Assistant {
                    content: m.content.clone(),
                    tool_calls: Vec::new(),
                },
            })
            .collect();
        let cached = chat.context_summary.clone().map(|text| {
            crate::session::context_window::CachedSummary {
                text,
                covered: chat.context_summary_upto,
            }
        });
        let fitted = crate::session::context_window::fit(
            system.as_deref(),
            &raw_history,
            cached.as_ref(),
            context_length,
            &crate::session::context_window::ContextWindow {
                recent_keep,
                response_reserve: crate::session::context_window::DEFAULT_RESPONSE_RESERVE,
            },
        );
        chat.context_occupancy = Some(fitted.occupancy);
        let history = fitted.messages;

        let assistant_index = chat.messages.len();
        chat.messages.push(Message {
            role: Role::Assistant,
            content: String::new(),
            appeared_at: Some(Instant::now()),
            interrupted: false,
            images: Vec::new(),
            sources: Vec::new(),
        });

        let (tx, rx) = mpsc::channel::<StreamUiEvent>();
        let cancel = CancellationToken::new();
        state.pending.insert(
            chat_id,
            PendingResponse {
                assistant_index,
                rx,
                cancel: cancel.clone(),
            },
        );

        self.persist_open_chats();
        self.rt.spawn(run_chat_turn(
            provider, model, system, history, search, cancel, tx,
        ));
    }

    pub fn cancel_pending(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if let Some(pending) = state.active_chat().and_then(|c| state.pending.get(&c.id)) {
            pending.cancel.cancel();
        }
    }

    pub fn export_diagnostics(&mut self) {
        let suggested = format!("orbit-diagnostics-{}.zip", env!("CARGO_PKG_VERSION"));
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(&suggested)
            .add_filter("Zip", &["zip"])
            .save_file()
        else {
            return;
        };
        match crate::diagnostics::export_bundle(&path) {
            Ok(saved) => tracing::info!("wrote diagnostics to {}", saved.display()),
            Err(e) => tracing::warn!("could not export diagnostics: {e:#}"),
        }
    }

    pub fn eject_key(&mut self) {
        if let Err(e) = SecureStore::delete_key() {
            tracing::warn!("couldn't delete cached key: {e:#}");
        }
        self.screen = Screen::Onboarding(OnboardingState {
            key_input: String::new(),
            show_key: false,
            status: OnboardingStatus::Idle,
            rx: None,
        });
    }

    pub fn open_settings(&mut self, tab: SettingsTab) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.settings_ui.open = true;
        state.settings_ui.tab = tab;
    }

    pub fn close_settings(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.settings_ui.open = false;
        state.settings_ui.confirm_remove = false;
    }

    pub fn start_key_test(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if matches!(state.settings_ui.test_status, KeyTestStatus::Testing) {
            return;
        }
        let typed = state.settings_ui.key_input.trim().to_string();
        let key = if typed.is_empty() {
            match SecureStore::load_key() {
                Ok(Some(key)) => key,
                Ok(None) => {
                    state.settings_ui.test_status =
                        KeyTestStatus::Err("No API key configured.".into());
                    return;
                }
                Err(e) => {
                    state.settings_ui.test_status = KeyTestStatus::Err(format!("{e}"));
                    return;
                }
            }
        } else {
            typed
        };

        self.start_provider_key_test(OPENROUTER, key);
    }

    pub fn start_provider_key_test(&mut self, provider: &'static str, key: String) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        let timeout = std::time::Duration::from_secs(state.settings.request_timeout_secs);
        let base = state.settings.openai_compat_base_url.clone();
        let (tx, rx) = mpsc::channel();
        state.settings_ui.test_rx = Some(rx);
        state.settings_ui.test_status = KeyTestStatus::Testing;
        state.settings_ui.test_provider = provider.to_string();
        self.rt.spawn(async move {
            let result = match validate_provider_key(provider, key.clone(), timeout, &base).await {
                Ok(()) => ValidationResult::Ok(key),
                Err(ProviderError::Unauthorized) => ValidationResult::Err(format!(
                    "That key was rejected by {}.",
                    crate::providers::catalog::provider_label(provider)
                )),
                Err(e) => ValidationResult::Err(format!("{e}")),
            };
            let _ = tx.send(result);
        });
    }

    pub fn poll_key_test(&mut self) {
        let outcome = {
            let Screen::Main(state) = &mut self.screen else {
                return;
            };
            let Some(rx) = &state.settings_ui.test_rx else {
                return;
            };
            let Ok(result) = rx.try_recv() else {
                return;
            };
            state.settings_ui.test_rx = None;
            let testing_stored = state.settings_ui.key_input.trim().is_empty();
            (result, testing_stored)
        };
        match outcome {
            (ValidationResult::Ok(key), _) => {
                let provider = if let Screen::Main(state) = &self.screen {
                    state.settings_ui.test_provider.clone()
                } else {
                    OPENROUTER.to_string()
                };
                if let Err(e) = self.apply_provider_key(&provider, key) {
                    if let Screen::Main(state) = &mut self.screen {
                        state.settings_ui.test_status = KeyTestStatus::Err(format!("{e}"));
                    }
                    return;
                }
                if let Screen::Main(state) = &mut self.screen {
                    state.settings_ui.test_status = KeyTestStatus::Ok;
                    state.settings_ui.key_input.clear();
                    state.banner_key_input.clear();
                }
                self.retry_after_auth_fix();
            }
            (ValidationResult::Err(msg), testing_stored) => {
                if let Screen::Main(state) = &mut self.screen {
                    if testing_stored {
                        state.credential.state = CredentialState::Rejected;
                    }
                    state.settings_ui.test_status = KeyTestStatus::Err(msg);
                }
            }
        }
    }

    pub fn apply_provider_key(&mut self, provider: &str, key: String) -> anyhow::Result<()> {
        if !key.trim().is_empty() {
            SecureStore::save_key_for(provider, &key)?;
        }
        let timeout = match &self.screen {
            Screen::Main(state) => {
                std::time::Duration::from_secs(state.settings.request_timeout_secs)
            }
            _ => std::time::Duration::from_secs(storage::DEFAULT_REQUEST_TIMEOUT_SECS),
        };
        let Screen::Main(state) = &mut self.screen else {
            return Ok(());
        };
        state.providers = build_provider_hub(timeout, &state.settings.openai_compat_base_url);
        state.provider = state.providers.get(OPENROUTER);
        if provider == OPENROUTER {
            state.credential = CredentialStatus::from_key(Some(&key));
        }
        Ok(())
    }

    pub fn remove_api_key(&mut self) {
        if let Err(e) = SecureStore::delete_key() {
            tracing::warn!("couldn't delete cached key: {e:#}");
        }
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.providers = build_provider_hub(
            std::time::Duration::from_secs(state.settings.request_timeout_secs),
            &state.settings.openai_compat_base_url,
        );
        state.provider = state.providers.get(OPENROUTER);
        state.credential = CredentialStatus::from_key(None);
        state.settings_ui.confirm_remove = false;
        state.settings_ui.test_status = KeyTestStatus::Idle;
        state.settings_ui.key_input.clear();
    }

    pub fn start_banner_retry(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if matches!(state.settings_ui.test_status, KeyTestStatus::Testing) {
            return;
        }
        if !state.banner_key_input.trim().is_empty() {
            state.settings_ui.key_input = state.banner_key_input.clone();
        }
        // Capture the origin before asynchronous key validation or navigation.
        state.retry_after_auth = state.auth_retry_target();
        self.start_key_test();
    }

    pub fn retry_after_auth_fix(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(target) = state.retry_after_auth.take() else {
            return;
        };
        match target {
            AuthRetryTarget::Chat {
                chat_id,
                generation,
            } => {
                if state.pending.contains_key(&chat_id)
                    || !state
                        .chat_ui
                        .get(&chat_id)
                        .is_some_and(|s| s.auth_rejected && s.generation == generation)
                {
                    return;
                }
                let Some(chat) = state.chat(chat_id) else {
                    return;
                };
                if resolve_provider(state, &chat.model).is_none() {
                    return;
                }
                let Some(chat) = state.chat_mut(chat_id) else {
                    return;
                };
                if !chat.messages.last().is_some_and(|m| {
                    matches!(m.role, Role::Assistant) && m.content == AUTH_REJECTED_NOTICE
                }) {
                    return;
                }
                chat.messages.pop();
                self.start_chat_stream(chat_id);
            }
            AuthRetryTarget::Coder(id) => {
                // The existing Coder resume method operates on the selected session.
                // Select its captured origin only for dispatch, then restore navigation.
                let previous = state.coder.sessions.active;
                let Some(index) = state
                    .coder
                    .sessions
                    .sessions
                    .iter()
                    .position(|s| s.id == id)
                else {
                    return;
                };
                state.coder.sessions.active = index;
                self.resume_coder_after_auth();
                if let Screen::Main(state) = &mut self.screen {
                    state.coder.sessions.active = previous;
                }
            }
        }
    }

    pub fn rebuild_provider_timeout(&mut self, timeout_secs: u64) {
        let Screen::Main(state) = &self.screen else {
            return;
        };
        let base = state.settings.openai_compat_base_url.clone();
        let hub = build_provider_hub(std::time::Duration::from_secs(timeout_secs), &base);
        if let Screen::Main(state) = &mut self.screen {
            state.provider = hub.get(OPENROUTER);
            state.providers = hub;
        }
    }

    pub fn poll_pending(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        let ids: Vec<_> = state.pending.keys().copied().collect();
        let mut persist = false;
        for chat_id in ids {
            if state.chat(chat_id).is_none() {
                state.dispose_chat(chat_id);
                continue;
            }
            let Some(pending) = state.pending.get(&chat_id) else {
                continue;
            };
            let assistant_index = pending.assistant_index;
            let mut events = Vec::new();
            // Give every chat a chance to advance without monopolizing the UI frame.
            for _ in 0..256 {
                match pending.rx.try_recv() {
                    Ok(event) => {
                        let terminal = matches!(
                            event,
                            StreamUiEvent::Done
                                | StreamUiEvent::Cancelled
                                | StreamUiEvent::Error(_)
                                | StreamUiEvent::Unauthorized
                        );
                        events.push(event);
                        if terminal {
                            break;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        events.push(StreamUiEvent::Error(
                            "Response stream disconnected before completion.".into(),
                        ));
                        break;
                    }
                }
            }
            for event in events {
                let Some(last) = state
                    .chat_mut(chat_id)
                    .and_then(|c| c.messages.get_mut(assistant_index))
                    .filter(|m| matches!(m.role, Role::Assistant))
                else {
                    state.pending.remove(&chat_id);
                    state.chat_ui.entry(chat_id).or_default().retry_hint = None;
                    break;
                };
                let mut finished = false;
                match event {
                    StreamUiEvent::Delta(text) => last.content.push_str(&text),
                    StreamUiEvent::Sources(sources) => last.sources = sources,
                    StreamUiEvent::Searching => {
                        state.chat_ui.entry(chat_id).or_default().search_status =
                            Some("Searching the web…".into());
                    }
                    StreamUiEvent::Done => finished = true,
                    StreamUiEvent::Error(e) => {
                        if !last.content.is_empty() {
                            last.content.push_str("\n\n");
                        }
                        last.content.push_str(&format!("⚠ Error: {e}"));
                        last.interrupted = true;
                        finished = true;
                    }
                    StreamUiEvent::Unauthorized => {
                        last.content = AUTH_REJECTED_NOTICE.into();
                        state.chat_ui.entry(chat_id).or_default().auth_rejected = true;
                        state.credential.state = CredentialState::Rejected;
                        finished = true;
                    }
                    StreamUiEvent::Retrying {
                        attempt,
                        max_attempts,
                        wait_secs,
                    } => {
                        state.chat_ui.entry(chat_id).or_default().retry_hint = Some(format!(
                            "Retrying in {wait_secs}s… ({attempt}/{max_attempts})"
                        ));
                    }
                    StreamUiEvent::Cancelled => {
                        last.interrupted = true;
                        if last.content.is_empty() {
                            last.content = "*(interrupted)*".into();
                        }
                        finished = true;
                    }
                }
                if finished {
                    state.chat_ui.entry(chat_id).or_default().retry_hint = None;
                    state.chat_ui.entry(chat_id).or_default().search_status = None;
                    state.pending.remove(&chat_id);
                    persist |= state.chats.iter().any(|c| c.id == chat_id);
                    if state.mode == AppMode::Chat
                        && !state.settings_ui.open
                        && state.active_chat().is_some_and(|c| c.id == chat_id)
                    {
                        state.focus_input_next_frame = true;
                    }
                    break;
                }
            }
        }
        if persist {
            self.persist_open_chats();
        }
    }
}

async fn run_chat_turn(
    provider: Arc<dyn AiProvider>,
    model: String,
    system: Option<String>,
    mut messages: Vec<ChatMessage>,
    search: Option<ChatSearchRoute>,
    cancel: CancellationToken,
    tx: Sender<StreamUiEvent>,
) {
    let mut sources = Vec::new();
    let mut search_calls = 0usize;
    loop {
        if cancel.is_cancelled() {
            let _ = tx.send(StreamUiEvent::Cancelled);
            return;
        }
        let request = ChatRequest {
            model: model.clone(),
            system: system.clone(),
            messages: messages.clone(),
            tools: match search.as_ref() {
                Some(ChatSearchRoute::Hosted) => vec![hosted_web_search_schema()],
                Some(ChatSearchRoute::Tavily(_)) => vec![web_search_schema()],
                None => Vec::new(),
            },
            temperature: None,
            max_output_tokens: None,
            system_cache_chars: 0,
        };
        let mut stream = match provider.stream_chat(request, cancel.clone()).await {
            Ok(stream) => stream,
            Err(ProviderError::Unauthorized) => {
                let _ = tx.send(StreamUiEvent::Unauthorized);
                return;
            }
            Err(ProviderError::Cancelled) => {
                let _ = tx.send(StreamUiEvent::Cancelled);
                return;
            }
            Err(error) => {
                let _ = tx.send(StreamUiEvent::Error(error.to_string()));
                return;
            }
        };
        let mut accumulator = AssistantAccumulator::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(ProviderEvent::TextDelta(text)) => {
                    accumulator.push_text(&text);
                    if tx.send(StreamUiEvent::Delta(text)).is_err() {
                        return;
                    }
                }
                Ok(ProviderEvent::Retrying {
                    attempt,
                    max_attempts,
                    wait_secs,
                }) => {
                    let _ = tx.send(StreamUiEvent::Retrying {
                        attempt,
                        max_attempts,
                        wait_secs,
                    });
                }
                Ok(event) => accumulator.push_event(event),
                Err(ProviderError::Unauthorized) => {
                    let _ = tx.send(StreamUiEvent::Unauthorized);
                    return;
                }
                Err(ProviderError::Cancelled) => {
                    let _ = tx.send(StreamUiEvent::Cancelled);
                    return;
                }
                Err(error) => {
                    let _ = tx.send(StreamUiEvent::Error(error.to_string()));
                    return;
                }
            }
        }
        let assistant = match accumulator.finish() {
            Ok(assistant) => assistant,
            Err(error) => {
                let _ = tx.send(StreamUiEvent::Error(format!(
                    "Invalid tool call from model: {error}"
                )));
                return;
            }
        };
        if !matches!(assistant.finish, crate::providers::FinishReason::ToolCalls) {
            if !sources.is_empty() {
                let _ = tx.send(StreamUiEvent::Sources(sources));
            }
            let _ = tx.send(StreamUiEvent::Done);
            return;
        }
        let Some(ChatSearchRoute::Tavily(search)) = &search else {
            let _ = tx.send(StreamUiEvent::Error(
                "The model requested a tool that Chat Mode does not provide.".into(),
            ));
            return;
        };
        if assistant.tool_calls.is_empty() || search_calls >= MAX_SEARCHES_PER_TURN {
            let _ = tx.send(StreamUiEvent::Error(
                "Web search reached its per-message limit.".into(),
            ));
            return;
        }
        messages.push(ChatMessage::Assistant {
            content: assistant.content,
            tool_calls: assistant.tool_calls.clone(),
        });
        for call in assistant.tool_calls {
            if call.name != "web_search" {
                let _ = tx.send(StreamUiEvent::Error(
                    "Chat Mode only permits its web-search tool.".into(),
                ));
                return;
            }
            let Some(query) = call
                .arguments
                .get("query")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|query| !query.is_empty())
            else {
                messages.push(ChatMessage::ToolResult {
                    call_id: call.id,
                    content: "Search query was missing.".into(),
                    is_error: true,
                });
                continue;
            };
            if search_calls >= MAX_SEARCHES_PER_TURN {
                let _ = tx.send(StreamUiEvent::Error(
                    "Web search reached its per-message limit.".into(),
                ));
                return;
            }
            search_calls += 1;
            let _ = tx.send(StreamUiEvent::Searching);
            match search.search(query, cancel.clone()).await {
                Ok(mut found) => {
                    for source in &mut found {
                        source.id = format!("S{}", sources.len() + 1);
                        sources.push(source.clone());
                    }
                    messages.push(ChatMessage::ToolResult {
                        call_id: call.id,
                        content: format_tool_result(&found),
                        is_error: false,
                    });
                }
                Err(ProviderError::Cancelled) => {
                    let _ = tx.send(StreamUiEvent::Cancelled);
                    return;
                }
                Err(error) => messages.push(ChatMessage::ToolResult {
                    call_id: call.id,
                    content: format!("Search failed: {error}"),
                    is_error: true,
                }),
            }
        }
    }
}

#[cfg(test)]
mod chat_tests;

#[cfg(test)]
mod tests {
    use super::{Chat, Role};
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn legacy_chat_json_without_system_or_pin_still_loads() {
        let json = r#"{
            "id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "title": "Old chat",
            "model": "openai/gpt-4.1",
            "messages": [{"role": "User", "content": "hi", "interrupted": false}],
            "created_at": "2024-01-01T00:00:00Z"
        }"#;
        let chat: Chat = serde_json::from_str(json).expect("legacy chat");
        assert!(chat.system.is_none());
        assert!(!chat.pinned);
        assert_eq!(chat.messages.len(), 1);
        assert!(matches!(chat.messages[0].role, Role::User));
    }

    #[test]
    fn request_system_omits_blank_and_keeps_defined_prompt() {
        let mut chat = Chat::new("test".into());
        assert_eq!(chat.request_system(), None);
        chat.system = Some("   ".into());
        assert_eq!(chat.request_system(), None);
        chat.system = Some("Answer only in JSON.".into());
        assert_eq!(
            chat.request_system().as_deref(),
            Some("Answer only in JSON.")
        );
    }

    #[test]
    fn mask_api_key_never_shows_the_secret() {
        assert_eq!(
            super::mask_api_key("sk-or-v1-abcdefgh1a2b"),
            "sk-or-v1-••••1a2b"
        );
        assert_eq!(super::mask_api_key(""), "");
        assert_eq!(super::mask_api_key("abcd"), "••••");
        let masked = super::mask_api_key("sk-or-v1-SECRETxxxxZZ9");
        assert!(!masked.contains("SECRET"));
        assert!(masked.ends_with("xZZ9") || masked.ends_with("ZZ9"));
    }

    #[test]
    fn credential_state_follows_stored_key() {
        let present = super::CredentialStatus::from_key(Some("sk-or-v1-abcdefgh1a2b"));
        assert_eq!(
            super::credential_state(&present),
            super::CredentialState::Present
        );
        assert_eq!(present.masked, "sk-or-v1-••••1a2b");
        let missing = super::CredentialStatus::from_key(None);
        assert_eq!(
            super::credential_state(&missing),
            super::CredentialState::Missing
        );
    }

    #[test]
    fn can_create_session_only_when_present() {
        assert!(super::can_create_session(super::CredentialState::Present).is_ok());
        assert_eq!(
            super::can_create_session(super::CredentialState::Missing).unwrap_err(),
            super::CredentialState::Missing
        );
        assert_eq!(
            super::can_create_session(super::CredentialState::Rejected).unwrap_err(),
            super::CredentialState::Rejected
        );
    }

    #[test]
    fn chat_round_trips_system_prompt() {
        let chat = Chat {
            id: Uuid::nil(),
            title: "t".into(),
            model: "m".into(),
            messages: Vec::new(),
            created_at: Utc::now(),
            system: Some("Be terse.".into()),
            context_summary: None,
            context_summary_upto: 0,
            context_occupancy: None,
            pinned: false,
            web_search: ChatSearchMode::Off,
        };
        let json = serde_json::to_string(&chat).unwrap();
        let loaded: Chat = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.system.as_deref(), Some("Be terse."));
    }
}
