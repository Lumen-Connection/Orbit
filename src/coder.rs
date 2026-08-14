use crate::app::{App, MainState};
use crate::context::{OrbitStore, SessionRecord, build_handoff};
use crate::providers::ChatMessage;
use crate::security::{ApprovalDecision, ApprovalId, CommandPolicy, Policy};
use crate::session::agent_loop::summarize;
use crate::session::agent_loop::{TurnDeps, run_turn};
use crate::session::{
    ActiveWork, DEFAULT_SESSION_BUDGET_USD, Session, SessionId, SessionManager, TranscriptItem,
    summarize_active_work,
};
use crate::storage::{Db, UsageReport};
use crate::tools::ToolRegistry;
use crate::tools::shell::TerminalEvent;
use crate::workspace::{
    FilePatch, FileTree, Project, RecentProject, ScanEntry, ScanEvent, apply_patch,
    load_recent_projects, remember_project, revalidate_patch, scan_project,
};
use eframe::egui;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

pub const TERMINAL_LINE_LIMIT: usize = 5_000;

pub struct TerminalState {
    pub lines: VecDeque<String>,
    partial: String,
    pub running: bool,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u128>,
    pub timed_out: bool,
    pub started_at: Option<Instant>,
    pub cancel: Option<CancellationToken>,
    pub tx: Sender<TerminalEvent>,
    pub rx: Receiver<TerminalEvent>,
    view_cache: String,
    dirty: bool,
}

impl Default for TerminalState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            lines: VecDeque::new(),
            partial: String::new(),
            running: false,
            command: None,
            exit_code: None,
            duration_ms: None,
            timed_out: false,
            started_at: None,
            cancel: None,
            tx,
            rx,
            view_cache: String::new(),
            dirty: false,
        }
    }
}

impl TerminalState {
    pub fn clear(&mut self) {
        self.lines.clear();
        self.partial.clear();
        self.running = false;
        self.command = None;
        self.exit_code = None;
        self.duration_ms = None;
        self.timed_out = false;
        self.started_at = None;
        self.cancel = None;
        self.dirty = true;
    }

    pub fn apply(&mut self, event: TerminalEvent) {
        match event {
            TerminalEvent::Started { command, cancel } => {
                self.command = Some(command);
                self.cancel = Some(cancel);
                self.running = true;
                self.exit_code = None;
                self.duration_ms = None;
                self.timed_out = false;
                self.started_at = Some(Instant::now());
                self.push_meta(format!("$ {}", self.command.as_deref().unwrap_or_default()));
            }
            TerminalEvent::Chunk(text) => self.push_chunk(&text),
            TerminalEvent::Finished {
                exit_code,
                duration_ms,
                timed_out,
                cancelled,
            } => {
                if !self.partial.is_empty() {
                    let leftover = std::mem::take(&mut self.partial);
                    self.push_line(leftover);
                }
                self.running = false;
                self.exit_code = exit_code;
                self.duration_ms = Some(duration_ms);
                self.timed_out = timed_out;
                self.cancel = None;
                let status = if timed_out {
                    "timed out".into()
                } else if cancelled {
                    "cancelled".into()
                } else {
                    format!("exit {}", exit_code.unwrap_or(-1))
                };
                self.push_meta(format!("[{status} · {:.2}s]", duration_ms as f64 / 1000.0));
            }
        }
    }

    pub fn view(&mut self) -> &str {
        if self.dirty {
            let mut out = String::new();
            for (i, line) in self.lines.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(line);
            }
            if !self.partial.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&self.partial);
            }
            self.view_cache = out;
            self.dirty = false;
        }
        &self.view_cache
    }

    fn push_chunk(&mut self, text: &str) {
        self.partial.push_str(text);
        while let Some(idx) = self.partial.find('\n') {
            let mut line: String = self.partial.drain(..=idx).collect();
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            self.push_line(line);
        }
        self.dirty = true;
    }

    fn push_line(&mut self, line: String) {
        self.lines.push_back(line);
        while self.lines.len() > TERMINAL_LINE_LIMIT {
            self.lines.pop_front();
        }
        self.dirty = true;
    }

    fn push_meta(&mut self, line: String) {
        self.push_line(line);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    #[default]
    Chat,
    Coder,
}

pub enum ViewerBody {
    Empty,
    Loading,
    Text {
        plain: String,
        highlighted: Option<egui::text::LayoutJob>,
    },
    Error(String),
}

pub struct ViewerState {
    pub relative: Option<PathBuf>,
    pub body: ViewerBody,
    pub rx: Option<Receiver<ViewerBody>>,
}

impl Default for ViewerState {
    fn default() -> Self {
        Self {
            relative: None,
            body: ViewerBody::Empty,
            rx: None,
        }
    }
}

pub struct CoderState {
    pub project: Option<Arc<Project>>,
    pub recent: Vec<RecentProject>,
    pub projects: Vec<crate::workspace::registry::ProjectEntry>,
    pub projects_loaded: bool,
    pub tree: FileTree,
    pub scan_rx: Option<Receiver<ScanEvent>>,
    pub scan_cancel: Option<CancellationToken>,
    pub scanning: bool,
    pub selected: Option<PathBuf>,
    pub viewer: ViewerState,
    pub pending_patches: Vec<FilePatch>,
    pub path_input: String,
    pub status: Option<String>,
    pub sessions: SessionManager,
    pub policy: Policy,
    pub terminal: TerminalState,
    pub store: Option<Arc<std::sync::Mutex<OrbitStore>>>,
    pub last_context_reload: Option<Instant>,
    pub expand_decisions: bool,
    pub expand_tasks: bool,
    pub expand_findings: bool,
    pub restore_rx: Option<Receiver<ProjectSnapshot>>,
    pub show_usage: bool,
    pub usage_report: Option<UsageReport>,
    pub usage_rx: Option<Receiver<UsageReport>>,
    pub switch_prompt: Option<SwitchPrompt>,
    pub run_configs: Vec<crate::workspace::run_config::RunConfig>,
    pub suggested_runs: Vec<crate::workspace::run_config::RunConfig>,
    pub run_editor: Option<crate::workspace::run_config::RunConfig>,
    pub run_pending_approval: Option<crate::workspace::run_config::RunConfig>,
    pub runner: std::sync::Arc<std::sync::Mutex<crate::runner::ProcessRegistry>>,
    pub run_restart_prompt: Option<crate::workspace::run_config::RunConfig>,
    pub coder_search: String,
    pub pipeline_tx: Option<std::sync::mpsc::Sender<crate::pipeline::PipelineEvent>>,
    pub pipeline_rx: Option<Receiver<crate::pipeline::PipelineEvent>>,
    pub pipeline: Option<crate::pipeline::Pipeline>,
    pub pipeline_dialog: Option<crate::pipeline::PipelineConfig>,
}

#[derive(Debug, Clone)]
pub enum SwitchTarget {
    Path(PathBuf),
    Close,
}

#[derive(Debug, Clone)]
pub struct SwitchPrompt {
    pub target: SwitchTarget,
    pub work: ActiveWork,
}

pub struct ProjectSnapshot {
    pub sessions: Vec<RestoredSession>,
    pub pending: Vec<FilePatch>,
}

pub struct RestoredSession {
    pub id: String,
    pub label: String,
    pub model: String,
    pub role: crate::session::AgentRole,
    pub messages: Vec<ChatMessage>,
    pub spent_usd: f64,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub context_summary: Option<String>,
    pub context_summary_upto: usize,
}

impl Default for CoderState {
    fn default() -> Self {
        let (pipeline_tx, pipeline_rx) = mpsc::channel();
        Self {
            project: None,
            recent: load_recent_projects(),
            projects: Vec::new(),
            projects_loaded: false,
            tree: FileTree::default(),
            scan_rx: None,
            scan_cancel: None,
            scanning: false,
            selected: None,
            viewer: ViewerState::default(),
            pending_patches: Vec::new(),
            path_input: String::new(),
            status: None,
            sessions: SessionManager::new(),
            policy: Policy::default(),
            terminal: TerminalState::default(),
            store: None,
            last_context_reload: None,
            expand_decisions: true,
            expand_tasks: true,
            expand_findings: false,
            restore_rx: None,
            show_usage: false,
            usage_report: None,
            usage_rx: None,
            switch_prompt: None,
            run_configs: Vec::new(),
            suggested_runs: Vec::new(),
            run_editor: None,
            run_pending_approval: None,
            runner: std::sync::Arc::new(std::sync::Mutex::new(
                crate::runner::ProcessRegistry::default(),
            )),
            run_restart_prompt: None,
            coder_search: String::new(),
            pipeline_tx: Some(pipeline_tx),
            pipeline_rx: Some(pipeline_rx),
            pipeline: None,
            pipeline_dialog: None,
        }
    }
}

