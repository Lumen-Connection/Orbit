use crate::coder::{AppMode, CoderState};
use crate::providers::catalog::ModelCatalog;
use crate::providers::{
    AiProvider, ChatMessage, ChatRequest, ProviderError, ProviderEvent, connect_openrouter_timed,
    validate_openrouter_key,
};
use crate::secure_store::SecureStore;
use crate::storage::{self, AppSettings, Db};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
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
}

pub struct PendingResponse {
    pub chat_id: Uuid,
    pub rx: Receiver<StreamUiEvent>,
    pub cancel: CancellationToken,
}

pub struct MainState {
    pub provider: Option<Arc<dyn AiProvider>>,
    pub chats: Vec<Chat>,
    pub active_chat_id: Option<Uuid>,
    pub temp_chat: Option<Chat>,
    pub temporary_mode: bool,
    pub input: String,
    pub pending: Option<PendingResponse>,
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
    pub retry_after_auth: bool,
    pub retry_hint: Option<String>,
    pub draft_images: Vec<crate::providers::ImageAttachment>,
    pub lightbox: Option<crate::providers::ImageAttachment>,
    pub chat_search: String,
    pub focus_search_next_frame: bool,
    pub renaming_chat: Option<(Uuid, String)>,
    pub pending_confirm: Option<PendingConfirm>,
    pub editing_chat: Option<MessageEdit>,
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
        index: usize,
        count: usize,
    },
    EditResendChat {
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

        Ok(Self { screen, rt, db })
    }

    fn build_main_state(api_key: String, rt: &Runtime) -> anyhow::Result<MainState> {
        let settings = storage::load_settings();
        let timeout = std::time::Duration::from_secs(settings.request_timeout_secs);
        let provider = connect_openrouter_timed(api_key.clone(), timeout)?;
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
            let fetch_provider = provider.clone();
            rt.spawn(async move {
                match fetch_provider.list_models().await {
                    Ok(models) => {
                        let models = models.into_iter().map(Into::into).collect();
                        let catalog = ModelCatalog::from_remote(models, Utc::now());
                        if let Err(e) = catalog.save_cache() {
                            tracing::warn!("couldn't save model catalog cache: {e:#}");
                        }
                        let _ = tx.send(catalog);
                    }
                    Err(e) => tracing::warn!("couldn't refresh model catalog: {e:#}"),
                }
            });
            Some(rx)
        };

        Ok(MainState {
            provider: Some(provider),
            chats,
            active_chat_id,
            temp_chat: None,
            temporary_mode: false,
            input: String::new(),
            pending: None,
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
            retry_after_auth: false,
            retry_hint: None,
            draft_images: Vec::new(),
            lightbox: None,
            chat_search: String::new(),
            focus_search_next_frame: false,
            renaming_chat: None,
            pending_confirm: None,
            editing_chat: None,
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
            state.temp_chat = Some(Chat::new(model));
            return;
        }
        let chat = Chat::new(model);
        state.active_chat_id = Some(chat.id);
        state.chats.insert(0, chat);
        let _ = storage::save_chats(&state.chats);
        state.focus_input_next_frame = true;
    }

    pub fn select_chat(&mut self, id: Uuid) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.temporary_mode {
            return;
        }
        state.active_chat_id = Some(id);
        state.focus_input_next_frame = true;
    }

    pub fn delete_chat(&mut self, id: Uuid) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.chats.retain(|c| c.id != id);
        if state.active_chat_id == Some(id) {
            state.active_chat_id = state.chats.first().map(|c| c.id);
        }
        if state.chats.is_empty() && !state.temporary_mode {
            let chat = Chat::new(state.settings.chat_default_model.clone());
            state.active_chat_id = Some(chat.id);
            state.chats.push(chat);
        }
        let _ = storage::save_chats(&state.chats);
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
            state.temp_chat = None;
        }
        state.focus_input_next_frame = true;
    }

    pub fn send_message(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.pending.is_some() {
            return;
        }
        let text = state.input.trim().to_string();
        if text.is_empty() && state.draft_images.is_empty() {
            return;
        }
        let preview_model = state
            .active_chat()
            .map(|c| c.model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        if !state.draft_images.is_empty()
            && !state
                .catalog
                .find(&preview_model)
                .is_some_and(|m| m.supports_vision)
        {
            state.retry_hint = Some(
                "This model is text-only. Switch to a vision model or remove the image.".into(),
            );
            return;
        }
        let images = std::mem::take(&mut state.draft_images);
        let Some(chat) = state.active_chat_mut() else {
            return;
        };
        chat.messages.push(Message {
            role: Role::User,
            content: text.clone(),
            appeared_at: Some(Instant::now()),
            interrupted: false,
            images,
        });
        if chat.title == "New chat" {
            chat.title = text.chars().take(40).collect::<String>();
            if text.chars().count() > 40 {
                chat.title.push('…');
            }
        }
        state.input.clear();
        self.start_chat_stream();
    }

    pub fn regenerate_chat(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.pending.is_some() {
            return;
        }
        let Some(chat) = state.active_chat_mut() else {
            return;
        };
        if !crate::session::message_ops::discard_last_chat_assistant(&mut chat.messages) {
            return;
        }
        self.invalidate_chat_summary();
        self.start_chat_stream();
    }

    pub fn edit_resend_chat(&mut self, index: usize, text: String) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.pending.is_some() {
            return;
        }
        let Some(chat) = state.active_chat_mut() else {
            return;
        };
        crate::session::message_ops::truncate_chat_from(&mut chat.messages, index, text);
        if chat.context_summary_upto > chat.messages.len() {
            chat.context_summary = None;
            chat.context_summary_upto = 0;
        }
        state.editing_chat = None;
        state.pending_confirm = None;
        self.start_chat_stream();
    }

    pub fn delete_chat_pair(&mut self, index: usize) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.pending.is_some() {
            return;
        }
        let Some(chat) = state.active_chat_mut() else {
            return;
        };
        crate::session::message_ops::delete_chat_pair(&mut chat.messages, index);
        if chat.context_summary_upto > chat.messages.len() {
            chat.context_summary = None;
            chat.context_summary_upto = 0;
        }
        state.pending_confirm = None;
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
        if state.temporary_mode {
            return;
        }
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

    fn start_chat_stream(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.pending.is_some() {
            return;
        }
        let recent_keep = state.settings.context_recent_messages.max(1);
        let preview_model = state
            .active_chat()
            .map(|c| c.model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let context_length = state
            .catalog
            .find(&preview_model)
            .and_then(|m| m.context_length)
            .unwrap_or(crate::session::context_window::DEFAULT_CONTEXT_LENGTH);

        let Some(chat) = state.active_chat_mut() else {
            return;
        };
        if chat.messages.is_empty() {
            return;
        }
        let chat_id = chat.id;
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

        chat.messages.push(Message {
            role: Role::Assistant,
            content: String::new(),
            appeared_at: Some(Instant::now()),
            interrupted: false,
            images: Vec::new(),
        });

        if !state.temporary_mode {
            let _ = storage::save_chats(&state.chats);
        }

        let (tx, rx) = mpsc::channel::<StreamUiEvent>();
        let cancel = CancellationToken::new();
        state.pending = Some(PendingResponse {
            chat_id,
            rx,
            cancel: cancel.clone(),
        });

        let Some(provider) = state.provider.clone() else {
            return;
        };
        self.rt.spawn(async move {
            let request = ChatRequest {
                model,
                system,
                messages: history,
                tools: Vec::new(),
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
                Err(e) => {
                    let _ = tx.send(StreamUiEvent::Error(format!("{e}")));
                    return;
                }
            };
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        let _ = tx.send(StreamUiEvent::Cancelled);
                        return;
                    }
                    event = stream.next() => {
                        match event {
                            Some(Ok(ProviderEvent::TextDelta(text))) => {
                                if tx.send(StreamUiEvent::Delta(text)).is_err() {
                                    return;
                                }
                            }
                            Some(Ok(ProviderEvent::Usage(_) | ProviderEvent::ToolCallDelta { .. })) => {}
                            Some(Ok(ProviderEvent::Retrying { attempt, max_attempts, wait_secs })) => {
                                let _ = tx.send(StreamUiEvent::Retrying { attempt, max_attempts, wait_secs });
                            }
                            Some(Ok(ProviderEvent::Finished(_))) | None => {
                                let _ = tx.send(StreamUiEvent::Done);
                                return;
                            }
                            Some(Err(ProviderError::Cancelled)) => {
                                let _ = tx.send(StreamUiEvent::Cancelled);
                                return;
                            }
                            Some(Err(ProviderError::Unauthorized)) => {
                                let _ = tx.send(StreamUiEvent::Unauthorized);
                                return;
                            }
                            Some(Err(e)) => {
                                let _ = tx.send(StreamUiEvent::Error(format!("{e}")));
                                return;
                            }
                        }
                    }
                }
            }
        });
    }

    pub fn cancel_pending(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if let Some(pending) = &state.pending {
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

        let (tx, rx) = mpsc::channel();
        state.settings_ui.test_rx = Some(rx);
        state.settings_ui.test_status = KeyTestStatus::Testing;
        self.rt.spawn(async move {
            let result = match validate_openrouter_key(key.clone()).await {
                Ok(()) => ValidationResult::Ok(key),
                Err(ProviderError::Unauthorized) => {
                    ValidationResult::Err("That key was rejected by OpenRouter.".into())
                }
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
                if let Err(e) = self.apply_api_key(key) {
                    if let Screen::Main(state) = &mut self.screen {
                        state.settings_ui.test_status = KeyTestStatus::Err(format!("{e}"));
                    }
                    return;
                }
                let retry = if let Screen::Main(state) = &mut self.screen {
                    state.settings_ui.test_status = KeyTestStatus::Ok;
                    state.settings_ui.key_input.clear();
                    state.banner_key_input.clear();
                    let retry = state.retry_after_auth;
                    state.retry_after_auth = false;
                    retry
                } else {
                    false
                };
                if retry {
                    self.retry_after_auth_fix();
                }
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

    pub fn apply_api_key(&mut self, key: String) -> anyhow::Result<()> {
        SecureStore::save_key(&key)?;
        let timeout = match &self.screen {
            Screen::Main(state) => {
                std::time::Duration::from_secs(state.settings.request_timeout_secs)
            }
            _ => std::time::Duration::from_secs(storage::DEFAULT_REQUEST_TIMEOUT_SECS),
        };
        let provider =
            connect_openrouter_timed(key.clone(), timeout).map_err(|e| anyhow::anyhow!("{e}"))?;
        let Screen::Main(state) = &mut self.screen else {
            return Ok(());
        };
        state.provider = Some(provider);
        state.credential = CredentialStatus::from_key(Some(&key));
        Ok(())
    }

    pub fn remove_api_key(&mut self) {
        if let Err(e) = SecureStore::delete_key() {
            tracing::warn!("couldn't delete cached key: {e:#}");
        }
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.provider = None;
        state.credential = CredentialStatus::from_key(None);
        state.settings_ui.confirm_remove = false;
        state.settings_ui.test_status = KeyTestStatus::Idle;
        state.settings_ui.key_input.clear();
    }

    pub fn start_banner_retry(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if !state.banner_key_input.trim().is_empty() {
            state.settings_ui.key_input = state.banner_key_input.clone();
        }
        state.retry_after_auth = true;
        self.start_key_test();
    }

    pub fn retry_after_auth_fix(&mut self) {
        let mode = match &self.screen {
            Screen::Main(state) => state.mode,
            _ => return,
        };
        match mode {
            AppMode::Chat => self.resend_chat_after_auth(),
            AppMode::Coder => self.resume_coder_after_auth(),
        }
    }

    fn resend_chat_after_auth(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        if state.pending.is_some() {
            return;
        }
        let Some(chat) = state.active_chat_mut() else {
            return;
        };
        if chat
            .messages
            .last()
            .is_some_and(|m| matches!(m.role, Role::Assistant) && m.content == AUTH_REJECTED_NOTICE)
        {
            chat.messages.pop();
        }
        let chat_id = chat.id;
        let model = chat.model.clone();
        let system = chat.request_system();
        let history: Vec<ChatMessage> = chat
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
        if history.is_empty() {
            return;
        }
        chat.messages.push(Message {
            role: Role::Assistant,
            content: String::new(),
            appeared_at: Some(Instant::now()),
            interrupted: false,
            images: Vec::new(),
        });
        let Some(provider) = state.provider.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel::<StreamUiEvent>();
        let cancel = CancellationToken::new();
        state.pending = Some(PendingResponse {
            chat_id,
            rx,
            cancel: cancel.clone(),
        });
        self.rt.spawn(async move {
            let request = ChatRequest {
                model,
                system,
                messages: history,
                tools: Vec::new(),
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
                Err(e) => {
                    let _ = tx.send(StreamUiEvent::Error(format!("{e}")));
                    return;
                }
            };
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        let _ = tx.send(StreamUiEvent::Cancelled);
                        return;
                    }
                    event = stream.next() => {
                        match event {
                            Some(Ok(ProviderEvent::TextDelta(text))) => {
                                if tx.send(StreamUiEvent::Delta(text)).is_err() {
                                    return;
                                }
                            }
                            Some(Ok(ProviderEvent::Usage(_) | ProviderEvent::ToolCallDelta { .. })) => {}
                            Some(Ok(ProviderEvent::Retrying { attempt, max_attempts, wait_secs })) => {
                                let _ = tx.send(StreamUiEvent::Retrying { attempt, max_attempts, wait_secs });
                            }
                            Some(Ok(ProviderEvent::Finished(_))) | None => {
                                let _ = tx.send(StreamUiEvent::Done);
                                return;
                            }
                            Some(Err(ProviderError::Cancelled)) => {
                                let _ = tx.send(StreamUiEvent::Cancelled);
                                return;
                            }
                            Some(Err(ProviderError::Unauthorized)) => {
                                let _ = tx.send(StreamUiEvent::Unauthorized);
                                return;
                            }
                            Some(Err(e)) => {
                                let _ = tx.send(StreamUiEvent::Error(format!("{e}")));
                                return;
                            }
                        }
                    }
                }
            }
        });
    }

    pub fn rebuild_provider_timeout(&mut self, timeout_secs: u64) {
        let Screen::Main(state) = &self.screen else {
            return;
        };
        if state.provider.is_none() {
            return;
        }
        let Ok(Some(key)) = SecureStore::load_key() else {
            return;
        };
        match connect_openrouter_timed(key, std::time::Duration::from_secs(timeout_secs)) {
            Ok(provider) => {
                if let Screen::Main(state) = &mut self.screen {
                    state.provider = Some(provider);
                }
            }
            Err(e) => tracing::warn!("couldn't rebuild provider: {e}"),
        }
    }

    pub fn poll_pending(&mut self) {
        let Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(pending) = &state.pending else {
            return;
        };
        let chat_id = pending.chat_id;
        let mut events = Vec::new();
        while let Ok(event) = pending.rx.try_recv() {
            events.push(event);
        }
        if events.is_empty() {
            return;
        }

        let mut finished = false;
        let mut persist = false;
        let mut unauthorized = false;
        let mut retry_hint = None;
        for event in events {
            let target = if state.temporary_mode {
                state.temp_chat.as_mut().filter(|c| c.id == chat_id)
            } else {
                state.chats.iter_mut().find(|c| c.id == chat_id)
            };
            let Some(chat) = target else {
                continue;
            };
            let Some(last) = chat.messages.last_mut() else {
                continue;
            };
            if !matches!(last.role, Role::Assistant) {
                continue;
            }
            match event {
                StreamUiEvent::Delta(text) => last.content.push_str(&text),
                StreamUiEvent::Done => {
                    finished = true;
                    persist = true;
                }
                StreamUiEvent::Error(e) => {
                    if last.content.is_empty() {
                        last.content = format!("⚠ Error: {e}");
                    } else {
                        last.content.push_str(&format!("\n\n⚠ Error: {e}"));
                    }
                    finished = true;
                    persist = true;
                }
                StreamUiEvent::Unauthorized => {
                    last.content = AUTH_REJECTED_NOTICE.into();
                    unauthorized = true;
                    finished = true;
                    persist = true;
                }
                StreamUiEvent::Retrying {
                    attempt,
                    max_attempts,
                    wait_secs,
                } => {
                    retry_hint = Some(format!(
                        "Retrying in {wait_secs}s… ({attempt}/{max_attempts})"
                    ));
                }
                StreamUiEvent::Cancelled => {
                    last.interrupted = true;
                    if last.content.is_empty() {
                        last.content = "*(interrupted)*".into();
                    }
                    finished = true;
                    persist = true;
                }
            }
        }

        if unauthorized {
            state.credential.state = CredentialState::Rejected;
        }
        if let Some(hint) = retry_hint {
            state.retry_hint = Some(hint);
        }
        if finished {
            state.retry_hint = None;
            state.pending = None;
            state.focus_input_next_frame = true;
        }
        if persist && !state.temporary_mode {
            let _ = storage::save_chats(&state.chats);
        }
    }
}

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
        };
        let json = serde_json::to_string(&chat).unwrap();
        let loaded: Chat = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.system.as_deref(), Some("Be terse."));
    }
}
