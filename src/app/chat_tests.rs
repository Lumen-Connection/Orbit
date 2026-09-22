use super::*;
use crate::providers::{AiModel, FinishReason, ImageAttachment, ModelId};
use futures_util::stream::BoxStream;
use std::time::Duration;

struct ControlledRequest {
    request: ChatRequest,
    events: tokio::sync::mpsc::UnboundedSender<Result<ProviderEvent, ProviderError>>,
    cancel: CancellationToken,
}

struct ControlledProvider {
    requests: Sender<ControlledRequest>,
}

#[async_trait::async_trait]
impl AiProvider for ControlledProvider {
    fn id(&self) -> &'static str {
        OPENROUTER
    }
    async fn list_models(&self) -> Result<Vec<AiModel>, ProviderError> {
        Ok(Vec::new())
    }
    fn supports_tools(&self, _: &ModelId) -> bool {
        false
    }
    async fn stream_chat(
        &self,
        request: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'static, Result<ProviderEvent, ProviderError>>, ProviderError> {
        let (events, rx) = tokio::sync::mpsc::unbounded_channel();
        self.requests
            .send(ControlledRequest {
                request,
                events,
                cancel,
            })
            .unwrap();
        Ok(Box::pin(futures_util::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|event| (event, rx)) },
        )))
    }
}

struct Fixture {
    app: App,
    requests: Receiver<ControlledRequest>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (tx, requests) = mpsc::channel();
        let provider: Arc<dyn AiProvider> = Arc::new(ControlledProvider { requests: tx });
        let chat = Chat::new(DEFAULT_MODEL.into());
        let active_chat_id = Some(chat.id);
        let state = MainState {
            providers: ProviderHub::default(),
            provider: Some(provider),
            chats: vec![chat],
            active_chat_id,
            temp_chat: None,
            temporary_mode: false,
            chat_ui: HashMap::new(),
            pending: HashMap::new(),
            focus_input_next_frame: false,
            confirm_eject: false,
            catalog: ModelCatalog::curated(),
            catalog_rx: None,
            model_search: String::new(),
            mode: AppMode::Chat,
            coder: CoderState::default(),
            settings: AppSettings::default(),
            settings_ui: SettingsUi::default(),
            credential: CredentialStatus::from_key(None),
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
        };
        let app = App {
            screen: Screen::Main(Box::new(state)),
            rt: Arc::new(Runtime::new().unwrap()),
            db: Arc::new(Db::open_at(dir.path().join("orbit.db")).unwrap()),
            chat_history_path: Some(dir.path().join("chats.json")),
        };
        Self {
            app,
            requests,
            _dir: dir,
        }
    }
    fn state(&self) -> &MainState {
        let Screen::Main(state) = &self.app.screen else {
            panic!("expected main")
        };
        state
    }
    fn state_mut(&mut self) -> &mut MainState {
        let Screen::Main(state) = &mut self.app.screen else {
            panic!("expected main")
        };
        state
    }
    fn send(&mut self, text: &str) -> (Uuid, ControlledRequest) {
        let id = self.state().active_chat().unwrap().id;
        self.state_mut().chat_ui.entry(id).or_default().input = text.into();
        self.app.send_message();
        (
            id,
            self.requests.recv_timeout(Duration::from_secs(3)).unwrap(),
        )
    }
    fn poll_until(&mut self, ready: impl Fn(&MainState) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            self.app.poll_pending();
            if ready(self.state()) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "stream did not reach expected state"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn text(state: &MainState, id: Uuid) -> &str {
    &state.chat(id).unwrap().messages.last().unwrap().content
}

#[test]
fn app_renderer_polls_chat_streams_while_coder_is_visible() {
    let mut f = Fixture::new();
    let (id, request) = f.send("background request");
    f.state_mut().mode = AppMode::Coder;
    request
        .events
        .send(Ok(ProviderEvent::TextDelta("background reply".into())))
        .unwrap();
    request
        .events
        .send(Ok(ProviderEvent::Finished(FinishReason::Stop)))
        .unwrap();
    let ctx = eframe::egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(3);
    while f.state().pending.contains_key(&id) {
        let mut output = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            crate::ui::render(&mut f.app, ui)
        });
        // Headless rendering has no GPU backend to apply texture uploads.
        output.textures_delta.clear();
        assert!(
            Instant::now() < deadline,
            "renderer did not finish the hidden chat"
        );
        std::thread::yield_now();
    }
    assert_eq!(text(f.state(), id), "background reply");
}