impl App {
    pub fn set_mode(&mut self, mode: AppMode) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.mode = mode;
    }

    pub fn refresh_project_registry(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        match crate::workspace::registry::list_recent(&self.db, 20) {
            Ok(projects) => {
                state.coder.projects = projects;
                state.coder.projects_loaded = true;
            }
            Err(e) => {
                state.coder.projects_loaded = true;
                tracing::warn!("could not list projects: {e:#}");
            }
        }
    }

    pub fn forget_project(&mut self, id: &str) {
        if let Err(e) = crate::workspace::registry::forget(&self.db, id) {
            tracing::warn!("could not forget project: {e:#}");
        }
        self.refresh_project_registry();
    }

    pub fn rebind_project(&mut self, id: &str, path: PathBuf) {
        match crate::workspace::registry::rebind(&self.db, id, &path) {
            Ok(project) => self.open_project_path(project.canonical_root),
            Err(e) => {
                if let crate::app::Screen::Main(state) = &mut self.screen {
                    state.coder.status = Some(e.to_string());
                }
            }
        }
    }

    pub fn open_project_path(&mut self, path: PathBuf) {
        self.request_open_project(path);
    }

    pub fn request_open_project(&mut self, path: PathBuf) {
        self.request_switch(SwitchTarget::Path(path));
    }

    pub fn request_close_project(&mut self) {
        self.request_switch(SwitchTarget::Close);
    }

    fn request_switch(&mut self, target: SwitchTarget) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        if let SwitchTarget::Path(path) = &target
            && let Some(current) = &state.coder.project
        {
            let same = current.canonical_root == *path
                || path
                    .canonicalize()
                    .ok()
                    .is_some_and(|canon| canon == current.canonical_root);
            if same {
                return;
            }
        }
        if state.coder.project.is_none() {
            if let SwitchTarget::Path(path) = target {
                self.open_project_now(path);
            }
            return;
        }
        let running = state
            .coder
            .runner
            .lock()
            .map(|r| r.running_count())
            .unwrap_or(0);
        let work =
            summarize_active_work(&state.coder.sessions, &state.coder.pending_patches, running);
        if work.is_empty() {
            self.commit_switch(target);
            return;
        }
        state.coder.switch_prompt = Some(SwitchPrompt { target, work });
    }

    pub fn confirm_switch(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(prompt) = state.coder.switch_prompt.take() else {
            return;
        };
        self.commit_switch(prompt.target);
    }

    pub fn cancel_switch(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.coder.switch_prompt = None;
    }

    fn commit_switch(&mut self, target: SwitchTarget) {
        self.abandon_and_unload();
        if let SwitchTarget::Path(path) = target {
            self.open_project_now(path);
        }
    }

    fn abandon_and_unload(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        flush_coder_state(state, &self.db);
        for live in &mut state.coder.sessions.sessions {
            live.cancel();
        }
        if let Some(cancel) = state.coder.scan_cancel.take() {
            cancel.cancel();
        }
        if let Some(cancel) = state.coder.terminal.cancel.take() {
            cancel.cancel();
        }
        state.coder.sessions.shutdown();
        state.coder.project = None;
        state.coder.store = None;
        state.coder.pending_patches.clear();
        state.coder.tree = FileTree::default();
        state.coder.viewer = ViewerState::default();
        state.coder.selected = None;
        state.coder.scan_rx = None;
        state.coder.scanning = false;
        state.coder.restore_rx = None;
        state.coder.terminal.clear();
        state.coder.status = None;
        state.coder.switch_prompt = None;
        state.coder.run_configs.clear();
        state.coder.suggested_runs.clear();
        state.coder.run_editor = None;
        state.coder.run_pending_approval = None;
        if let Ok(mut runner) = state.coder.runner.lock() {
            runner.kill_all();
        }
        state.coder.run_restart_prompt = None;
    }

    fn open_project_now(&mut self, path: PathBuf) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        match Project::open(&path) {
            Ok(project) => start_scan(state, project, self.rt.clone(), self.db.clone()),
            Err(e) => state.coder.status = Some(e.to_string()),
        }
    }

    pub fn adopt_suggested_run(&mut self, id: &str) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(idx) = state.coder.suggested_runs.iter().position(|c| c.id == id) else {
            return;
        };
        let config = state.coder.suggested_runs.remove(idx);
        state.coder.run_configs.push(config);
        self.persist_run_configs();
    }

    pub fn persist_run_configs(&mut self) {
        let crate::app::Screen::Main(state) = &self.screen else {
            return;
        };
        let Some(root) = state
            .coder
            .project
            .as_ref()
            .map(|p| p.canonical_root.clone())
        else {
            return;
        };
        if let Err(e) = crate::workspace::run_config::save_saved(&root, &state.coder.run_configs) {
            tracing::warn!("could not save run configs: {e:#}");
        }
        if let crate::app::Screen::Main(state) = &mut self.screen {
            state.coder.suggested_runs = crate::workspace::run_config::suggestions_not_saved(&root);
        }
    }

    pub fn upsert_run_config(&mut self, config: crate::workspace::run_config::RunConfig) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        if let Some(existing) = state
            .coder
            .run_configs
            .iter_mut()
            .find(|c| c.id == config.id)
        {
            *existing = config;
        } else {
            state.coder.run_configs.push(config);
        }
        state.coder.run_editor = None;
        self.persist_run_configs();
    }

    pub fn request_run(&mut self, id: &str) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(config) = state
            .coder
            .run_configs
            .iter()
            .chain(state.coder.suggested_runs.iter())
            .find(|c| c.id == id)
            .cloned()
        else {
            return;
        };
        let policy = state
            .coder
            .policy
            .commands
            .lock()
            .ok()
            .map(|p| p.clone())
            .unwrap_or_default();
        match crate::workspace::run_config::gate(&config, &policy) {
            crate::workspace::run_config::RunGate::Denied => {
                state.coder.status = Some(format!(
                    "Blocked by the command denylist: `{}`",
                    config.display()
                ));
            }
            crate::workspace::run_config::RunGate::NeedsApproval => {
                state.coder.run_pending_approval = Some(config);
            }
            crate::workspace::run_config::RunGate::Approved => {
                self.start_approved_run(config);
            }
        }
    }

    pub fn confirm_run_approval(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(config) = state.coder.run_pending_approval.take() else {
            return;
        };
        if let Err(e) = crate::workspace::run_config::approve_on_this_machine(&config) {
            state.coder.status = Some(format!("Could not store run approval: {e}"));
            return;
        }
        self.start_approved_run(config);
    }

    pub fn decline_run_approval(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.coder.run_pending_approval = None;
    }

    pub fn poll_runner(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        if let Ok(mut runner) = state.coder.runner.lock() {
            runner.poll(Some(&self.rt));
        }
    }

    pub fn stop_run(&mut self, id: &str) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        if let Ok(mut runner) = state.coder.runner.lock() {
            runner.stop(id);
        }
    }

    pub fn restart_run(&mut self, config: crate::workspace::run_config::RunConfig) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(project) = state.coder.project.clone() else {
            return;
        };
        if let Ok(mut runner) = state.coder.runner.lock() {
            runner.request_restart(config, project.canonical_root.clone(), Some(&self.rt));
        }
        state.coder.run_restart_prompt = None;
    }

    pub fn confirm_run_restart(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(config) = state.coder.run_restart_prompt.take() else {
            return;
        };
        self.restart_run(config);
    }

    pub fn decline_run_restart(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.coder.run_restart_prompt = None;
    }

    pub fn kill_all_runs(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        if let Ok(mut runner) = state.coder.runner.lock() {
            runner.kill_all();
        }
    }

    fn start_approved_run(&mut self, config: crate::workspace::run_config::RunConfig) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        match config.kind {
            crate::workspace::run_config::RunKind::LongRunning => {
                let already = state
                    .coder
                    .runner
                    .lock()
                    .map(|r| r.is_running(&config.id))
                    .unwrap_or(false);
                if already {
                    state.coder.run_restart_prompt = Some(config);
                    return;
                }
                let Some(project) = state.coder.project.clone() else {
                    return;
                };
                let started = match state.coder.runner.lock() {
                    Ok(mut runner) => runner
                        .start(config, project.canonical_root.clone(), Some(&self.rt))
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                if let Err(e) = started {
                    state.coder.status = Some(e);
                }
            }
            crate::workspace::run_config::RunKind::OneShot => {
                self.spawn_oneshot_run(config);
            }
        }
    }

    fn spawn_oneshot_run(&mut self, config: crate::workspace::run_config::RunConfig) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(project) = state.coder.project.clone() else {
            return;
        };
        if state.coder.terminal.running {
            state.coder.status = Some("A command is already running in the terminal.".into());
            return;
        }
        let cancel = CancellationToken::new();
        state
            .coder
            .terminal
            .apply(crate::tools::shell::TerminalEvent::Started {
                command: config.display(),
                cancel: cancel.clone(),
            });
        let tx = state.coder.terminal.tx.clone();
        self.rt.spawn(async move {
            let mut cmd = tokio::process::Command::new(&config.program);
            cmd.args(&config.args)
                .current_dir(&project.canonical_root)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .stdin(std::process::Stdio::null());
            for (k, v) in &config.env {
                cmd.env(k, v);
            }
            let started = std::time::Instant::now();
            match cmd.spawn() {
                Err(e) => {
                    let _ = tx.send(crate::tools::shell::TerminalEvent::Chunk(format!("{e}\n")));
                    let _ = tx.send(crate::tools::shell::TerminalEvent::Finished {
                        exit_code: None,
                        duration_ms: started.elapsed().as_millis(),
                        timed_out: false,
                        cancelled: false,
                    });
                }
                Ok(mut child) => {
                    if let Some(mut out) = child.stdout.take() {
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            let mut buf = [0u8; 2048];
                            loop {
                                match tokio::io::AsyncReadExt::read(&mut out, &mut buf).await {
                                    Ok(0) | Err(_) => break,
                                    Ok(n) => {
                                        let _ = tx.send(crate::tools::shell::TerminalEvent::Chunk(
                                            String::from_utf8_lossy(&buf[..n]).into_owned(),
                                        ));
                                    }
                                }
                            }
                        });
                    }
                    if let Some(mut err) = child.stderr.take() {
                        let tx = tx.clone();
                        tokio::spawn(async move {
                            let mut buf = [0u8; 2048];
                            loop {
                                match tokio::io::AsyncReadExt::read(&mut err, &mut buf).await {
                                    Ok(0) | Err(_) => break,
                                    Ok(n) => {
                                        let _ = tx.send(crate::tools::shell::TerminalEvent::Chunk(
                                            String::from_utf8_lossy(&buf[..n]).into_owned(),
                                        ));
                                    }
                                }
                            }
                        });
                    }
                    let outcome = tokio::select! {
                        status = child.wait() => {
                            (status.ok().and_then(|s| s.code()), false, false)
                        }
                        _ = tokio::time::sleep(crate::tools::shell::COMMAND_TIMEOUT) => {
                            let _ = child.start_kill();
                            let _ = child.wait().await;
                            (None, true, false)
                        }
                        _ = cancel.cancelled() => {
                            let _ = child.start_kill();
                            let _ = child.wait().await;
                            (None, false, true)
                        }
                    };
                    let _ = tx.send(crate::tools::shell::TerminalEvent::Finished {
                        exit_code: outcome.0,
                        duration_ms: started.elapsed().as_millis(),
                        timed_out: outcome.1,
                        cancelled: outcome.2,
                    });
                }
            }
        });
    }

    pub fn browse_for_project(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.request_open_project(path);
        }
    }

    pub fn poll_scan(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(rx) = &state.coder.scan_rx else {
            return;
        };
        let mut batches = Vec::new();
        let mut done = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                ScanEvent::Batch(entries) => batches.push(entries),
                ScanEvent::Done => done = true,
                ScanEvent::Failed(err) => {
                    state.coder.status = Some(err);
                    done = true;
                }
            }
        }
        for entries in batches {
            ingest_entries(&mut state.coder.tree, &entries);
        }
        if done {
            state.coder.scan_rx = None;
            state.coder.scanning = false;
        }
    }

    pub fn select_file(&mut self, relative: PathBuf) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(project) = state.coder.project.clone() else {
            return;
        };
        state.coder.selected = Some(relative.clone());
        state.coder.viewer.relative = Some(relative.clone());
        state.coder.viewer.body = ViewerBody::Loading;
        let (tx, rx) = mpsc::channel();
        state.coder.viewer.rx = Some(rx);
        self.rt.spawn_blocking(move || {
            let body = load_and_highlight(&project, &relative);
            let _ = tx.send(body);
        });
    }

    pub fn poll_viewer(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(rx) = &state.coder.viewer.rx else {
            return;
        };
        let Ok(body) = rx.try_recv() else {
            return;
        };
        state.coder.viewer.body = body;
        state.coder.viewer.rx = None;
    }

    #[allow(dead_code)]
    pub fn preview_patch(&mut self, relative: PathBuf, proposed: String) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let ViewerBody::Text { plain, .. } = &state.coder.viewer.body else {
            return;
        };
        state
            .coder
            .pending_patches
            .push(FilePatch::new(relative, plain.clone(), proposed));
    }

    pub fn apply_selected_patch(&mut self, index: usize) {
        let reload = {
            let crate::app::Screen::Main(state) = &mut self.screen else {
                return;
            };
            let Some(project) = state.coder.project.clone() else {
                return;
            };
            let Some(patch) = state.coder.pending_patches.get_mut(index) else {
                return;
            };
            match apply_patch(&project.canonical_root, patch) {
                Ok(()) => {
                    state.coder.status = Some(format!(
                        "{}: {:?}",
                        patch.relative_path.display(),
                        patch.status
                    ));
                    state.coder.selected.clone()
                }
                Err(e) => {
                    state.coder.status = Some(e.to_string());
                    None
                }
            }
        };
        if let Some(rel) = reload {
            self.select_file(rel);
        }
    }

    pub fn resume_coder_after_auth(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        if crate::app::can_create_session(state.credential.state).is_err() {
            return;
        }
        let Some(project) = state.coder.project.clone() else {
            return;
        };
        let slots = state.coder.sessions.slots.clone();
        let Some(live) = state.coder.sessions.active_mut() else {
            return;
        };
        if live.busy {
            return;
        }
        if live
            .transcript
            .last()
            .is_some_and(|item| matches!(item, TranscriptItem::Assistant(text) if text == crate::app::AUTH_REJECTED_NOTICE))
        {
            live.transcript.pop();
        }
        live.busy = true;
        let (tx, rx) = mpsc::channel();
        let cancel = CancellationToken::new();
        live.agent_rx = Some(rx);
        live.agent_cancel = Some(cancel.clone());
        let session = live.handle.clone();
        let session_id = live.id.clone();
        let session_label = live.label.clone();
        let session_model = live.model.clone();
        let approvals = live.approvals.clone();
        let budget_bridge = live.budget_bridge.clone();
        let budget_usd = live.budget_usd;
        let spent_start = live.spent_usd;
        let pipeline_tx = state.coder.pipeline_tx.clone();
        let prices = state
            .catalog
            .find(&session_model)
            .map(|m| (m.prompt_price, m.completion_price))
            .unwrap_or((None, None));
        let Some(provider) = state.provider.clone() else {
            live.busy = false;
            return;
        };
        let session_role = live.role;
        let deps = TurnDeps {
            provider,
            registry: Arc::new(ToolRegistry::for_role(session_role)),
            project,
            events: tx,
            approvals,
            policy: state.coder.policy.clone(),
            cancel,
            session_id: session_id.clone(),
            terminal: state.coder.terminal.tx.clone(),
            store: state.coder.store.clone(),
            session_label,
            session_model: session_model.clone(),
            session_role,
            summary_model: state
                .coder
                .store
                .as_ref()
                .and_then(|s| s.lock().ok())
                .and_then(|store| store.settings.summary_model.clone()),
            db: Some(self.db.clone()),
            prompt_price: prices.0,
            completion_price: prices.1,
            budget_usd: Some(budget_usd),
            budget_bridge,
            spent_start,
            context_length: state
                .catalog
                .find(&session_model)
                .and_then(|m| m.context_length)
                .unwrap_or(crate::session::context_window::DEFAULT_CONTEXT_LENGTH),
            recent_keep: state.settings.context_recent_messages,
            run_env: crate::session::agent_loop::RunEnv {
                runner: Some(state.coder.runner.clone()),
                configs: state
                    .coder
                    .run_configs
                    .iter()
                    .chain(state.coder.suggested_runs.iter())
                    .cloned()
                    .collect(),
                starts: std::sync::Arc::new(
                    std::sync::Mutex::new(std::collections::HashMap::new()),
                ),
            },
            user_images: Vec::new(),
        };
        self.rt.spawn(async move {
            let cancel = deps.cancel.clone();
            let _permit = tokio::select! {
                _ = cancel.cancelled() => None,
                permit = slots.acquire_owned() => permit.ok(),
            };
            {
                let mut session = session.lock().await;
                session.model = session_model;
            }
            let result = run_turn(session, None, deps).await;
            if let Some(tx) = pipeline_tx {
                let _ = tx.send(crate::pipeline::PipelineEvent::stage_finished(
                    session_id, result,
                ));
            }
        });
    }

    pub fn send_coder_prompt(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        if crate::app::can_create_session(state.credential.state).is_err() {
            return;
        }
        let Some(project) = state.coder.project.clone() else {
            return;
        };
        if state.coder.sessions.sessions.is_empty() {
            reset_agent_session(state, &project.name, true);
        }
        let Some(live) = state.coder.sessions.active_mut() else {
            return;
        };
        if live.busy {
            return;
        }
        let text = live.input.trim().to_string();
        if text.is_empty() && state.draft_images.is_empty() {
            return;
        }
        if !state.draft_images.is_empty()
            && !state
                .catalog
                .find(&live.model)
                .is_some_and(|m| m.supports_vision)
        {
            state.coder.status = Some(
                "This model is text-only. Switch to a vision model or remove the image.".into(),
            );
            return;
        }

        live.transcript.push(TranscriptItem::User(text.clone()));
        live.input.clear();
        self.launch_coder_turn(Some(text));
    }

    pub fn regenerate_coder(&mut self) {
        let persist = {
            let crate::app::Screen::Main(state) = &mut self.screen else {
                return;
            };
            let Some(live) = state.coder.sessions.active_mut() else {
                return;
            };
            if live.busy {
                return;
            }
            crate::session::message_ops::discard_last_coder_turn(&mut live.transcript);
            if let Ok(mut session) = live.handle.try_lock() {
                crate::session::message_ops::discard_after_user_message(&mut session.messages);
                if session.context_summary_upto > session.messages.len() {
                    session.context_summary = None;
                    session.context_summary_upto = 0;
                }
                Some((live.id.clone(), session.messages.clone()))
            } else {
                None
            }
        };
        if let Some((id, messages)) = persist {
            self.persist_coder_messages(&id, messages);
        }
        self.launch_coder_turn(None);
    }

    pub fn edit_resend_coder(&mut self, index: usize, text: String) {
        let persist = {
            let crate::app::Screen::Main(state) = &mut self.screen else {
                return;
            };
            let Some(live) = state.coder.sessions.active_mut() else {
                return;
            };
            if live.busy {
                return;
            }
            let Some(ordinal) =
                crate::session::message_ops::user_ordinal_in_transcript(&live.transcript, index)
            else {
                return;
            };
            crate::session::message_ops::truncate_coder_from_user(
                &mut live.transcript,
                index,
                text.clone(),
            );
            let messages = if let Ok(mut session) = live.handle.try_lock() {
                crate::session::message_ops::truncate_coder_messages_from(
                    &mut session.messages,
                    ordinal,
                    text,
                );
                if session.context_summary_upto > session.messages.len() {
                    session.context_summary = None;
                    session.context_summary_upto = 0;
                }
                Some((live.id.clone(), session.messages.clone()))
            } else {
                None
            };
            state.editing_coder = None;
            state.pending_confirm = None;
            messages
        };
        if let Some((id, messages)) = persist {
            self.persist_coder_messages(&id, messages);
        }
        self.launch_coder_turn(None);
    }

    pub fn delete_coder_turn(&mut self, index: usize) {
        let persist = {
            let crate::app::Screen::Main(state) = &mut self.screen else {
                return;
            };
            let Some(live) = state.coder.sessions.active_mut() else {
                return;
            };
            if live.busy {
                return;
            }
            let Some(ordinal) =
                crate::session::message_ops::turn_user_ordinal(&live.transcript, index)
            else {
                return;
            };
            crate::session::message_ops::delete_coder_turn(&mut live.transcript, index);
            let messages = if let Ok(mut session) = live.handle.try_lock() {
                crate::session::message_ops::delete_coder_messages_turn(
                    &mut session.messages,
                    ordinal,
                );
                if session.context_summary_upto > session.messages.len() {
                    session.context_summary = None;
                    session.context_summary_upto = 0;
                }
                Some((live.id.clone(), session.messages.clone()))
            } else {
                None
            };
            state.pending_confirm = None;
            messages
        };
        if let Some((id, messages)) = persist {
            self.persist_coder_messages(&id, messages);
        }
    }

    pub fn cycle_coder_session(&mut self, delta: i32) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let n = state.coder.sessions.sessions.len();
        if n == 0 {
            return;
        }
        let next = (state.coder.sessions.active as i32 + delta).rem_euclid(n as i32) as usize;
        state.coder.sessions.active = next;
    }

    pub fn export_active_coder(&self) -> Option<String> {
        let crate::app::Screen::Main(state) = &self.screen else {
            return None;
        };
        let live = state.coder.sessions.active()?;
        let project = state.coder.project.as_ref().map(|p| p.name.as_str());
        Some(crate::session::export::transcript_to_markdown(
            &crate::session::export::ExportMeta {
                title: &live.label,
                project,
                model: &live.model,
                date: chrono::Utc::now(),
                cost_usd: Some(live.spent_usd),
            },
            &live.transcript,
            None,
        ))
    }

    fn persist_coder_messages(&self, id: &SessionId, messages: Vec<ChatMessage>) {
        let db = self.db.clone();
        let id = id.clone();
        self.rt.spawn_blocking(move || {
            if let Err(e) = db.replace_messages(&id, &messages) {
                tracing::warn!("could not persist truncated session: {e:#}");
            }
        });
    }

    fn launch_coder_turn(&mut self, user_input: Option<String>) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(project) = state.coder.project.clone() else {
            return;
        };
        let slots = state.coder.sessions.slots.clone();
        let Some(live) = state.coder.sessions.active_mut() else {
            return;
        };
        if live.busy && user_input.is_some() {
            return;
        }
        live.busy = true;
        live.handoff_dismissed = true;
        state.coder.status = None;

        let (tx, rx) = mpsc::channel();
        let cancel = CancellationToken::new();
        live.agent_rx = Some(rx);
        live.agent_cancel = Some(cancel.clone());
        let session = live.handle.clone();
        let session_id = live.id.clone();
        let session_label = live.label.clone();
        let session_model = live.model.clone();
        let approvals = live.approvals.clone();
        let budget_bridge = live.budget_bridge.clone();
        let budget_usd = live.budget_usd;
        let pipeline_tx = state.coder.pipeline_tx.clone();
        let prices = state
            .catalog
            .find(&session_model)
            .map(|m| (m.prompt_price, m.completion_price))
            .unwrap_or((None, None));

        let Some(provider) = state.provider.clone() else {
            live.busy = false;
            live.transcript.push(TranscriptItem::Assistant(
                "⚠ Configure an API key in Settings before sending.".into(),
            ));
            return;
        };
        let session_role = live.role;
        let deps = TurnDeps {
            provider,
            registry: Arc::new(ToolRegistry::for_role(session_role)),
            project,
            events: tx,
            approvals,
            policy: state.coder.policy.clone(),
            cancel,
            session_id: session_id.clone(),
            terminal: state.coder.terminal.tx.clone(),
            store: state.coder.store.clone(),
            session_label,
            session_model: session_model.clone(),
            session_role,
            summary_model: state
                .coder
                .store
                .as_ref()
                .and_then(|s| s.lock().ok())
                .and_then(|store| store.settings.summary_model.clone()),
            db: Some(self.db.clone()),
            prompt_price: prices.0,
            completion_price: prices.1,
            budget_usd: Some(budget_usd),
            budget_bridge,
            spent_start: live.spent_usd,
            context_length: state
                .catalog
                .find(&session_model)
                .and_then(|m| m.context_length)
                .unwrap_or(crate::session::context_window::DEFAULT_CONTEXT_LENGTH),
            recent_keep: state.settings.context_recent_messages,
            run_env: crate::session::agent_loop::RunEnv {
                runner: Some(state.coder.runner.clone()),
                configs: state
                    .coder
                    .run_configs
                    .iter()
                    .chain(state.coder.suggested_runs.iter())
                    .cloned()
                    .collect(),
                starts: std::sync::Arc::new(
                    std::sync::Mutex::new(std::collections::HashMap::new()),
                ),
            },
            user_images: std::mem::take(&mut state.draft_images),
        };
        self.rt.spawn(async move {
            let cancel = deps.cancel.clone();
            let _permit = tokio::select! {
                _ = cancel.cancelled() => None,
                permit = slots.acquire_owned() => permit.ok(),
            };
            {
                let mut session = session.lock().await;
                session.model = session_model;
            }
            let result = run_turn(session, user_input, deps).await;
            if let Some(tx) = pipeline_tx {
                let _ = tx.send(crate::pipeline::PipelineEvent::stage_finished(
                    session_id, result,
                ));
            }
        });
    }

    pub fn poll_agent(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let poll = state.coder.sessions.poll_all_detailed();
        if poll.unauthorized {
            state.credential.state = crate::app::CredentialState::Rejected;
        }
    }

    pub fn cancel_coder_turn(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        if let Some(live) = state.coder.sessions.active_mut() {
            live.cancel();
        }
    }

    pub fn resolve_coder_approval(&mut self, id: ApprovalId, decision: ApprovalDecision) {
        let db = self.db.clone();
        let rt = self.rt.clone();
        let reload = {
            let crate::app::Screen::Main(state) = &mut self.screen else {
                return;
            };
            match state.coder.sessions.resolve_approval(id, decision) {
                Some((false, Some(mut patch), sid)) if decision == ApprovalDecision::Approved => {
                    if let Some(project) = state.coder.project.clone() {
                        let _ = apply_patch(&project.canonical_root, &mut patch);
                        persist_patch_db(&db, &rt, &project.id, &sid, &patch);
                    }
                    state.coder.selected.clone()
                }
                Some((true, _, _)) if decision == ApprovalDecision::Approved => {
                    state.coder.selected.clone()
                }
                Some(_) => None,
                None => None,
            }
        };
        if let Some(rel) = reload {
            self.select_file(rel);
        }
    }

    pub fn new_coder_session(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        if crate::app::can_create_session(state.credential.state).is_err() {
            return;
        }
        let Some(project) = state.coder.project.clone() else {
            return;
        };
        let model = state
            .coder
            .sessions
            .active()
            .map(|s| s.model.clone())
            .unwrap_or_else(|| state.settings.coder_default_model.clone());
        let label = state.coder.sessions.next_label(&project.name);
        let id = state.coder.sessions.create(label.clone(), model.clone());
        apply_session_limits(state, &id);
        persist_live_session(state, &id, &label, &model);
        persist_session_db(
            &self.db,
            &self.rt,
            &project,
            &id,
            &label,
            &model,
            crate::session::AgentRole::Coder.id(),
        );
        refresh_active_handoff(state);
    }

    pub fn open_pipeline_dialog(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let default = state.settings.coder_default_model.clone();
        state.coder.pipeline_dialog = Some(crate::pipeline::PipelineConfig {
            feature: String::new(),
            complexity: crate::pipeline::Complexity::Normal,
            planner: crate::pipeline::StageModel {
                auto: true,
                model: crate::providers::catalog::auto_model_for(
                    crate::pipeline::contract::StageKind::Planner,
                )
                .to_string(),
            },
            coder: crate::pipeline::StageModel {
                auto: true,
                model: crate::providers::catalog::auto_model_for(
                    crate::pipeline::contract::StageKind::Coder,
                )
                .to_string(),
            },
            reviewer: crate::pipeline::StageModel {
                auto: true,
                model: crate::providers::catalog::auto_model_for(
                    crate::pipeline::contract::StageKind::Reviewer,
                )
                .to_string(),
            },
            git_gate: crate::pipeline::GitGateMode::Manual,
            auto_planner_to_coder: true,
            auto_coder_to_reviewer: true,
        });
        let _ = default;
    }

    pub fn confirm_pipeline_dialog(&mut self) {
        let config = {
            let crate::app::Screen::Main(state) = &mut self.screen else {
                return;
            };
            state.coder.pipeline_dialog.take()
        };
        let Some(mut config) = config else {
            return;
        };
        if config.planner.auto {
            config.planner.model = crate::providers::catalog::auto_model_for(
                crate::pipeline::contract::StageKind::Planner,
            )
            .into();
        }
        if config.coder.auto {
            config.coder.model = crate::providers::catalog::auto_model_for(
                crate::pipeline::contract::StageKind::Coder,
            )
            .into();
        }
        if config.reviewer.auto {
            config.reviewer.model = crate::providers::catalog::auto_model_for(
                crate::pipeline::contract::StageKind::Reviewer,
            )
            .into();
        }
        self.instantiate_pipeline(config);
    }

    fn instantiate_pipeline(&mut self, config: crate::pipeline::PipelineConfig) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        if crate::app::can_create_session(state.credential.state).is_err() {
            return;
        }
        let Some(project) = state.coder.project.clone() else {
            return;
        };
        let mut pipeline = crate::pipeline::Pipeline::new(config.clone());
        for stage in config.intelligence_stages() {
            let role = match stage {
                crate::pipeline::contract::StageKind::Planner => {
                    crate::session::AgentRole::Architect
                }
                crate::pipeline::contract::StageKind::Reviewer => {
                    crate::session::AgentRole::Reviewer
                }
                _ => crate::session::AgentRole::Coder,
            };
            let model = config.model_for(stage).to_string();
            let label = format!(
                "{} · {}",
                stage.label(),
                crate::ui::truncate(&config.feature, 24)
            );
            let id = state
                .coder
                .sessions
                .create_with_role(label.clone(), model.clone(), role);
            apply_session_limits(state, &id);
            persist_live_session(state, &id, &label, &model);
            persist_session_db(&self.db, &self.rt, &project, &id, &label, &model, role.id());
            pipeline.bind_session(stage, id);
        }
        pipeline.note(
            pipeline.current,
            "Pipeline created. Sessions are idle until Start or the first prompt.",
        );
        state.coder.pipeline = Some(pipeline);
        refresh_active_handoff(state);
    }

    pub fn start_pipeline(&mut self) {
        let action = {
            let crate::app::Screen::Main(state) = &mut self.screen else {
                return;
            };
            state.coder.pipeline.as_ref().and_then(|p| p.first_start())
        };
        if let Some(action) = action {
            self.apply_pipeline_action(action);
        }
    }

    pub fn cancel_pipeline(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(pipeline) = state.coder.pipeline.as_mut() else {
            return;
        };
        pipeline.cancel_all();
        for id in [
            &pipeline.planner_id,
            &pipeline.coder_id,
            &pipeline.reviewer_id,
        ]
        .into_iter()
        .flatten()
        {
            if let Some(live) = state.coder.sessions.get_mut(id) {
                live.cancel();
            }
        }
        state.coder.status = Some("Pipeline cancelled.".into());
    }

    pub fn poll_pipeline(&mut self) {
        let events: Vec<crate::pipeline::PipelineEvent> = {
            let crate::app::Screen::Main(state) = &mut self.screen else {
                return;
            };
            let Some(rx) = &state.coder.pipeline_rx else {
                return;
            };
            let mut out = Vec::new();
            while let Ok(ev) = rx.try_recv() {
                out.push(ev);
            }
            out
        };
        for event in events {
            let review = {
                let crate::app::Screen::Main(state) = &self.screen else {
                    continue;
                };
                state.coder.project.as_ref().and_then(|p| {
                    crate::pipeline::contract::ContractStore::open(&p.canonical_root)
                        .reviewer()
                        .ok()
                        .flatten()
                })
            };
            let action = {
                let crate::app::Screen::Main(state) = &mut self.screen else {
                    continue;
                };
                let Some(pipeline) = state.coder.pipeline.as_mut() else {
                    continue;
                };
                pipeline.on_stage_finished(&event, review.as_ref())
            };
            self.apply_pipeline_action(action);
        }
    }

    fn apply_pipeline_action(&mut self, action: crate::pipeline::NextAction) {
        match action {
            crate::pipeline::NextAction::None => {}
            crate::pipeline::NextAction::Start {
                session,
                prompt,
                stage,
            } => {
                if let crate::app::Screen::Main(state) = &mut self.screen {
                    state.coder.sessions.select(&session);
                    if let Some(live) = state.coder.sessions.active_mut() {
                        live.transcript
                            .push(crate::session::TranscriptItem::User(prompt.clone()));
                        live.input.clear();
                    }
                    if let Some(pipeline) = state.coder.pipeline.as_mut() {
                        pipeline.note(stage, format!("Starting {}", stage.label()));
                    }
                }
                self.launch_coder_turn(Some(prompt));
            }
            crate::pipeline::NextAction::RunVerify => self.run_pipeline_verify(),
            crate::pipeline::NextAction::WaitGitGate => {
                if let crate::app::Screen::Main(state) = &mut self.screen {
                    state.coder.status =
                        Some("Git Gate: approve commit/push when you are ready.".into());
                }
            }
            crate::pipeline::NextAction::Stop { reason } => {
                if let crate::app::Screen::Main(state) = &mut self.screen {
                    state.coder.status = Some(reason);
                    if let Some(pipeline) = &state.coder.pipeline
                        && pipeline.review_cycles >= crate::pipeline::MAX_REVIEW_CYCLES
                        && let Some(store) = &state.coder.store
                        && let Ok(mut store) = store.lock()
                    {
                        let ids: Vec<String> = store
                            .tasks
                            .iter()
                            .filter(|t| t.status != crate::context::store::TaskStatus::Done)
                            .map(|t| t.id.clone())
                            .collect();
                        for id in ids {
                            let _ = store.upsert_task(
                                Some(id),
                                crate::context::store::TaskStatus::Open,
                                String::new(),
                            );
                        }
                    }
                }
            }
        }
    }

    fn run_pipeline_verify(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(project) = state.coder.project.clone() else {
            return;
        };
        let configs: Vec<_> = state
            .coder
            .run_configs
            .iter()
            .chain(state.coder.suggested_runs.iter())
            .cloned()
            .collect();
        let cmds = crate::pipeline::verify::plan_verify_commands(&project.canonical_root, &configs);
        let report = crate::pipeline::verify::run_verify(
            &cmds,
            &crate::pipeline::verify::SystemRunner,
            &project.canonical_root,
        );
        let summary = report.summary();
        let store = crate::pipeline::contract::ContractStore::open(&project.canonical_root);
        let mut coder = store.coder().ok().flatten().unwrap_or_default();
        coder.lint_results = report
            .steps
            .iter()
            .filter(|s| s.name != "test")
            .map(|s| format!("{}: {}", s.name, if s.passed { "pass" } else { "fail" }))
            .collect::<Vec<_>>()
            .join("\n");
        coder.test_results = report
            .steps
            .iter()
            .filter(|s| s.name == "test")
            .map(|s| s.output.clone())
            .collect::<Vec<_>>()
            .join("\n");
        coder.tests_executed = report.steps.iter().map(|s| s.name.clone()).collect();
        let _ = store.write_coder(&coder);
        let action = if let Some(pipeline) = state.coder.pipeline.as_mut() {
            pipeline.on_verify_finished(report.passed(), &summary)
        } else {
            crate::pipeline::NextAction::None
        };
        self.apply_pipeline_action(action);
    }

    pub fn select_coder_session(&mut self, id: SessionId) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.coder.sessions.select(&id);
        refresh_active_handoff(state);
    }

    pub fn close_coder_session(&mut self, id: SessionId) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.coder.sessions.close(&id);
        refresh_active_handoff(state);
    }

    pub fn rename_coder_session(&mut self, id: SessionId, label: String) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let label = label.trim().to_string();
        if label.is_empty() {
            if let Some(live) = state.coder.sessions.get_mut(&id) {
                live.editing_label = false;
            }
            return;
        }
        let (model, role) = state
            .coder
            .sessions
            .get_mut(&id)
            .map(|s| {
                s.set_label(label.clone());
                (s.model.clone(), s.role.id().to_string())
            })
            .unwrap_or_default();
        persist_live_session(state, &id, &label, &model);
        if let Some(project) = state.coder.project.clone() {
            persist_session_db(&self.db, &self.rt, &project, &id, &label, &model, &role);
        }
    }

    pub fn poll_terminal(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let mut events = Vec::new();
        while let Ok(event) = state.coder.terminal.rx.try_recv() {
            events.push(event);
        }
        for event in events {
            state.coder.terminal.apply(event);
        }
    }

    pub fn cancel_coder_command(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        if let Some(cancel) = &state.coder.terminal.cancel {
            cancel.cancel();
        }
    }

    pub fn refresh_context_if_stale(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let now = Instant::now();
        if state
            .coder
            .last_context_reload
            .is_some_and(|t| now.duration_since(t) < std::time::Duration::from_secs(2))
        {
            return;
        }
        if let Some(store) = &state.coder.store
            && let Ok(mut store) = store.try_lock()
        {
            store.reload();
            state.coder.last_context_reload = Some(now);
        }
    }

    pub fn open_orbit_folder(&mut self) {
        let crate::app::Screen::Main(state) = &self.screen else {
            return;
        };
        let Some(project) = &state.coder.project else {
            return;
        };
        let dir = OrbitStore::dir_for(&project.canonical_root);
        let program = if cfg!(windows) {
            "explorer"
        } else {
            "xdg-open"
        };
        if let Err(e) = std::process::Command::new(program).arg(&dir).spawn() {
            tracing::warn!("could not open {}: {e}", dir.display());
        }
    }

    pub fn raise_coder_budget(&mut self, new_cap: f64) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(live) = state.coder.sessions.active_mut() else {
            return;
        };
        live.budget_usd = new_cap;
        live.budget_prompt = None;
        if let Ok(mut session) = live.handle.try_lock() {
            session.limits.budget_usd = new_cap;
        }
        live.budget_bridge.resolve(Some(new_cap));
    }

    pub fn decline_coder_budget(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        if let Some(live) = state.coder.sessions.active_mut() {
            live.budget_prompt = None;
            live.budget_bridge.resolve(None);
        }
    }

    pub fn poll_restore(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(rx) = &state.coder.restore_rx else {
            return;
        };
        let Ok(snapshot) = rx.try_recv() else {
            return;
        };
        state.coder.restore_rx = None;
        apply_snapshot(state, snapshot);
        if state.coder.sessions.sessions.is_empty()
            && crate::app::can_create_session(state.credential.state).is_ok()
        {
            let name = state
                .coder
                .project
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "project".into());
            reset_agent_session(state, &name, true);
            if let (Some(project), Some(live)) =
                (state.coder.project.clone(), state.coder.sessions.active())
            {
                persist_session_db(
                    &self.db,
                    &self.rt,
                    &project,
                    &live.id,
                    &live.label,
                    &live.model,
                    live.role.id(),
                );
            }
        }
    }

    pub fn request_usage_report(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        state.coder.show_usage = true;
        let (tx, rx) = mpsc::channel();
        state.coder.usage_rx = Some(rx);
        let db = self.db.clone();
        self.rt.spawn_blocking(move || match db.usage_report() {
            Ok(report) => {
                let _ = tx.send(report);
            }
            Err(e) => tracing::warn!("usage report failed: {e:#}"),
        });
    }

    pub fn poll_usage_report(&mut self) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(rx) = &state.coder.usage_rx else {
            return;
        };
        let Ok(report) = rx.try_recv() else {
            return;
        };
        state.coder.usage_report = Some(report);
        state.coder.usage_rx = None;
    }

    pub fn set_coder_model(&mut self, model: String) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(live) = state.coder.sessions.active_mut() else {
            return;
        };
        if live.busy {
            return;
        }
        let id = live.id.clone();
        let label = live.label.clone();
        let role = live.role.id().to_string();
        live.set_model(model.clone());
        persist_live_session(state, &id, &label, &model);
        if let Some(project) = state.coder.project.clone() {
            persist_session_db(&self.db, &self.rt, &project, &id, &label, &model, &role);
        }
    }

    pub fn set_coder_role(&mut self, role: crate::session::AgentRole) {
        let crate::app::Screen::Main(state) = &mut self.screen else {
            return;
        };
        let Some(live) = state.coder.sessions.active_mut() else {
            return;
        };
        if live.busy {
            return;
        }
        let id = live.id.clone();
        let label = live.label.clone();
        let model = live.model.clone();
        live.set_role(role);
        persist_live_session(state, &id, &label, &model);
        if let Some(project) = state.coder.project.clone() {
            persist_session_db(&self.db, &self.rt, &project, &id, &label, &model, role.id());
        }
        refresh_active_handoff(state);
    }
}

