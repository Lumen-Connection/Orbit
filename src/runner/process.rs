//! Process group lifecycle: start / stop / restart / kill-all.

use super::ansi::{OutputLine, split_complete_lines};
use crate::workspace::run_config::RunConfig;
use command_group::AsyncCommandGroup;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc as tokio_mpsc;
use tokio::time::{Duration, sleep};

pub const LINE_LIMIT: usize = 5_000;
const GRACEFUL_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    Starting,
    Running,
    Stopping,
    Exited,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ProcessView {
    pub config_id: String,
    pub name: String,
    #[allow(dead_code)]
    pub display: String,
    pub pid: Option<u32>,
    pub started_at: Instant,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub unexpected: bool,
    pub lines: VecDeque<OutputLine>,
    pub follow: bool,
}

impl ProcessView {
    pub fn duration_label(&self) -> String {
        let secs = self.started_at.elapsed().as_secs();
        format!("{secs}s")
    }
}

#[derive(Debug, Clone)]
pub enum RunnerEvent {
    Started {
        id: String,
        pid: Option<u32>,
    },
    Lines {
        id: String,
        lines: Vec<OutputLine>,
    },
    Status {
        id: String,
        status: ProcessStatus,
        exit_code: Option<i32>,
        unexpected: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunControl {
    Stop,
    Kill,
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("that run config is already running")]
    AlreadyRunning,
}

pub struct ProcessRegistry {
    pub processes: HashMap<String, ProcessView>,
    controls: HashMap<String, tokio_mpsc::Sender<RunControl>>,
    pub tx: Sender<RunnerEvent>,
    pub rx: Receiver<RunnerEvent>,
    pub restart_pending: HashMap<String, (RunConfig, PathBuf)>,
    pub sandbox: crate::security::sandbox::SandboxProfile,
}

impl Default for ProcessRegistry {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            processes: HashMap::new(),
            controls: HashMap::new(),
            tx,
            rx,
            restart_pending: HashMap::new(),
            sandbox: crate::security::sandbox::SandboxProfile::Off,
        }
    }
}

impl Drop for ProcessRegistry {
    fn drop(&mut self) {
        self.kill_all();
    }
}

impl ProcessRegistry {
    pub fn running_count(&self) -> u32 {
        self.processes
            .iter()
            .filter(|(_, p)| {
                matches!(
                    p.status,
                    ProcessStatus::Starting | ProcessStatus::Running | ProcessStatus::Stopping
                )
            })
            .count() as u32
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.processes.get(id).is_some_and(|p| {
            matches!(
                p.status,
                ProcessStatus::Starting | ProcessStatus::Running | ProcessStatus::Stopping
            )
        })
    }

    pub fn last_lines(&self, id: &str, n: usize) -> Option<(Vec<String>, bool)> {
        let view = self.processes.get(id)?;
        let take = n.min(1_000);
        let truncated = view.lines.len() > take;
        let lines: Vec<String> = view
            .lines
            .iter()
            .rev()
            .take(take)
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Some((lines, truncated))
    }

    pub fn clear_output(&mut self, id: &str) {
        if let Some(view) = self.processes.get_mut(id) {
            view.lines.clear();
        }
    }

    pub fn start(
        &mut self,
        config: RunConfig,
        cwd: PathBuf,
        rt: Option<&tokio::runtime::Runtime>,
    ) -> Result<(), StartError> {
        if self.is_running(&config.id) {
            return Err(StartError::AlreadyRunning);
        }
        let id = config.id.clone();
        let (ctrl_tx, ctrl_rx) = tokio_mpsc::channel(4);
        self.controls.insert(id.clone(), ctrl_tx);
        self.processes.insert(
            id.clone(),
            ProcessView {
                config_id: id.clone(),
                name: config.name.clone(),
                display: config.display(),
                pid: None,
                started_at: Instant::now(),
                status: ProcessStatus::Starting,
                exit_code: None,
                unexpected: false,
                lines: VecDeque::new(),
                follow: true,
            },
        );
        let events = self.tx.clone();
        let sandbox = self.sandbox;
        if let Some(rt) = rt {
            rt.spawn(run_loop(config, cwd, ctrl_rx, events, sandbox));
        } else {
            tokio::spawn(run_loop(config, cwd, ctrl_rx, events, sandbox));
        }
        Ok(())
    }

