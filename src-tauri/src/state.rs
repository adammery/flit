use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use sqlx::SqlitePool;

use crate::auth::oauth::{self, PendingSignIn, Provider};
use crate::auth::{self, AccessTokens, Credential, PasswordCache};
use crate::error::AppError;
use crate::llm;
use crate::models::{Account, OutgoingMessage};

/// Shared app state managed by Tauri; commands receive it via `tauri::State`.
///
/// why: no Mutex around the pool — SqlitePool is internally synchronized and
/// Clone, so concurrent commands can use it directly.
pub struct AppState {
    pub pool: SqlitePool,
    /// Drafts parked for compose windows that are still opening, keyed by
    /// window label. One-shot: taking a draft removes the entry.
    ///
    /// why std Mutex, not tokio: it is only held for a map insert/remove,
    /// never across an await.
    pending_drafts: Mutex<HashMap<String, OutgoingMessage>>,
    /// Messages sitting out their undo window before actually sending.
    /// One-shot by design: whoever takes the entry owns the outcome — the
    /// timer task sends it, or undo hands it back to a compose window.
    pending_sends: Mutex<HashMap<u64, OutgoingMessage>>,
    /// Session cache of account passwords (see auth::PasswordCache). For
    /// an OAuth account the keychain item — and so this entry — is the
    /// refresh token.
    pub passwords: PasswordCache,
    /// Session cache of OAuth access tokens (see auth::AccessTokens).
    pub access_tokens: AccessTokens,
    /// The browser sign-in waiting for its redirect, so Cancel can end it.
    pub sign_in: PendingSignIn,
    /// Which accounts are syncing their inbox — see try_begin_sync.
    syncing: Mutex<SyncSlots>,
    /// Accounts with background sync work running (the other folders, body
    /// prefetch, header backfill). Separate from `syncing`: that work runs
    /// for minutes and must never block an inbox sync (or vice versa).
    background: Mutex<HashSet<i64>>,
    /// When each account last started a full pass (SyncScope::Everything),
    /// in Unix seconds — what lets a folder switch skip a pass that just ran.
    ///
    /// why wall-clock seconds and not Instant: Instant stands still while
    /// the Mac sleeps, so a pass from before a night's sleep would still look
    /// a few seconds old in the morning, and the first click would sync
    /// nothing.
    last_full_pass: Mutex<HashMap<i64, i64>>,
    /// Per-account queue for background draft pushes (append / delete /
    /// folder sync). tokio's Mutex hands the lock out in FIFO order, so
    /// pushes run in save order — a later save can never reach the server
    /// before the version it is meant to replace.
    draft_pushes: Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>,
    /// Model downloads in flight or failed (see llm::download::Downloads).
    pub llm_downloads: llm::download::Downloads,
    /// The summaries worker (see llm::engine). Idle until the first request.
    pub llm_engine: llm::engine::Engine,
    /// Summaries being written, so a cancel can reach them.
    pub llm_summaries: llm::Summaries,
    /// Where this run saved each attachment (attachment id → file), so the
    /// conversation view can offer Open and Show in Finder for exactly
    /// those files.
    ///
    /// why ids and not paths from the frontend: the webview names an
    /// attachment, never a path, so it can only ever open a file Flit
    /// itself wrote — not anything else on the disk.
    saved_attachments: Mutex<HashMap<i64, PathBuf>>,
}

/// The accounts whose inbox is being synced, and those among them for
/// which another sync was asked meanwhile.
///
/// why one struct behind one Mutex: "was it asked again?" and "let go"
/// must happen as a single step (see SyncSlot::release_or_rerun). With two
/// separate locks, a request landing between the check and the release
/// would be dropped — exactly the lost push this exists to prevent.
#[derive(Default)]
struct SyncSlots {
    running: HashSet<i64>,
    asked_again: HashSet<i64>,
}

/// Proof of holding an account's sync slot. Dropping it releases the slot —
/// RAII, like MutexGuard — so an early return or `?` in the sync path can
/// never leave an account stuck "already syncing" forever.
pub struct SyncSlot<'a> {
    state: &'a AppState,
    account_id: i64,
    /// False once release_or_rerun has let go, so Drop does not release a
    /// second time — by then the slot may already belong to someone else.
    held: bool,
}

impl SyncSlot<'_> {
    /// Let go of the slot — unless a sync of this account was asked for
    /// while this one ran. Then the slot is handed back (`Some`) and the
    /// caller must sync once more: the request came too late to be covered
    /// by the pass that was already under way.
    pub fn release_or_rerun(mut self) -> Option<Self> {
        let mut slots = self.state.lock_syncing();
        if slots.asked_again.remove(&self.account_id) {
            drop(slots);
            return Some(self);
        }
        slots.running.remove(&self.account_id);
        self.held = false;
        None
    }
}