fn start_scan(
    state: &mut MainState,
    project: Project,
    rt: Arc<tokio::runtime::Runtime>,
    db: Arc<Db>,
) {
    if let Some(cancel) = state.coder.scan_cancel.take() {
        cancel.cancel();
    }
    let recent = remember_project(&project);
    let _ = db.upsert_project(&project);
    let projects = crate::workspace::registry::list_recent(&db, 20).unwrap_or_default();
    let project = Arc::new(project);
    let project_name = project.name.clone();
    let project_row = (*project).clone();
    let (tx, rx) = mpsc::channel();
    let cancel = CancellationToken::new();
    let root = project.canonical_root.clone();
    let token = cancel.clone();
    rt.spawn_blocking(move || scan_project(root, tx, token));
    state.coder.project = Some(project);
    state.coder.recent = recent;
    state.coder.projects = projects;
    state.coder.projects_loaded = true;
    state.coder.run_configs = crate::workspace::run_config::load_saved(&project_row.canonical_root);
    state.coder.suggested_runs =
        crate::workspace::run_config::suggestions_not_saved(&project_row.canonical_root);
    state.coder.tree = FileTree::default();
    state.coder.scan_rx = Some(rx);
    state.coder.scan_cancel = Some(cancel);
    state.coder.scanning = true;
    state.coder.viewer = ViewerState::default();
    state.coder.selected = None;
    state.coder.status = None;
    reset_agent_session(state, &project_name, false);
    let (tx, rx) = mpsc::channel();
    state.coder.restore_rx = Some(rx);
    rt.spawn_blocking(move || {
        if let Err(e) = db.upsert_project(&project_row) {
            tracing::warn!("could not persist project: {e:#}");
        }
        match load_snapshot(&db, &project_row.id) {
            Ok(snapshot) => {
                let _ = tx.send(snapshot);
            }
            Err(e) => tracing::warn!("could not restore sessions: {e:#}"),
        }
    });
}

