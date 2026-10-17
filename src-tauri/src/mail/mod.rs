pub mod attachments;
pub mod avatars;
pub mod css;
pub mod draft;
pub mod imap;
pub mod parse;
pub mod quote;
pub mod recipients;
pub mod remote;
pub mod sanitize;
pub mod smtp;
pub mod sync;
pub mod trackers;
pub mod translate;

use std::future::Future;
use std::time::Duration;

use crate::auth::Credential;
use crate::error::AppError;
use crate::models::NewAccount;

/// Prove a credential against both of an account's servers, in parallel,
/// before the account is stored anywhere. The errors already name the
/// failing leg ("imap error: …" / "smtp error: …").
pub async fn verify_servers(account: &NewAccount, credential: &Credential) -> Result<(), AppError> {
    let (imap, smtp) = tokio::join!(
        imap::verify(
            &account.imap_host,
            account.imap_port,
            &account.username,
            credential,
        ),
        smtp::verify(
            &account.smtp_host,
            account.smtp_port,
            &account.username,
            credential,
        ),
    );
    imap?;
    smtp
}

/// Run one piece of mail I/O under a hard time limit.
///
/// why this exists: a TCP connection killed while the Mac slept does not
/// fail — the peer never answers and the read simply never completes. The
/// task then hangs forever, holds whatever slot it claimed, and every later
/// attempt skips because that slot looks busy. The account stops syncing and
/// nothing is ever recorded, because the code that writes the error is on
/// the other side of the await that never returns. A ceiling turns that
/// invisible hang into an ordinary error the caller can record and retry.
pub async fn with_timeout<T>(
    label: &str,
    limit: Duration,
    work: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    tokio::time::timeout(limit, work).await.unwrap_or_else(|_| {
        Err(AppError::Imap(format!(
            "{label} timed out after {}s",
            limit.as_secs()
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn work_that_finishes_in_time_passes_its_value_through() {
        let out = with_timeout("fetch", Duration::from_secs(30), async { Ok(7) })
            .await
            .unwrap();

        assert_eq!(out, 7);
    }

    #[tokio::test]
    async fn work_that_fails_keeps_its_own_error() {
        let err = with_timeout::<()>("fetch", Duration::from_secs(30), async {
            Err(AppError::Imap("login: no".to_string()))
        })
        .await
        .unwrap_err();

        assert_eq!(err.to_string(), "imap error: login: no");
    }

    #[tokio::test]
    async fn work_that_hangs_becomes_an_error_instead_of_hanging() {
        let err = with_timeout::<()>("sync pass", Duration::from_millis(10), async {
            // Stands in for a read on a socket whose peer is gone.
            std::future::pending::<()>().await;
            Ok(())
        })
        .await
        .unwrap_err();

        assert_eq!(err.to_string(), "imap error: sync pass timed out after 0s");
    }
}
