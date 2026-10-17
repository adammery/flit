use serde::Serializer;

/// App-wide error type returned by every fallible `#[tauri::command]`.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("keychain error: {0}")]
    Keychain(#[from] keyring::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("background task failed: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),
    #[error("window error: {0}")]
    Tauri(#[from] tauri::Error),
    // why: a String, not #[from] — async-imap's login returns (Error, Client)
    // tuples and we add context (connect/tls/login) at each call site.
    #[error("imap error: {0}")]
    Imap(String),
    #[error("smtp error: {0}")]
    Smtp(String),
    /// Model download problems — a String like Imap, with context added at
    /// the call site (status, verification, size).
    #[error("download error: {0}")]
    Http(String),
    /// The local model could not load or answer; the message says which
    /// step (load, prompt, context, generation) gave up.
    #[error("local model error: {0}")]
    Llm(String),
    /// OAuth sign-in or token refresh failed; the message says which step.
    #[error("sign-in error: {0}")]
    OAuth(String),
    /// The provider no longer honours the stored sign-in (revoked, password
    /// changed, or unused too long). Only signing in again fixes it.
    #[error("sign-in expired: sign in to the account again")]
    SignInExpired,
    // why: user-fixable input problems (e.g. a schedule time in the past)
    // get their own variant so the message reads as guidance, not a fault.
    #[error("{0}")]
    Invalid(String),
}

// why: Serialize can't be derived here because the wrapped errors (sqlx,
// keyring, io) don't implement it. The official Tauri v2 pattern is a manual
// impl that serializes the Display string, so a failed invoke() rejects with
// a readable message on the frontend.
impl serde::Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_imap_error_with_context() {
        let err = AppError::Imap("login: no".to_string());

        let json = serde_json::to_string(&err).unwrap();

        assert_eq!(json, "\"imap error: login: no\"");
    }

    #[test]
    fn serializes_to_display_string() {
        let err = AppError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing file",
        ));

        let json = serde_json::to_string(&err).unwrap();

        assert_eq!(json, "\"io error: missing file\"");
    }
}