fn reset_agent_session(state: &mut MainState, project_name: &str, create_session: bool) {
    state.coder.sessions.shutdown();
    let model = state.settings.coder_default_model.clone();
    let id = if create_session {
        let id = state.coder.sessions.create(project_name, model.clone());
        apply_session_limits(state, &id);
        Some(id)
    } else {
        None
    };
    if let Some(project) = &state.coder.project {
        let loaded = CommandPolicy::load(&project.canonical_root);
        if let Ok(mut commands) = state.coder.policy.commands.lock() {
            *commands = loaded;
        }
        let mut store = OrbitStore::open(&project.canonical_root);
        if !store.warnings.is_empty() {
            state.coder.status = Some(store.warnings.join(" · "));
        }
        if let Some(id) = &id {
            let _ = store.upsert_session(SessionRecord {
                id: id.as_str().to_string(),
                label: project_name.to_string(),
                model: model.clone(),
                last_active_at: None,
                touched: Vec::new(),
            });
        }
        state.coder.store = Some(Arc::new(std::sync::Mutex::new(store)));
        state.coder.last_context_reload = Some(Instant::now());
    }
    state.coder.terminal.clear();
    let budget = state
        .coder
        .store
        .as_ref()
        .and_then(|s| s.lock().ok())
        .map(|s| s.settings.session_budget_usd)
        .unwrap_or(DEFAULT_SESSION_BUDGET_USD);
    if let Some(live) = state.coder.sessions.active_mut() {
        live.budget_usd = budget;
        if let Ok(mut session) = live.handle.try_lock() {
            session.limits.budget_usd = budget;
        }
    }
}