    pub fn stop(&mut self, id: &str) {
        if let Some(tx) = self.controls.get(id) {
            let _ = tx.try_send(RunControl::Stop);
            if let Some(view) = self.processes.get_mut(id) {
                view.status = ProcessStatus::Stopping;
            }
        }
    }

    #[allow(dead_code)]
    pub fn kill(&mut self, id: &str) {
        if let Some(tx) = self.controls.get(id) {
            let _ = tx.try_send(RunControl::Kill);
        }
    }

    pub fn kill_all(&mut self) {
        for tx in self.controls.values() {
            let _ = tx.try_send(RunControl::Kill);
        }
    }

    pub fn request_restart(
        &mut self,
        config: RunConfig,
        cwd: PathBuf,
        rt: Option<&tokio::runtime::Runtime>,
    ) {
        let id = config.id.clone();
        if self.is_running(&id) {
            self.restart_pending.insert(id.clone(), (config, cwd));
            self.stop(&id);
        } else {
            let _ = self.start(config, cwd, rt);
        }
    }

    pub fn poll(&mut self, rt: Option<&tokio::runtime::Runtime>) {
        let mut events = Vec::new();
        while let Ok(event) = self.rx.try_recv() {
            events.push(event);
        }
        let mut finished = Vec::new();
        for event in events {
            match event {
                RunnerEvent::Started { id, pid } => {
                    if let Some(view) = self.processes.get_mut(&id) {
                        view.pid = pid;
                        view.status = ProcessStatus::Running;
                    }
                }
                RunnerEvent::Lines { id, lines } => {
                    if let Some(view) = self.processes.get_mut(&id) {
                        for line in lines {
                            if view.lines.len() >= LINE_LIMIT {
                                view.lines.pop_front();
                            }
                            view.lines.push_back(line);
                        }
                    }
                }
                RunnerEvent::Status {
                    id,
                    status,
                    exit_code,
                    unexpected,
                } => {
                    if let Some(view) = self.processes.get_mut(&id) {
                        view.status = status;
                        view.exit_code = exit_code;
                        view.unexpected = unexpected;
                    }
                    if matches!(status, ProcessStatus::Exited | ProcessStatus::Failed) {
                        self.controls.remove(&id);
                        finished.push(id);
                    }
                }
            }
        }
        for id in finished {
            if let Some((config, cwd)) = self.restart_pending.remove(&id) {
                let _ = self.start(config, cwd, rt);
            }
        }
    }
}

async fn run_loop(
    config: RunConfig,
    cwd: PathBuf,
    mut ctrl: tokio_mpsc::Receiver<RunControl>,
    events: Sender<RunnerEvent>,
    sandbox: crate::security::sandbox::SandboxProfile,
) {
    let id = config.id.clone();
    let mut cmd = Command::new(&config.program);
    cmd.args(&config.args)
        .current_dir(&cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    crate::security::sandbox::apply_to_tokio(&mut cmd, sandbox, &cwd);

    let mut child = match cmd.group_spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = events.send(RunnerEvent::Status {
                id,
                status: ProcessStatus::Failed,
                exit_code: None,
                unexpected: true,
            });
            let _ = events.send(RunnerEvent::Lines {
                id: config.id,
                lines: split_complete_lines(&format!("{e}\n"), &mut String::new()),
            });
            return;
        }
    };

    let pid = child.id();
    let _ = events.send(RunnerEvent::Started {
        id: id.clone(),
        pid,
    });

    if let Some(stdout) = child.inner().stdout.take() {
        spawn_pump(stdout, id.clone(), events.clone());
    }
    if let Some(stderr) = child.inner().stderr.take() {
        spawn_pump(stderr, id.clone(), events.clone());
    }

    let mut stopping = false;
    let exit = loop {
        tokio::select! {
            status = child.wait() => {
                break status.ok().and_then(|s| s.code());
            }
            msg = ctrl.recv() => {
                match msg {
                    Some(RunControl::Kill) | None => {
                        let _ = child.start_kill();
                        let status = child.wait().await.ok().and_then(|s| s.code());
                        break status;
                    }
                    Some(RunControl::Stop) if !stopping => {
                        stopping = true;
                        graceful_stop(&mut child, pid);
                        tokio::select! {
                            status = child.wait() => {
                                break status.ok().and_then(|s| s.code());
                            }
                            _ = sleep(GRACEFUL_WAIT) => {
                                let _ = child.start_kill();
                                break child.wait().await.ok().and_then(|s| s.code());
                            }
                        }
                    }
                    Some(RunControl::Stop) => {}
                }
            }
        }
    };

    let _ = events.send(RunnerEvent::Status {
        id,
        status: ProcessStatus::Exited,
        exit_code: exit,
        unexpected: !stopping,
    });
}

