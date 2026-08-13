//! Sanitized diagnostic bundle. Never includes credentials.

use crate::security::redact_secrets;
use crate::storage::data_dir;
use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

pub fn export_bundle(dest: impl AsRef<Path>) -> Result<PathBuf> {
    let dest = dest.as_ref().to_path_buf();
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file =
        std::fs::File::create(&dest).with_context(|| format!("creating {}", dest.display()))?;
    let mut zip = ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("version.txt", opts)?;
    writeln!(
        zip,
        "orbit {}\nos {} {}\nschema {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        schema_version()
    )?;

    zip.start_file("providers.txt", opts)?;
    writeln!(zip, "openrouter (no credentials included)")?;
    writeln!(zip, "id=openrouter")?;

    zip.start_file("schema_version.txt", opts)?;
    writeln!(zip, "{}", schema_version())?;

    zip.start_file("logs.txt", opts)?;
    let logs = collect_logs();
    zip.write_all(redact_secrets(&logs).as_bytes())?;

    zip.finish()?;
    Ok(dest)
}

fn schema_version() -> i32 {
    let path = data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("orbit.db");
    let Ok(conn) = rusqlite::Connection::open(&path) else {
        return 0;
    };
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

fn collect_logs() -> String {
    let path = data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("orbit.log");
    match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => format!(
            "No persistent log file at {}.\nOrbit logs to stderr; this bundle contains runtime metadata only.\n",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::export_bundle;
    use crate::security::redact::contains_secret;
    use std::io::Read;
    use tempfile::TempDir;
    use zip::ZipArchive;

    #[test]
    fn bundle_contains_no_secrets() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("diag.zip");
        export_bundle(&dest).unwrap();
        let file = std::fs::File::open(&dest).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();
        let mut names = Vec::new();
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).unwrap();
            names.push(entry.name().to_string());
            let mut body = String::new();
            entry.read_to_string(&mut body).unwrap();
            assert!(!contains_secret(&body), "{} leaked a secret", entry.name());
            assert!(
                !body.to_ascii_lowercase().contains("sk-or-v1-secret"),
                "{}",
                entry.name()
            );
            assert!(!body.contains("Bearer sk-"));
        }
        assert!(names.iter().any(|n| n == "version.txt"));
        assert!(names.iter().any(|n| n == "providers.txt"));
        assert!(names.iter().any(|n| n == "logs.txt"));
        let providers = {
            let mut e = zip.by_name("providers.txt").unwrap();
            let mut s = String::new();
            e.read_to_string(&mut s).unwrap();
            s
        };
        assert!(providers.contains("openrouter"));
        assert!(!providers.to_ascii_lowercase().contains("authorization"));
    }
}