fn apply_session_limits(state: &mut MainState, id: &SessionId) {
    let max_iterations = state.settings.max_iterations;
    let budget = state.settings.session_budget_usd;
    if let Some(live) = state.coder.sessions.get_mut(id) {
        live.budget_usd = budget;
        if let Ok(mut session) = live.handle.try_lock() {
            session.limits.max_iterations = max_iterations;
            session.limits.budget_usd = budget;
        }
    }
}

fn flush_coder_state(state: &MainState, db: &Db) {
    let Some(project) = state.coder.project.as_ref() else {
        return;
    };
    if let Err(e) = db.upsert_project(project) {
        tracing::warn!("could not flush project: {e:#}");
    }
    for live in &state.coder.sessions.sessions {
        if let Err(e) = db.upsert_session_with_role(
            &project.id,
            &live.id,
            &live.label,
            &live.model,
            live.role.id(),
        ) {
            tracing::warn!("could not flush session: {e:#}");
        }
        if let Ok(session) = live.handle.try_lock() {
            if let Err(e) = db.replace_messages(&session.id, &session.messages) {
                tracing::warn!("could not flush messages: {e:#}");
            }
            if let Some(summary) = &session.context_summary
                && let Err(e) =
                    db.save_context_summary(&session.id, summary, session.context_summary_upto)
            {
                tracing::warn!("could not flush context summary: {e:#}");
            }
        }
    }
    if let Some(sid) = state.coder.sessions.active().map(|s| s.id.clone()) {
        for patch in &state.coder.pending_patches {
            if let Err(e) = db.upsert_file_change(&project.id, &sid, patch) {
                tracing::warn!("could not flush patch: {e:#}");
            }
        }
    }
}