#[test]
fn overlapping_streams_route_in_coder_mode_and_finish_independently() {
    let mut f = Fixture::new();
    let (a, ra) = f.send("question A");
    f.app.new_chat();
    let (b, rb) = f.send("question B");
    assert_eq!(f.state().pending.len(), 2);
    assert!(
        matches!(&ra.request.messages[0], ChatMessage::User { content, .. } if content == "question A")
    );
    assert!(
        matches!(&rb.request.messages[0], ChatMessage::User { content, .. } if content == "question B")
    );
    f.state_mut().mode = AppMode::Coder;
    f.state_mut().focus_input_next_frame = false;
    ra.events
        .send(Ok(ProviderEvent::TextDelta("A only".into())))
        .unwrap();
    rb.events
        .send(Ok(ProviderEvent::TextDelta("B only".into())))
        .unwrap();
    ra.events
        .send(Ok(ProviderEvent::Finished(FinishReason::Stop)))
        .unwrap();
    f.poll_until(|s| !s.pending.contains_key(&a) && text(s, b) == "B only");
    assert_eq!(text(f.state(), a), "A only");
    assert!(f.state().pending.contains_key(&b));
    assert!(!rb.cancel.is_cancelled());
    assert!(!f.state().focus_input_next_frame);
    rb.events
        .send(Ok(ProviderEvent::Finished(FinishReason::Stop)))
        .unwrap();
    f.poll_until(|s| s.pending.is_empty());
}

#[test]
fn stop_selected_chat_leaves_other_stream_running() {
    let mut f = Fixture::new();
    let (a, ra) = f.send("A");
    f.app.new_chat();
    let (b, rb) = f.send("B");
    f.app.select_chat(a);
    f.app.cancel_pending();
    f.poll_until(|s| !s.pending.contains_key(&a));
    assert!(ra.cancel.is_cancelled());
    assert!(!rb.cancel.is_cancelled());
    assert!(f.state().pending.contains_key(&b));
    assert!(
        f.state()
            .chat(a)
            .unwrap()
            .messages
            .last()
            .unwrap()
            .interrupted
    );
    rb.events
        .send(Ok(ProviderEvent::TextDelta("still B".into())))
        .unwrap();
    f.poll_until(|s| text(s, b) == "still B");
}

#[test]
fn saved_completion_persists_while_temporary_chat_is_visible() {
    let mut f = Fixture::new();
    let (a, ra) = f.send("saved");
    f.app.set_temporary_mode(true);
    let (temporary, rt) = f.send("private temporary");
    ra.events
        .send(Ok(ProviderEvent::TextDelta("saved response".into())))
        .unwrap();
    ra.events
        .send(Ok(ProviderEvent::Finished(FinishReason::Stop)))
        .unwrap();
    f.poll_until(|s| !s.pending.contains_key(&a));
    let saved: Vec<Chat> =
        serde_json::from_slice(&std::fs::read(f.app.chat_history_path.as_ref().unwrap()).unwrap())
            .unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, a);
    assert_eq!(saved[0].messages.last().unwrap().content, "saved response");
    assert!(f.state().pending.contains_key(&temporary));
    assert!(!rt.cancel.is_cancelled());
}

#[test]
fn deleting_and_replacing_temporary_chats_cancel_only_their_requests() {
    let mut f = Fixture::new();
    let (a, ra) = f.send("saved");
    f.app.set_temporary_mode(true);
    let (temporary, rt) = f.send("temporary");
    f.app.new_chat();
    assert!(rt.cancel.is_cancelled());
    assert!(!f.state().pending.contains_key(&temporary));
    assert!(!f.state().chat_ui.contains_key(&temporary));
    assert!(f.state().pending.contains_key(&a));
    assert!(!ra.cancel.is_cancelled());
    f.app.delete_chat(a);
    assert!(ra.cancel.is_cancelled());
    assert!(f.state().pending.is_empty());
    assert!(f.state().chat(a).is_none());
}

