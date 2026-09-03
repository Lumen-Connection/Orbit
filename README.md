<div align="center">

# Orbit

A native desktop client for [OpenRouter](https://openrouter.ai): **Chat Mode**
for conversations, **Coder Mode** for local coding agents that share a
versionable Project Context.

![Rust](https://img.shields.io/badge/Rust-2024-000599C?logo=rust&logoColor=white)
![Platform](https://img.shields.io/badge/Windows%20%7C%20Linux-x64-0078D6?logo=rust&logoColor=white)

</div>

## Features


- **Chat Mode** — the original lightweight OpenRouter chat client: streaming
  replies, cancel, a live model catalog, and chats stored in the OS data
  directory.
- **Agent Mode** — open a local folder. Independent agent sessions (different
  models) read, search, edit and run commands in that project. Writes and new
  commands wait for you. They share a human-readable `.orbit/` context so
  switching models is a handoff, not a restart.


## Install

### From a release

Download the Windows installer (or portable version) or the Linux AppImage from
[Releases](https://github.com/Lumen-Connection/Orbit/releases).

### Building

```sh
cargo build --release
```

Debian/Ubuntu build dependencies:

```sh
sudo apt install build-essential pkg-config libdbus-1-dev libgl1-mesa-dev \
  libwayland-dev libx11-dev libxcursor-dev libxi-dev libxinerama-dev \
  libxkbcommon-dev libxrandr-dev
```

Requirements: Windows 10+ or a mainstream x86_64 Linux desktop (X11 or Wayland,
unlocked Secret Service). An [OpenRouter API key](https://openrouter.ai/keys).
Windows also needs [VC++ 2015–2022 x64](https://aka.ms/vs/17/release/vc_redist.x64.exe).

## First run

1. Launch Orbit. Paste your OpenRouter key. It is checked against the API and
   stored in the OS keyring.
2. **Chat Mode** — pick a model and talk. Enter sends, Shift+Enter is a newline,
   **Stop** cancels.
3. **Coder Mode** — Open folder. Ask the agent to inspect or change the project.
   Read tools run immediately. Writes show a diff; **Apply** or **Deny**.
   New commands ask once, then join the allowlist.

## A first Coder task

1. Open this repository (or any Rust crate).
2. Ask: *find `authenticate` and rename it if it exists; then run `cargo --version` and record the decision*.
3. Approve the edit and the command when prompted.
4. Open `.orbit/decisions.md` in your editor — the decision should be there
   with model, session label and timestamp.

## `.orbit/` layout

Created on first open, meant to be committed:

| File | Role |
|---|---|
| `context.md` | Project goal, architecture, constraints |
| `decisions.md` | Append-only decision log |
| `findings.md` | Append-only findings |
| `tasks.md` | Checkbox tasks |
| `sessions.json` | Session index and file touches |
| `config.toml` | Command allowlist, digest limits, spend cap |

Details: [docs/orbit-dir.md](docs/orbit-dir.md).  
Allowlist and budgets: [docs/config.md](docs/config.md).

## Contributing

See [packaging/README.md](packaging/README.md). Tag a version (`v1.0.0`) to
run the release workflow.