fn persist_live_session(state: &mut MainState, id: &SessionId, label: &str, model: &str) {
    if let Some(store) = &state.coder.store
        && let Ok(mut store) = store.lock()
    {
        let _ = store.upsert_session(SessionRecord {
            id: id.as_str().to_string(),
            label: label.to_string(),
            model: model.to_string(),
            last_active_at: None,
            touched: Vec::new(),
        });
    }
}

fn refresh_active_handoff(state: &mut MainState) {
    let Some(store) = state.coder.store.clone() else {
        return;
    };
    let Ok(store) = store.lock() else {
        return;
    };
    let Some(live) = state.coder.sessions.active_mut() else {
        return;
    };
    let summary = build_handoff(&store, &live.id);
    if summary.is_interesting() && !live.busy {
        live.handoff = Some(summary);
        live.handoff_dismissed = false;
    } else {
        live.handoff = None;
    }
}

fn persist_session_db(
    db: &Arc<Db>,
    rt: &Arc<tokio::runtime::Runtime>,
    project: &Project,
    id: &SessionId,
    label: &str,
    model: &str,
    role: &str,
) {
    let db = db.clone();
    let project = project.clone();
    let id = id.clone();
    let label = label.to_string();
    let model = model.to_string();
    let role = role.to_string();
    rt.spawn_blocking(move || {
        if let Err(e) = db.upsert_project(&project) {
            tracing::warn!("could not persist project: {e:#}");
        }
        if let Err(e) = db.upsert_session_with_role(&project.id, &id, &label, &model, &role) {
            tracing::warn!("could not persist session: {e:#}");
        }
    });
}

