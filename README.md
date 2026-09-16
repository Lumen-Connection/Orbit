<div align="center">

# 🪐 Orbit

A native and fast AI interface, including a **Chat Mode**
for conversations and a **Coder Mode** for agentic programming.

![Rust](https://img.shields.io/badge/Rust-2024-000599C?logo=rust&logoColor=white)
![Platform](https://img.shields.io/badge/Windows%20%7C%20Linux-x64-0078D6?logo=rust&logoColor=white)

</div>

## Features


- **Chat Mode** — a simple and extremely lightweight AI chat interface with all SOTA models.
- **Coder Mode** — a multi-agent, highly programmable AI-first coding environment.


## Install

Download the portable zip file or the Linux AppImage from
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

Requirements: 
* Windows 11 or a mainstream x64 Linux desktop (X11 or Wayland,
unlocked Secret Service). 
* An [OpenRouter API key](https://openrouter.ai/keys), Anthropic API key, or OpenAI API key.
* Windows also needs [Visual C++ 2015–2022 x64](https://aka.ms/vs/17/release/vc_redist.x64.exe).

## Contributing

See [packaging/README.md](packaging/README.md). Tag a version (`v1.0.0`) to
run the release workflow.
