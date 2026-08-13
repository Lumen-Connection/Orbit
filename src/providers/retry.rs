//! Classify provider failures and compute retry backoff.

use std::time::Duration;

pub const MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    Transient,
    Permanent,
}

pub fn classify_http_status(status: u16) -> ErrorClass {
    match status {
        429 | 500 | 502 | 503 | 504 => ErrorClass::Transient,
        400 | 401 | 403 | 404 | 422 => ErrorClass::Permanent,
        s if (500..600).contains(&s) => ErrorClass::Transient,
        _ => ErrorClass::Permanent,
    }
}

pub fn parse_retry_after(raw: Option<&str>) -> Option<u64> {
    let raw = raw?.trim();
    raw.parse::<u64>().ok().map(|s| s.min(30))
}

/// `attempt` is the attempt that just failed (1-based).
pub fn wait_duration(attempt: u32, retry_after: Option<u64>) -> Duration {
    if let Some(secs) = retry_after {
        return Duration::from_secs(secs.min(30));
    }
    let exp = 1u64 << attempt.saturating_sub(1).min(4);
    Duration::from_secs(exp.min(16))
}

pub fn hint_for_status(status: u16) -> &'static str {
    match status {
        400 => "The request was rejected. Check the model id and try a shorter prompt.",
        401 | 403 => "Update the API key in Settings.",
        404 => "This model is not available. Pick another from the catalog.",
        429 => "Rate limited. Orbit will retry automatically.",
        500 | 502 | 503 | 504 => "OpenRouter is having trouble. Orbit will retry automatically.",
        _ => "See the error details and try again.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_transient_and_permanent() {
        assert_eq!(classify_http_status(429), ErrorClass::Transient);
        assert_eq!(classify_http_status(503), ErrorClass::Transient);
        assert_eq!(classify_http_status(401), ErrorClass::Permanent);
        assert_eq!(classify_http_status(400), ErrorClass::Permanent);
        assert_eq!(classify_http_status(404), ErrorClass::Permanent);
    }

    #[test]
    fn retry_after_zero_is_respected() {
        assert_eq!(parse_retry_after(Some("0")), Some(0));
        assert_eq!(wait_duration(1, Some(0)), Duration::from_secs(0));
    }

    #[test]
    fn backoff_grows_without_retry_after() {
        assert_eq!(wait_duration(1, None), Duration::from_secs(1));
        assert_eq!(wait_duration(2, None), Duration::from_secs(2));
        assert_eq!(wait_duration(3, None), Duration::from_secs(4));
    }
}