fn persist_patch_db(
    db: &Arc<Db>,
    rt: &Arc<tokio::runtime::Runtime>,
    project_id: &str,
    session_id: &SessionId,
    patch: &FilePatch,
) {
    let db = db.clone();
    let project_id = project_id.to_string();
    let session_id = session_id.clone();
    let patch = patch.clone();
    rt.spawn_blocking(move || {
        if let Err(e) = db.upsert_file_change(&project_id, &session_id, &patch) {
            tracing::warn!("could not persist patch: {e:#}");
        }
    });
}

fn load_snapshot(db: &Db, project_id: &str) -> anyhow::Result<ProjectSnapshot> {
    let stored = db.load_sessions(project_id)?;
    let pending = db.pending_patches(project_id)?;
    let mut sessions = Vec::new();
    for row in stored {
        let id = SessionId::new(row.id.clone());
        let messages = db.load_messages(&id).unwrap_or_default();
        let usage = db.session_usage(&id).unwrap_or_default();
        let summary = db.load_context_summary(&id).ok().flatten();
        sessions.push(RestoredSession {
            id: row.id,
            label: row.label,
            model: row.model_id,
            role: row.role,
            messages,
            spent_usd: usage.cost_usd,
            prompt_tokens: usage.input_tokens,
            completion_tokens: usage.output_tokens,
            context_summary: summary.as_ref().map(|(t, _)| t.clone()),
            context_summary_upto: summary.map(|(_, n)| n).unwrap_or(0),
        });
    }
    Ok(ProjectSnapshot { sessions, pending })
}

