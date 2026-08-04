<div align="center">

# 💬 Lumen Chat

A lightweight, monolithic native chat client that connects to
[OpenRouter](https://openrouter.ai), giving you access to a wide range of
language models through a single, clean, and extremely lightweight interface.

![Rust](https://img.shields.io/badge/Rust-2024-000599C?logo=rust&logoColor=white)
![Platform](https://img.shields.io/badge/Windows%20%7C%20Linux-x64-0078D6?logo=rust&logoColor=white)

</div>

## Features

- **Secure API key storage.** API keys are stored in the native system
  credential service—Windows Credential Manager on Windows or Secret Service
  (GNOME Keyring/KWallet) on Linux—never in a plain text file or environment
  variables.
- **Key validation.** The key is verified against the OpenRouter API before
  being accepted.
- **Chat interface.** A simple message thread with a model picker, an
  attachment button, and a persistent input bar.
- **Super optimized.** Built in Rust, the whole application uses *80%* less
  system memory than a ChatGPT tab.

### Supported Models

| Provider | Models |
| :--- | :--- |
| **OpenAI** | GPT-5.6 Sol, GPT-5.6 Terra, GPT-5.6 Luna |
| **Google** | Gemini 3.1 Pro, Gemini 3.6 Flash, Gemini 3.5 Flash-Lite |
| **Anthropic** | Claude Fable 5, Claude Opus 5, Claude Sonnet 5 |
| **SpaceXAI** | Grok 4.5, Grok 4.3, Grok Build 0.1 |
| **Alibaba** | Qwen3.8-Max, Qwen3.7-Plus, Qwen3.6-Flash |
| **DeepSeek** | DeepSeek V4 Pro, DeepSeek V4 Flash, DeepSeek V3.2 |
| **Z.ai** | GLM-5.2, GLM-5.1, GLM-5V-Turbo |
| **MoonshotAI** | Kimi K3, Kimi K2.7 Code, Kimi K2.6 |
| **MiniMax** | MiniMax-M3, MiniMax-M2.7, MiniMax-M2-her |
| **Xiaomi** | MiMo-V2.5-Pro, MiMo-V2.5, MiMo-V2-Flash |
| **Coding-focused** | GPT-5.6 Sol Pro, Claude Opus 4.8, KAT-Coder-Pro V2.5 |

## Requirements

- Windows 10 or later, or a mainstream x86_64 Linux desktop distribution
- A valid [OpenRouter API key](https://openrouter.ai/keys)
- Windows: [Visual C++ for Visual Studio 2015-2022 x64](https://aka.ms/vs/17/release/vc_redist.x64.exe)
- Linux: an X11 or Wayland desktop session with D-Bus Secret Service available
  and unlocked (for example, GNOME Keyring or KWallet). WSL and headless Linux
  are not supported.
- [Rust toolchain](https://rustup.rs) (only for building from source)

## Building

```sh
cargo build --release
```

On Debian/Ubuntu, install the native development libraries before building:

```sh
sudo apt install build-essential pkg-config libdbus-1-dev libgl1-mesa-dev libwayland-dev libx11-dev libxcursor-dev libxi-dev libxinerama-dev libxkbcommon-dev libxrandr-dev
```