impl Drop for SyncSlot<'_> {
    fn drop(&mut self) {
        if self.held {
            self.state.lock_syncing().running.remove(&self.account_id);
        }
    }
}

/// Proof of holding an account's background slot — same RAII contract as
/// `SyncSlot`.
pub struct BackgroundSlot<'a> {
    state: &'a AppState,
    account_id: i64,
}

impl Drop for BackgroundSlot<'_> {
    fn drop(&mut self) {
        self.state.lock_background().remove(&self.account_id);
    }
}

impl AppState {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            pending_drafts: Mutex::new(HashMap::new()),
            pending_sends: Mutex::new(HashMap::new()),
            passwords: PasswordCache::default(),
            access_tokens: AccessTokens::default(),
            sign_in: PendingSignIn::default(),
            syncing: Mutex::new(SyncSlots::default()),
            background: Mutex::new(HashSet::new()),
            last_full_pass: Mutex::new(HashMap::new()),
            draft_pushes: Mutex::new(HashMap::new()),
            llm_downloads: llm::download::Downloads::default(),
            llm_engine: llm::engine::Engine::default(),
            llm_summaries: llm::Summaries::default(),
            saved_attachments: Mutex::new(HashMap::new()),
        }
    }

    /// Claim the account's sync slot; `None` = a sync of this account is
    /// already running (a push or refresh racing the background poll). The
    /// loser does not run — two parallel passes would double-fire new-mail
    /// notifications — but its request is remembered, and the running pass
    /// goes round once more before it lets go (release_or_rerun).
    pub fn try_begin_sync(&self, account_id: i64) -> Option<SyncSlot<'_>> {
        let mut slots = self.lock_syncing();
        if slots.running.insert(account_id) {
            // A fresh pass covers any request left over from before.
            slots.asked_again.remove(&account_id);
            Some(SyncSlot {
                state: self,
                account_id,
                held: true,
            })
        } else {
            slots.asked_again.insert(account_id);
            None
        }
    }

    /// Claim the account's background slot; `None` = background work for
    /// this account is already running (every sync pass tries to start it —
    /// the running one already covers the work).
    pub fn try_begin_background(&self, account_id: i64) -> Option<BackgroundSlot<'_>> {
        if self.lock_background().insert(account_id) {
            Some(BackgroundSlot {
                state: self,
                account_id,
            })
        } else {
            None
        }
    }

    /// The account's credential: its password, or for an OAuth account an
    /// access token (refreshed when it runs out).
    pub async fn credential(&self, account: &Account) -> Result<Credential, AppError> {
        let Some(provider) = oauth::provider_for(account.auth) else {
            return Ok(Credential::Password(
                self.keychain_secret(account.id).await?,
            ));
        };
        let token = self
            .access_tokens
            .get_or_refresh(account.id, || {
                self.refresh_access_token(account.id, provider)
            })
            .await?;
        Ok(Credential::AccessToken(token))
    }

    /// The account's keychain item — a password, or an OAuth refresh token —
    /// via the session cache: the keychain, and with it a possible macOS ACL
    /// prompt, is consulted at most once per account per app run.
    async fn keychain_secret(&self, account_id: i64) -> Result<String, AppError> {
        self.passwords
            .get_or_fetch(account_id, || auth::get_password(account_id))
            .await
    }

    /// Buy a new access token with the account's refresh token. A provider
    /// that rotates refresh tokens hands back a new one, which replaces the
    /// stored one at once — the old one may stop working.
    async fn refresh_access_token(
        &self,
        account_id: i64,
        provider: Provider,
    ) -> Result<(String, Duration), AppError> {
        let refresh_token = self.keychain_secret(account_id).await?;
        let tokens = provider
            .refresh(&oauth::http_client()?, &refresh_token)
            .await?;
        let lifetime = tokens.lifetime();
        if let Some(rotated) = tokens.refresh_token.filter(|t| *t != refresh_token) {
            auth::set_password(account_id, rotated.clone()).await?;
            self.passwords.insert(account_id, rotated);
        }
        Ok((tokens.access_token, lifetime))
    }

    /// Record that a full pass of the account starts at `now` (Unix secs).
    pub fn note_full_pass(&self, account_id: i64, now: i64) {
        self.lock_last_full_pass().insert(account_id, now);
    }

    /// Whether the account started a full pass less than `window` seconds
    /// before `now`.
    pub fn full_pass_within(&self, account_id: i64, now: i64, window: i64) -> bool {
        self.lock_last_full_pass()
            .get(&account_id)
            .is_some_and(|started| now - started < window)
    }

    /// The account's draft-push queue lock — hold it across the whole
    /// server conversation of one push or discard.
    pub fn draft_push_lock(&self, account_id: i64) -> Arc<tokio::sync::Mutex<()>> {
        self.draft_pushes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(account_id)
            .or_default()
            .clone()
    }

    /// Park a draft for the compose window `label` to pick up after it loads.
    pub fn park_draft(&self, label: &str, draft: OutgoingMessage) {
        self.lock_drafts().insert(label.to_string(), draft);
    }

    /// One-shot pickup: `None` for unknown labels (e.g. a reloaded window
    /// whose draft was already taken).
    pub fn take_draft(&self, label: &str) -> Option<OutgoingMessage> {
        self.lock_drafts().remove(label)
    }

    /// Park a message for the duration of its undo window.
    pub fn park_send(&self, id: u64, message: OutgoingMessage) {
        self.lock_sends().insert(id, message);
    }

    /// One-shot pickup: `None` means the other side already took it — for
    /// the timer that's "undone", for undo that's "too late, already sending".
    pub fn take_send(&self, id: u64) -> Option<OutgoingMessage> {
        self.lock_sends().remove(&id)
    }

    /// Remember where an attachment was just saved; a later save of the
    /// same attachment replaces the earlier spot.
    pub fn note_saved_attachment(&self, attachment_id: i64, path: PathBuf) {
        self.lock_saved_attachments().insert(attachment_id, path);
    }

    /// The file an attachment was last saved to this run, if any.
    pub fn saved_attachment(&self, attachment_id: i64) -> Option<PathBuf> {
        self.lock_saved_attachments().get(&attachment_id).cloned()
    }

    // why unwrap_or_else(into_inner): a poisoned lock only means some thread
    // panicked while holding it — the map itself is still coherent, and a
    // compose draft is not worth failing commands over.
    fn lock_drafts(&self) -> MutexGuard<'_, HashMap<String, OutgoingMessage>> {
        self.pending_drafts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_sends(&self) -> MutexGuard<'_, HashMap<u64, OutgoingMessage>> {
        self.pending_sends
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_syncing(&self) -> MutexGuard<'_, SyncSlots> {
        self.syncing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_last_full_pass(&self) -> MutexGuard<'_, HashMap<i64, i64>> {
        self.last_full_pass
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_background(&self) -> MutexGuard<'_, HashSet<i64>> {
        self.background
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_saved_attachments(&self) -> MutexGuard<'_, HashMap<i64, PathBuf>> {
        self.saved_attachments
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_pool;

    fn draft(subject: &str) -> OutgoingMessage {
        OutgoingMessage {
            account_id: 1,
            alias_id: None,
            to: "alice@example.com".to_string(),
            cc: String::new(),
            bcc: String::new(),
            subject: subject.to_string(),
            body: "hi".to_string(),
            body_html: None,
            attachments: Vec::new(),
            draft_message_id: None,
            in_reply_to: None,
            references: None,
            quote: None,
        }
    }

    async fn account(pool: &SqlitePool, auth: crate::models::AuthKind) -> Account {
        let new = crate::models::NewAccount {
            name: "Jan".to_string(),
            email: "jan@gmail.com".to_string(),
            imap_host: "imap.gmail.com".to_string(),
            imap_port: 993,
            smtp_host: "smtp.gmail.com".to_string(),
            smtp_port: 465,
            username: "jan@gmail.com".to_string(),
        };
        crate::storage::accounts::insert_with_auth(pool, &new, auth)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_password_account_signs_in_with_its_password() {
        let state = AppState::new(test_pool().await);
        let account = account(&state.pool, crate::models::AuthKind::Password).await;
        state.passwords.insert(account.id, "heslo".to_string());

        let credential = state.credential(&account).await.unwrap();

        assert!(matches!(credential, Credential::Password(p) if p == "heslo"));
    }

    #[tokio::test]
    async fn a_google_account_signs_in_with_an_access_token() {
        let state = AppState::new(test_pool().await);
        let account = account(&state.pool, crate::models::AuthKind::Google).await;
        state.passwords.insert(account.id, "1//refresh".to_string());
        // A token still fresh from an earlier refresh — no network needed.
        let cached = || async { Ok(("ya29.cached".to_string(), Duration::from_secs(3600))) };
        state
            .access_tokens
            .get_or_refresh(account.id, cached)
            .await
            .unwrap();

        let credential = state.credential(&account).await.unwrap();

        assert!(matches!(credential, Credential::AccessToken(t) if t == "ya29.cached"));
    }

    #[tokio::test]
    async fn taking_a_draft_is_one_shot() {
        let state = AppState::new(test_pool().await);
        state.park_draft("compose-0", draft("Hello"));

        let taken = state.take_draft("compose-0");

        assert_eq!(taken.map(|d| d.subject), Some("Hello".to_string()));
        assert!(state.take_draft("compose-0").is_none());
    }

    #[tokio::test]
    async fn taking_a_pending_send_is_one_shot() {
        let state = AppState::new(test_pool().await);
        state.park_send(7, draft("Hello"));

        let taken = state.take_send(7);

        assert_eq!(taken.map(|d| d.subject), Some("Hello".to_string()));
        // the second taker loses — that is the whole undo-vs-timer contract
        assert!(state.take_send(7).is_none());
        assert!(state.take_send(8).is_none());
    }

    #[tokio::test]
    async fn sync_slot_blocks_a_second_claim_until_dropped() {
        let state = AppState::new(test_pool().await);

        let slot = state.try_begin_sync(1);
        assert!(slot.is_some());
        // The same account cannot be claimed twice...
        assert!(state.try_begin_sync(1).is_none());
        // ...but another account syncs independently.
        assert!(state.try_begin_sync(2).is_some());

        drop(slot);
        assert!(state.try_begin_sync(1).is_some());
    }

    #[tokio::test]
    async fn a_sync_asked_for_while_one_runs_is_not_lost() {
        let state = AppState::new(test_pool().await);
        let slot = state.try_begin_sync(1).expect("free");
        // A push lands while the inbox is being synced: it cannot start a
        // pass of its own, so the running one has to go round again for it.
        assert!(state.try_begin_sync(1).is_none());

        let slot = slot
            .release_or_rerun()
            .expect("asked again — keep the slot and rerun");
        // Nobody asked during the rerun: now it lets go.
        assert!(slot.release_or_rerun().is_none());
        assert!(state.try_begin_sync(1).is_some());
    }

    #[tokio::test]
    async fn a_released_slot_does_not_free_the_next_holder() {
        let state = AppState::new(test_pool().await);
        let first = state.try_begin_sync(1).expect("free");
        assert!(first.release_or_rerun().is_none());

        let _second = state.try_begin_sync(1).expect("free again");

        // The first slot is gone; letting go must not have been repeated
        // when it was dropped, or it would have freed the second holder.
        assert!(state.try_begin_sync(1).is_none());
    }

    #[tokio::test]
    async fn background_slot_is_independent_of_the_sync_slot() {
        let state = AppState::new(test_pool().await);

        let background = state.try_begin_background(1);
        assert!(background.is_some());
        // Second background work for the same account must not start...
        assert!(state.try_begin_background(1).is_none());
        // ...but a regular sync of the same account still can.
        assert!(state.try_begin_sync(1).is_some());
        assert!(state.try_begin_background(2).is_some());

        drop(background);
        assert!(state.try_begin_background(1).is_some());
    }

    #[tokio::test]
    async fn a_full_pass_counts_as_recent_for_its_window_only() {
        let state = AppState::new(test_pool().await);
        assert!(!state.full_pass_within(1, 1_000, 60));

        state.note_full_pass(1, 1_000);

        assert!(state.full_pass_within(1, 1_059, 60));
        assert!(!state.full_pass_within(1, 1_060, 60));
        // Per account.
        assert!(!state.full_pass_within(2, 1_000, 60));
    }

    #[tokio::test]
    async fn a_saved_attachment_is_remembered_by_id() {
        let state = AppState::new(test_pool().await);
        assert!(state.saved_attachment(4).is_none());

        state.note_saved_attachment(4, PathBuf::from("/tmp/a.pdf"));
        // Saved a second time: the newer file is the one to open.
        state.note_saved_attachment(4, PathBuf::from("/tmp/b.pdf"));

        assert_eq!(state.saved_attachment(4), Some(PathBuf::from("/tmp/b.pdf")));
        assert!(state.saved_attachment(5).is_none());
    }

    #[tokio::test]
    async fn drafts_are_keyed_by_window_label() {
        let state = AppState::new(test_pool().await);
        state.park_draft("compose-0", draft("First"));
        state.park_draft("compose-1", draft("Second"));

        assert!(state.take_draft("compose-9").is_none());
        assert_eq!(
            state.take_draft("compose-1").map(|d| d.subject),
            Some("Second".to_string())
        );
        assert_eq!(
            state.take_draft("compose-0").map(|d| d.subject),
            Some("First".to_string())
        );
    }
}