fn apply_snapshot(state: &mut MainState, snapshot: ProjectSnapshot) {
    let mut pending = snapshot.pending;
    if let Some(project) = &state.coder.project {
        for patch in &mut pending {
            revalidate_patch(&project.canonical_root, patch);
        }
    }
    if snapshot.sessions.is_empty() {
        state.coder.pending_patches = pending;
        return;
    }
    state.coder.sessions.shutdown();
    let budget = state
        .coder
        .store
        .as_ref()
        .and_then(|s| s.lock().ok())
        .map(|s| s.settings.session_budget_usd)
        .unwrap_or(DEFAULT_SESSION_BUDGET_USD);
    let max_iterations = state.settings.max_iterations;
    for restored in snapshot.sessions {
        let mut session =
            Session::new(restored.label.clone(), restored.model.clone()).with_role(restored.role);
        session.id = SessionId::new(restored.id);
        session.messages = restored.messages.clone();
        session.context_summary = restored.context_summary;
        session.context_summary_upto = restored.context_summary_upto;
        session.limits.budget_usd = budget;
        session.limits.max_iterations = max_iterations;
        let mut live = crate::session::manager::LiveSession::from_session(session);
        live.transcript = transcript_from_messages(&restored.messages, &pending);
        live.spent_usd = restored.spent_usd;
        live.prompt_tokens = restored.prompt_tokens;
        live.completion_tokens = restored.completion_tokens;
        live.budget_usd = budget;
        state.coder.sessions.sessions.push(live);
    }
    if !state.coder.sessions.sessions.is_empty() {
        state.coder.sessions.active = 0;
    }
    state.coder.pending_patches = pending;
}

pub(crate) fn transcript_from_messages(
    messages: &[ChatMessage],
    pending: &[FilePatch],
) -> Vec<TranscriptItem> {
    let mut items = Vec::new();
    for message in messages {
        match message {
            ChatMessage::User { content, .. } => items.push(TranscriptItem::User(content.clone())),
            ChatMessage::Assistant {
                content,
                tool_calls,
            } => {
                if !content.is_empty() {
                    items.push(TranscriptItem::Assistant(content.clone()));
                }
                for call in tool_calls {
                    items.push(TranscriptItem::Tool {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        summary: summarize(&call.name, &call.arguments),
                        output: String::new(),
                        is_error: false,
                        running: false,
                        expanded: false,
                    });
                }
            }
            ChatMessage::ToolResult {
                call_id,
                content,
                is_error,
            } => {
                if let Some(TranscriptItem::Tool {
                    output,
                    is_error: err,
                    running,
                    ..
                }) = items.iter_mut().rev().find(|item| {
                    matches!(item, TranscriptItem::Tool { call_id: id, .. } if id == call_id)
                }) {
                    *output = content.clone();
                    *err = *is_error;
                    *running = false;
                }
            }
        }
    }
    for patch in pending {
        items.push(TranscriptItem::Approval {
            handle: crate::session::ApprovalHandle {
                id: ApprovalId::new(),
                tool_name: "write_file".into(),
                summary: format!("write_file(\"{}\")", patch.relative_path.display()),
                patch: Some(patch.clone()),
                command: None,
            },
            resolved: None,
        });
    }
    items
}

fn ingest_entries(tree: &mut FileTree, entries: &[ScanEntry]) {
    for entry in entries {
        tree.insert(&entry.relative, entry.is_dir);
    }
}

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 5_000;

fn load_and_highlight(project: &Project, relative: &std::path::Path) -> ViewerBody {
    if crate::security::is_sensitive(relative) {
        return ViewerBody::Error(
            "This file looks sensitive (.env, keys, certificates) and is not opened automatically."
                .into(),
        );
    }
    let path = match crate::security::resolve_within_root(&project.canonical_root, relative) {
        Ok(path) => path,
        Err(e) => return ViewerBody::Error(e.to_string()),
    };
    let meta = match std::fs::metadata(&path) {
        Ok(meta) => meta,
        Err(e) => return ViewerBody::Error(e.to_string()),
    };
    if meta.len() > MAX_FILE_BYTES {
        return ViewerBody::Error(format!(
            "File is {:.1} MB; the viewer limit is 2 MB.",
            meta.len() as f64 / (1024.0 * 1024.0)
        ));
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => return ViewerBody::Error(e.to_string()),
    };
    if bytes.contains(&0) {
        return ViewerBody::Error("Binary files cannot be opened in the viewer.".into());
    }
    let plain = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return ViewerBody::Error("File is not valid UTF-8.".into()),
    };
    let line_count = plain.lines().count();
    let highlighted = if line_count > MAX_HIGHLIGHT_LINES {
        None
    } else {
        Some(highlight_source(
            &plain,
            path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        ))
    };
    ViewerBody::Text { plain, highlighted }
}

fn highlight_source(src: &str, extension: &str) -> egui::text::LayoutJob {
    use std::sync::OnceLock;
    use syntect::easy::HighlightLines;
    use syntect::highlighting::ThemeSet;
    use syntect::parsing::SyntaxSet;
    use syntect::util::LinesWithEndings;

    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
    let themes = THEMES.get_or_init(ThemeSet::load_defaults);
    let syntax = syntaxes
        .find_syntax_by_extension(extension)
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
    let theme = themes
        .themes
        .get("base16-ocean.dark")
        .or_else(|| themes.themes.values().next())
        .expect("syntect default themes");
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::monospace(13.0);
    for line in LinesWithEndings::from(src) {
        let ranges = highlighter
            .highlight_line(line, syntaxes)
            .unwrap_or_else(|_| vec![(syntect::highlighting::Style::default(), line)]);
        for (style, text) in ranges {
            let fg = style.foreground;
            job.append(
                text,
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: crate::ui::theme::ansi_rgb(fg.r, fg.g, fg.b),
                    ..Default::default()
                },
            );
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::{TERMINAL_LINE_LIMIT, TerminalState};
    use crate::tools::shell::TerminalEvent;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn terminal_retains_at_most_limit_lines() {
        let mut term = TerminalState::default();
        term.apply(TerminalEvent::Started {
            command: "echo".into(),
            cancel: CancellationToken::new(),
        });
        let chunk = (0..(TERMINAL_LINE_LIMIT + 80))
            .map(|i| format!("line {i}\n"))
            .collect::<String>();
        term.apply(TerminalEvent::Chunk(chunk));
        assert_eq!(term.lines.len(), TERMINAL_LINE_LIMIT);
        assert!(
            term.view()
                .contains(&format!("line {}", TERMINAL_LINE_LIMIT + 79))
        );
        assert!(!term.view().contains("line 0\n"));
    }
}
