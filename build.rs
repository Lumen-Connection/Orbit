#[cfg(windows)]
fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Orbit");
        res.set("FileDescription", "Orbit — chat and local coding agents");
        res.set("CompanyName", "Lumen Connection");
        res.set("LegalCopyright", "🄯 2026 Lumen Connection");
        res.compile().expect("failed to compile Windows resources");
    }
}

#[cfg(not(windows))]
fn main() {}
