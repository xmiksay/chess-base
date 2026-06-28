//! Best-effort browser launch for local mode.

/// Open `url` in the user's default browser. Failure is non-fatal — the URL is
/// always also printed to stdout.
pub fn open(url: &str) {
    if let Err(e) = open::that(url) {
        tracing::warn!(error = %e, "could not open browser automatically");
    }
}