fn synthetic_pending(f: &mut Fixture, id: Uuid) -> (Sender<StreamUiEvent>, CancellationToken) {
    let (tx, rx) = mpsc::channel();
    let cancel = CancellationToken::new();
    let chat = f.state_mut().chat_mut(id).unwrap();
    chat.messages.push(Message {
        role: Role::Assistant,
        content: "partial".into(),
        appeared_at: None,
        interrupted: false,
        images: vec![],
        sources: vec![],
    });
    let assistant_index = chat.messages.len() - 1;
    f.state_mut().pending.insert(
        id,
        PendingResponse {
            assistant_index,
            rx,
            cancel: cancel.clone(),
        },
    );
    (tx, cancel)
}

#[test]
fn disconnected_and_missing_destinations_release_pending_state() {
    let mut f = Fixture::new();
    let a = f.state().active_chat().unwrap().id;
    let (tx, cancel) = synthetic_pending(&mut f, a);
    drop(tx);
    f.app.poll_pending();
    assert!(!f.state().pending.contains_key(&a));
    assert!(cancel.is_cancelled());
    assert!(text(f.state(), a).contains("partial"));
    let (tx, cancel) = synthetic_pending(&mut f, a);
    f.state_mut().chats.clear();
    tx.send(StreamUiEvent::Done).unwrap();
    f.app.poll_pending();
    assert!(f.state().pending.is_empty());
    assert!(cancel.is_cancelled());
}

#[test]
fn missing_provider_retains_draft_without_an_assistant_placeholder() {
    let mut f = Fixture::new();
    let a = f.state().active_chat().unwrap().id;
    f.state_mut().provider = None;
    f.state_mut().chat_ui.entry(a).or_default().input = "keep my draft".into();
    f.app.send_message();
    assert_eq!(f.state().chat_ui[&a].input, "keep my draft");
    assert!(f.state().chat(a).unwrap().messages.is_empty());
    assert!(f.state().pending.is_empty());
}

#[test]
fn drafts_images_and_editing_survive_chat_switches_independently() {
    let mut f = Fixture::new();
    let a = f.state().active_chat().unwrap().id;
    let image = ImageAttachment {
        mime: "image/png".into(),
        data: "test-image".into(),
        width: 1,
        height: 1,
    };
    let draft = f.state_mut().chat_ui.entry(a).or_default();
    draft.input = "draft A".into();
    draft.draft_images.push(image.clone());
    draft.editing = Some(MessageEdit {
        index: 0,
        draft: "edit A".into(),
    });
    f.app.new_chat();
    let b = f.state().active_chat().unwrap().id;
    f.state_mut().active_chat_ui_mut().unwrap().input = "draft B".into();
    assert!(f.state().chat_ui[&b].draft_images.is_empty());
    assert!(f.state().chat_ui[&b].editing.is_none());
    f.app.select_chat(a);
    assert_eq!(f.state().active_chat_ui().unwrap().input, "draft A");
    assert_eq!(f.state().chat_ui[&a].draft_images, vec![image]);
    assert_eq!(
        f.state().chat_ui[&a].editing.as_ref().unwrap().draft,
        "edit A"
    );
    assert_eq!(f.state().chat_ui[&b].input, "draft B");
    assert!(f.state().draft_images.is_empty());
}

#[test]
fn auth_retry_targets_original_chat_after_chat_and_mode_switch() {
    let mut f = Fixture::new();
    let (a, ra) = f.send("retry A");
    ra.events.send(Err(ProviderError::Unauthorized)).unwrap();
    f.poll_until(|s| !s.pending.contains_key(&a));
    let generation = f.state().chat_ui[&a].generation;
    f.state_mut().retry_after_auth = Some(AuthRetryTarget::Chat {
        chat_id: a,
        generation,
    });
    f.app.new_chat();
    let b = f.state().active_chat().unwrap().id;
    f.state_mut().mode = AppMode::Coder;
    f.app.retry_after_auth_fix();
    let retry = f.requests.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(
        matches!(&retry.request.messages[0], ChatMessage::User { content, .. } if content == "retry A")
    );
    assert!(f.state().pending.contains_key(&a));
    assert!(!f.state().pending.contains_key(&b));
    assert!(f.state().chat(b).unwrap().messages.is_empty());
    assert_eq!(f.state().active_chat_id, Some(b));
}

