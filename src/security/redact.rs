//! Secret redaction for logs, traces and diagnostic exports.

use regex::Regex;
use std::sync::OnceLock;

/// Replace API keys, bearer tokens and Authorization headers.
pub fn redact_secrets(input: &str) -> String {
    let mut out = OPENROUTER_KEY
        .get_or_init(|| Regex::new(r"sk-or-v1-[A-Za-z0-9_-]{8,}").expect("openrouter key pattern"))
        .replace_all(input, "sk-or-v1-[REDACTED]")
        .into_owned();
    out = BEARER
        .get_or_init(|| Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._\-+=/]{8,}").expect("bearer"))
        .replace_all(&out, "${1}[REDACTED]")
        .into_owned();
    AUTHORIZATION
        .get_or_init(|| {
            Regex::new(r"(?i)(authorization\s*[:=]\s*)\S+").expect("authorization header")
        })
        .replace_all(&out, "${1}[REDACTED]")
        .into_owned()
}

/// Write sink that redacts secrets before they hit the underlying writer.
pub struct RedactingWriter<W> {
    inner: W,
}

impl<W: std::io::Write> RedactingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }
}

impl<W: std::io::Write> std::io::Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let redacted = redact_secrets(&String::from_utf8_lossy(buf));
        self.inner.write_all(redacted.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

static OPENROUTER_KEY: OnceLock<Regex> = OnceLock::new();
static BEARER: OnceLock<Regex> = OnceLock::new();
static AUTHORIZATION: OnceLock<Regex> = OnceLock::new();

/// `true` when `haystack` still contains a raw secret that looks like an API key.
#[cfg(test)]
pub fn contains_secret(haystack: &str) -> bool {
    OPENROUTER_KEY
        .get_or_init(|| Regex::new(r"sk-or-v1-[A-Za-z0-9_-]{8,}").expect("openrouter key pattern"))
        .is_match(haystack)
}

#[cfg(test)]
mod tests {
    use super::{contains_secret, redact_secrets};
    use tracing::subscriber::with_default;
    use tracing_subscriber::fmt::MakeWriter;

    const SAMPLE_KEY: &str = "sk-or-v1-SECRETtestkey1234567890abcd";

    #[derive(Clone)]
    struct Buf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for Buf {
        type Writer = GuardWriter;

        fn make_writer(&'a self) -> Self::Writer {
            GuardWriter(self.0.clone())
        }
    }

    struct GuardWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for GuardWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let redacted = redact_secrets(&String::from_utf8_lossy(buf));
            self.0
                .lock()
                .unwrap()
                .extend_from_slice(redacted.as_bytes());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn redacts_openrouter_keys_and_headers() {
        let raw =
            format!("key={SAMPLE_KEY} Authorization: Bearer {SAMPLE_KEY} bearer {SAMPLE_KEY}");
        let clean = redact_secrets(&raw);
        assert!(!contains_secret(&clean), "{clean}");
        assert!(clean.contains("sk-or-v1-[REDACTED]"));
        assert!(clean.contains("[REDACTED]"));
        assert!(!clean.contains("SECRETtestkey"));
    }

    #[test]
    fn debug_tracing_does_not_emit_raw_key() {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(Buf(buf.clone()))
            .with_ansi(false)
            .finish();
        with_default(subscriber, || {
            // A careless log of the key must still be redacted by the writer.
            tracing::debug!(
                authorization = %format!("Bearer {SAMPLE_KEY}"),
                "validating OpenRouter key"
            );
            tracing::debug!("raw key in message: {SAMPLE_KEY}");
        });
        let logged = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            !contains_secret(&logged),
            "debug log leaked a secret:\n{logged}"
        );
        assert!(!logged.contains("SECRETtestkey"));
    }
}
