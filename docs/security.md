# Security

## Sandbox depends on the OS

**Windows:** there is no kernel jail. File tools stay inside the project root
(after symlink resolution). Commands run with `Command::new(program).args(...)`
in that root. That is **containment and policy**, not a jail. The sandbox
selector in Settings is disabled and says so.

**Linux:** child processes started by `run_command` and by run configs can be
confined with [Landlock](https://landlock.io). The ruleset is applied between
`fork` and `exec` of the child, never on the GUI process. Profiles:

| Profile | Read | Write |
|---|---|---|
| `off` (machine default) | unrestricted | unrestricted |
| `workspace` | project + toolchain + system paths | project, `/tmp`, `$CARGO_HOME`, `~/.cache` |
| `strict` | project + system paths | project, `/tmp` |

Restricted profiles do not grant `~/.ssh`, `~/.aws`, `~/.config/orbit`, or
the keyring socket. The UI reports the ABI actually obtained (`Active (ABI n)`),
or `Unavailable: kernel < 5.13` when Landlock cannot apply. A sandbox that
claims to be on while the kernel ignores it would be worse than no sandbox.

The profile is frozen for the life of a session. `.orbit/config.toml`
`[sandbox] profile = "..."` is commitable and **may only tighten** the
machine default from Settings. A cloned `profile = "off"` cannot turn off
someone else's local sandbox.

TCP `bind`/`connect` restrictions are a bonus on ABI v4 (kernel 6.7+) in the
`strict` profile, not a promise of every profile.

A determined model-plus-user-approval pair can still damage the machine. The
denylist blocks a short list of catastrophic shapes (`rm -rf /`, `format`,
`mkfs`, `dd of=/dev/*`, `curl | sh`, `shutdown`, `sh -c` / `cmd /C`). It is
not complete. Do not treat “the app asked me” as a substitute for reading the
command.

## What we do promise

- The OpenRouter key stays in the OS credential store. It is not written to
  `chats.json`, SQLite, `.orbit/`, logs, traces, or the diagnostic zip.
- Tool output is wrapped as **untrusted data** in the model history so a
  malicious file cannot quietly become a new system prompt.
- Paths are canonicalized before the “still under the project root” check.
- Sensitive names (`.env`, `*.pem`, `*.key`, `id_rsa`, `.git/config`, …)
  need an extra confirmation even to read.
- Environment variables whose names contain `KEY`, `TOKEN`, `SECRET`, or
  `PASSWORD` are stripped from child processes.

## Hooks are not a security boundary

`PreToolUse` / `PostToolUse` hooks declared in `.orbit/config.toml` are
project policy convenience. They run after the role guard and before (or
after) the tool. A broken hook **fails open**: timeout, empty stdout, or
unreadable JSON allows the tool and raises a warning. A hook command on the
absolute denylist never runs and cannot be approved.

Do not treat a hook deny as the product's security boundary. That remains
the role matrix, human approval, and the denylist.

## What we do not promise

- Isolation from other processes or the rest of your home directory once a
  command is approved, except the Landlock confinement of child processes on
  Linux when a restricted profile is active and the kernel ABI is available.
- A guarantee that every dangerous command is denylisted.
- Protection if you allowlist `bash` / `powershell` yourself (those forms
  that take a `-c` string are denied absolutely; do not look for workarounds).
- That a project hook will always run, or that a missing hook is a failure.

## Dependency advisories

`cargo audit` is run in CI. Two advisories are explicitly ignored in
`.cargo/audit.toml`: `RUSTSEC-2026-0194` and `RUSTSEC-2026-0195` in
`quick-xml 0.39`, pulled in by `wayland-scanner` (eframe). That crate
runs at **compile time** to generate Wayland bindings. It is not on the
OpenRouter request path. The ignore should be dropped when eframe
upgrades the scanner.

## Reporting

Open an issue on the repository. Do not attach diagnostic zips that you have
not inspected; the exporter is designed to strip secrets, but you should still
look.
