# Packaging

Release artifacts are built by `.github/workflows/release.yml` on tags
matching `v*`.

## Windows

- `target/release/orbit.exe` plus this tree’s `assets/icon.ico`.
- WiX source: `packaging/windows/orbit.wxs` (MSI).
- Portable fallback: zip of the exe, README and LICENSE.

Local MSI (WiX 4+):

```sh
dotnet tool install -g wix
wix build packaging/windows/orbit.wxs -d Version=1.0.0 -o Orbit.msi
```

The exe path is passed as `-d OrbitExe=...` from CI after `cargo build --release`.

## Linux

- `packaging/linux/orbit.desktop`
- `packaging/linux/build-appimage.sh` assembles an AppDir and calls
  `appimagetool` (downloaded by the script if missing).
