//! The log-capture check (D-015, review F11): captures everything a test or drill logs and
//! fails on a known secret or a credential-shaped string. Test support only.

use std::io;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use tracing_subscriber::fmt::MakeWriter;

/// The development passwords every test environment uses (public values, but a log line
/// carrying one carries a password).
pub const DEV_SECRETS: [&str; 3] = ["platform-dev", "platform-owner-dev", "platform-app-dev"];

/// Everything written, kept in memory.
#[derive(Debug, Clone, Default)]
pub struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    /// The text captured so far.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }

    /// Appends text captured elsewhere (a child process's stderr).
    pub fn push(&self, line: &str) {
        let mut buffer = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        buffer.extend_from_slice(line.as_bytes());
        buffer.push(b'\n');
    }
}

/// A writer into [`Captured`].
#[derive(Debug)]
pub struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for CaptureWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Captured {
    type Writer = CaptureWriter;
    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriter(self.0.clone())
    }
}

/// A second dispatcher registered for the life of the process. While only one dispatcher
/// is registered, tracing computes a callsite's interest from the default of whichever
/// thread reaches the callsite first: a parallel test with no subscriber then caches
/// "never" for every thread, and the capture silently misses that callsite. With two
/// registered, interest is computed over every live dispatcher.
static ANCHOR: OnceLock<tracing::Dispatch> = OnceLock::new();

/// Captures every `tracing` event at TRACE (sqlx's included) on the current thread until
/// the guard drops.
pub fn capture() -> (Captured, tracing::subscriber::DefaultGuard) {
    ANCHOR.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (captured, guard)
}

/// Every leak in `text`: one of `secrets` or [`DEV_SECRETS`], or a URL that carries a
/// password (`scheme://user:password@host`).
pub fn leaks(text: &str, secrets: &[&str]) -> Vec<String> {
    let mut found: Vec<String> = DEV_SECRETS
        .iter()
        .chain(secrets)
        .filter(|s| !s.is_empty() && text.contains(**s))
        .map(|s| format!("secret {s:?}"))
        .collect();
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let after = &rest[at + 3..];
        let authority_end = after
            .find(|c: char| c == '/' || c.is_whitespace() || c == '"')
            .unwrap_or(after.len());
        let authority = &after[..authority_end];
        if let Some((userinfo, _host)) = authority.rsplit_once('@')
            && userinfo.contains(':')
        {
            found.push(format!(
                "credentials in a URL: {}",
                &rest[at.saturating_sub(10)..(at + 3 + authority_end).min(rest.len())]
            ));
        }
        rest = after;
    }
    found
}

/// Panics with every leak found (the test's failure report).
pub fn assert_no_leaks(text: &str, secrets: &[&str], what: &str) {
    let found = leaks(text, secrets);
    assert!(
        found.is_empty(),
        "{what}: secrets reached the logs: {found:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_known_secrets_and_credential_urls() {
        assert_eq!(leaks("connected", &["tok-123"]), Vec::<String>::new());
        assert_eq!(leaks("bearer tok-123 used", &["tok-123"]).len(), 1);
        assert_eq!(
            leaks("url=postgres://platform_app:hunter2@127.0.0.1:5432/db", &[]).len(),
            1
        );
        assert_eq!(
            leaks("url=postgres://127.0.0.1:5432/db and http://host:80/x", &[]),
            Vec::<String>::new()
        );
        assert_eq!(leaks("pw platform-app-dev", &[]).len(), 1);
    }
}