fn spawn_pump<R>(mut reader: R, id: String, events: Sender<RunnerEvent>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        let mut partial = String::new();
        loop {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    let lines = split_complete_lines(&chunk, &mut partial);
                    if !lines.is_empty() {
                        let _ = events.send(RunnerEvent::Lines {
                            id: id.clone(),
                            lines,
                        });
                    }
                }
            }
        }
        if !partial.is_empty() {
            let lines = split_complete_lines("\n", &mut partial);
            if !lines.is_empty() {
                let _ = events.send(RunnerEvent::Lines { id, lines });
            }
        }
    });
}

fn graceful_stop(child: &mut command_group::AsyncGroupChild, pid: Option<u32>) {
    let _ = child;
    #[cfg(unix)]
    {
        use command_group::UnixChildExt;
        let _ = child.signal(command_group::Signal::SIGTERM);
        let _ = pid;
    }
    #[cfg(windows)]
    {
        // Job Objects have no SIGTERM. Killing the job still reaps the
        // whole tree (the port-leak case). CTRL_BREAK is unsafe here:
        // it can land on the Orbit console and halt the UI/tests.
        let _ = pid;
        let _ = child.start_kill();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = child.start_kill();
        let _ = pid;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::run_config::RunKind;

    fn sleep_config() -> RunConfig {
        #[cfg(windows)]
        {
            RunConfig::new(
                "ping",
                "ping",
                vec!["-n".into(), "20".into(), "127.0.0.1".into()],
                RunKind::LongRunning,
            )
        }
        #[cfg(not(windows))]
        {
            RunConfig::new("sleep", "sleep", vec!["20".into()], RunKind::LongRunning)
        }
    }

    #[test]
    fn buffer_drops_oldest_at_limit() {
        let mut view = ProcessView {
            config_id: "x".into(),
            name: "x".into(),
            display: "x".into(),
            pid: None,
            started_at: Instant::now(),
            status: ProcessStatus::Running,
            exit_code: None,
            unexpected: false,
            lines: VecDeque::new(),
            follow: true,
        };
        for i in 0..(LINE_LIMIT + 10) {
            if view.lines.len() >= LINE_LIMIT {
                view.lines.pop_front();
            }
            view.lines.push_back(OutputLine {
                text: format!("{i}"),
                spans: Vec::new(),
            });
        }
        assert_eq!(view.lines.len(), LINE_LIMIT);
        assert_eq!(view.lines.front().unwrap().text, "10");
    }

    #[test]
    fn start_stop_clears_running_state() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut reg = ProcessRegistry::default();
        let cfg = sleep_config();
        let id = cfg.id.clone();
        reg.start(cfg, std::env::temp_dir(), Some(&rt)).unwrap();
        // Allow the spawn to publish Started.
        std::thread::sleep(std::time::Duration::from_millis(200));
        reg.poll(Some(&rt));
        assert!(reg.is_running(&id), "process should be running");
        reg.kill(&id);
        let deadline = Instant::now() + std::time::Duration::from_secs(8);
        while Instant::now() < deadline {
            reg.poll(Some(&rt));
            if !reg.is_running(&id) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!reg.is_running(&id), "stop must release the process");
    }
}