#[test]
fn stale_auth_retry_does_not_restart_a_newer_turn() {
    let mut f = Fixture::new();
    let (a, ra) = f.send("A");
    ra.events.send(Err(ProviderError::Unauthorized)).unwrap();
    f.poll_until(|s| !s.pending.contains_key(&a));
    let old_generation = f.state().chat_ui[&a].generation;
    f.state_mut().chat_ui.get_mut(&a).unwrap().generation += 1;
    f.state_mut().retry_after_auth = Some(AuthRetryTarget::Chat {
        chat_id: a,
        generation: old_generation,
    });
    f.app.retry_after_auth_fix();
    assert!(f.state().pending.is_empty());
    assert_eq!(text(f.state(), a), AUTH_REJECTED_NOTICE);
    assert!(f.requests.try_recv().is_err());
}

struct EstablishingProvider(Sender<CancellationToken>);

#[async_trait::async_trait]
impl AiProvider for EstablishingProvider {
    fn id(&self) -> &'static str {
        OPENROUTER
    }
    async fn list_models(&self) -> Result<Vec<AiModel>, ProviderError> {
        Ok(Vec::new())
    }
    fn supports_tools(&self, _: &ModelId) -> bool {
        false
    }
    async fn stream_chat(
        &self,
        _: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<BoxStream<'static, Result<ProviderEvent, ProviderError>>, ProviderError> {
        self.0.send(cancel).unwrap();
        std::future::pending().await
    }
}

#[test]
fn stop_releases_a_request_still_establishing_its_provider_stream() {
    let mut f = Fixture::new();
    let (started, established) = mpsc::channel();
    f.state_mut().provider = Some(Arc::new(EstablishingProvider(started)));
    let a = f.state().active_chat().unwrap().id;
    f.state_mut().chat_ui.entry(a).or_default().input = "connect".into();
    f.app.send_message();
    let cancel = established.recv_timeout(Duration::from_secs(3)).unwrap();
    f.app.cancel_pending();
    f.poll_until(|s| s.pending.is_empty());
    assert!(cancel.is_cancelled());
    assert!(
        f.state()
            .chat(a)
            .unwrap()
            .messages
            .last()
            .unwrap()
            .interrupted
    );
}

#[test]
fn leaving_main_screen_cancels_every_outstanding_chat_request() {
    let mut f = Fixture::new();
    let (_, ra) = f.send("A");
    f.app.new_chat();
    let (_, rb) = f.send("B");
    f.app.screen = Screen::Onboarding(OnboardingState {
        key_input: String::new(),
        show_key: false,
        status: OnboardingStatus::Idle,
        rx: None,
    });
    assert!(ra.cancel.is_cancelled());
    assert!(rb.cancel.is_cancelled());
}

#[test]
fn retry_hints_and_errors_stay_with_their_own_chat() {
    let mut f = Fixture::new();
    let (a, ra) = f.send("A");
    f.app.new_chat();
    let (b, rb) = f.send("B");
    ra.events
        .send(Ok(ProviderEvent::Retrying {
            attempt: 1,
            max_attempts: 3,
            wait_secs: 2,
        }))
        .unwrap();
    f.poll_until(|s| s.chat_ui[&a].retry_hint.is_some());
    assert!(f.state().chat_ui[&b].retry_hint.is_none());
    ra.events
        .send(Err(ProviderError::Message("controlled failure".into())))
        .unwrap();
    f.poll_until(|s| !s.pending.contains_key(&a));
    assert!(text(f.state(), a).contains("controlled failure"));
    assert!(f.state().chat_ui[&a].retry_hint.is_none());
    assert!(text(f.state(), b).is_empty());
    assert!(f.state().pending.contains_key(&b));
    assert!(!rb.cancel.is_cancelled());
}

#[test]
fn duplicate_send_in_a_busy_chat_preserves_the_next_draft() {
    let mut f = Fixture::new();
    let (a, _request) = f.send("first");
    f.state_mut().chat_ui.get_mut(&a).unwrap().input = "next draft".into();
    f.app.send_message();
    assert_eq!(f.state().pending.len(), 1);
    assert_eq!(f.state().chat(a).unwrap().messages.len(), 2);
    assert_eq!(f.state().chat_ui[&a].input, "next draft");
    assert!(f.requests.try_recv().is_err());
}
