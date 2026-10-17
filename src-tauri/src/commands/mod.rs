use std::collections::HashMap;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::auth::Credential;
use crate::error::AppError;
use crate::models::{
    Account, Alias, Appearance, AuthKind, AuthResults, DateTimeFormat, LlmStatus, Mailbox,
    MessageBody, MessageHeader, MessageQuote, MessagesChanged, NewAccount, NotificationSettings,
    OutgoingMessage, RemoteImagePolicy, SearchResults, ShortcutAction, ShortcutBinding, Signature,
    SwipeActions, ThreadOrder,
};
use crate::state::AppState;
use crate::timing;
use crate::{auth, llm, mail, storage};

// why: commands stay thin — validate/orchestrate, call a module, return
// Result. Business logic lives in storage/ and auth/, which are unit-tested.

/// Least time between two progress events during a long backfill.
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Paces the progress events a long backfill emits.
///
/// why this exists: every "messages-changed" makes the window re-run the
/// whole list query — a full scan — plus the folder counts. Emitting one per
/// cached batch tied the UI's refresh rate to the write batch size, so
/// shrinking batches (for the write lock, see storage::WRITE_LOCK) silently
/// multiplied the query load and the app sat above 300% CPU for the length
/// of a backfill. How often we write and how often we redraw are unrelated
/// concerns and must not share a constant.
struct Progress {
    last: std::sync::Mutex<Option<std::time::Instant>>,
}

impl Progress {
    fn new() -> Self {
        Self {
            last: std::sync::Mutex::new(None),
        }
    }

    /// Is an event due? True the first time, then at most once per
    /// `PROGRESS_INTERVAL`.
    ///
    /// why `into_inner` on a poisoned lock: all this guards is a timestamp,
    /// so a panic elsewhere must not silence progress reporting for good.
    fn due(&self, now: std::time::Instant) -> bool {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|t| now.duration_since(t) < PROGRESS_INTERVAL) {
            return false;
        }
        *last = Some(now);
        true
    }
}

#[tauri::command]
pub async fn list_accounts(state: State<'_, AppState>) -> Result<Vec<Account>, AppError> {
    storage::accounts::list(&state.pool).await
}

#[tauri::command]
pub async fn add_account(
    app: AppHandle,
    state: State<'_, AppState>,
    account: NewAccount,
    password: String,
) -> Result<Account, AppError> {
    let inserted = storage::accounts::insert(&state.pool, &account).await?;
    // why: the keychain write can fail (locked keychain, denied prompt) — roll
    // the row back so no account can exist without a stored credential.
    if let Err(err) = auth::set_password(inserted.id, password.clone()).await {
        storage::accounts::delete(&state.pool, inserted.id).await?;
        return Err(err);
    }
    // why: seed the session cache — the first sync then needs no keychain
    // read (and no macOS prompt) at all.
    state.passwords.insert(inserted.id, password);
    // why: broadcast to every window — the settings window mutates accounts,
    // the main window listens and refetches its sidebar list.
    app.emit("accounts-changed", ())?;
    Ok(inserted)
}

/// The OAuth providers this build offers sign-in with; a build without the
/// client ids in its environment offers none.
#[tauri::command]
pub fn oauth_providers() -> Vec<AuthKind> {
    auth::oauth::configured_providers()
}

/// Add an account by signing in with an OAuth provider in the browser. The
/// address comes from the sign-in itself, the servers from the provider;
/// both servers must accept the new token before anything is stored.
#[tauri::command]
pub async fn add_oauth_account(
    app: AppHandle,
    state: State<'_, AppState>,
    provider: AuthKind,
    name: String,
) -> Result<Account, AppError> {
    let provider = auth::oauth::provider_for(provider)
        .ok_or_else(|| AppError::Invalid("not an OAuth provider".to_string()))?;
    let tokens = auth::oauth::sign_in(
        provider,
        &auth::oauth::http_client()?,
        None,
        open_in_browser,
        state.sign_in.begin(),
    )
    .await?;
    let lifetime = tokens.lifetime();
    let email = auth::oauth::signed_in_address(&tokens, None)?;
    let refresh_token = tokens
        .refresh_token
        .ok_or_else(|| AppError::OAuth("the provider issued no refresh token".to_string()))?;
    let account = NewAccount {
        name: match name.trim() {
            "" => email.clone(),
            name => name.to_string(),
        },
        email: email.clone(),
        imap_host: provider.imap_host.to_string(),
        imap_port: provider.imap_port,
        smtp_host: provider.smtp_host.to_string(),
        smtp_port: provider.smtp_port,
        username: email,
    };
    let access_token = tokens.access_token;
    mail::verify_servers(&account, &Credential::AccessToken(access_token.clone())).await?;

    let inserted =
        storage::accounts::insert_with_auth(&state.pool, &account, provider.kind).await?;
    // why: as in add_account — no account may exist without its credential.
    if let Err(err) = auth::set_password(inserted.id, refresh_token.clone()).await {
        storage::accounts::delete(&state.pool, inserted.id).await?;
        return Err(err);
    }
    state.passwords.insert(inserted.id, refresh_token);
    state
        .access_tokens
        .insert(inserted.id, access_token, lifetime);
    app.emit("accounts-changed", ())?;
    Ok(inserted)
}

/// Sign an OAuth account in again — after its sign-in expired or was
/// revoked. It must be the same mailbox; the new tokens replace the old
/// ones once both servers accept them, and the account's error clears.
#[tauri::command]
pub async fn reconnect_account(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), AppError> {
    let account = storage::accounts::get(&state.pool, id).await?;
    let provider = auth::oauth::provider_for(account.auth)
        .ok_or_else(|| AppError::Invalid("this account signs in with a password".to_string()))?;
    let tokens = auth::oauth::sign_in(
        provider,
        &auth::oauth::http_client()?,
        Some(&account.email),
        open_in_browser,
        state.sign_in.begin(),
    )
    .await?;
    auth::oauth::signed_in_address(&tokens, Some(&account.email))?;
    let lifetime = tokens.lifetime();
    let refresh_token = tokens
        .refresh_token
        .ok_or_else(|| AppError::OAuth("the provider issued no refresh token".to_string()))?;
    let access_token = tokens.access_token;
    let credential = Credential::AccessToken(access_token.clone());
    // why IMAP only: the servers were proven when the account was added; the
    // login here proves the new token, which SMTP accepts all the same.
    mail::imap::verify(
        &account.imap_host,
        account.imap_port,
        &account.username,
        &credential,
    )
    .await?;

    auth::set_password(id, refresh_token.clone()).await?;
    state.passwords.insert(id, refresh_token);
    state.access_tokens.insert(id, access_token, lifetime);
    storage::accounts::set_status(&state.pool, id, None, now_epoch()).await?;
    app.emit("accounts-changed", ())?;
    Ok(())
}

/// End the browser sign-in still waiting, if any (the dialog's Cancel).
#[tauri::command]
pub fn cancel_oauth_sign_in(state: State<'_, AppState>) {
    state.sign_in.cancel();
}

/// The sign-in page opens in the user's default browser — where they are
/// already signed in to the provider, and where Flit never sees a password.
fn open_in_browser(url: &str) -> Result<(), AppError> {
    tauri_plugin_opener::open_url(url, None::<&str>)
        .map_err(|e| AppError::OAuth(format!("could not open the browser: {e}")))
}

/// For an OAuth account about to be deleted: revoke its refresh token at
/// the provider, in the background. Best effort — deleting must work
/// offline too, and a grant left behind only lingers in the user's
/// provider account, where they can remove it themselves.
async fn revoke_sign_in(state: &AppState, id: i64) {
    let Ok(account) = storage::accounts::get(&state.pool, id).await else {
        return;
    };
    let Some(provider) = auth::oauth::provider_for(account.auth) else {
        return;
    };
    let Ok(refresh_token) = state
        .passwords
        .get_or_fetch(id, || auth::get_password(id))
        .await
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let revoked = match auth::oauth::http_client() {
            Ok(http) => provider.revoke(&http, &refresh_token).await,
            Err(err) => Err(err),
        };
        if let Err(err) = revoked {
            eprintln!("account {id}: {err}");
        }
    });
}

/// Set (or clear) an account's accent color, then broadcast so the main
/// window re-tints its sidebar and message dots.
#[tauri::command]
pub async fn set_account_color(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    color: Option<String>,
) -> Result<(), AppError> {
    storage::accounts::set_color(&state.pool, id, color.as_deref()).await?;
    app.emit("accounts-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_account(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), AppError> {
    revoke_sign_in(&state, id).await;
    // why: keychain first — if it fails the account stays intact; the reverse
    // order could strand a secret in the keychain with no owning account row.
    auth::delete_password(id).await?;
    state.passwords.remove(id);
    state.access_tokens.forget(id);
    storage::accounts::delete(&state.pool, id).await?;
    app.emit("accounts-changed", ())?;
    Ok(())
}

/// Sync one account into the local cache: folder list + every folder's
/// headers.
#[tauri::command]
pub async fn sync_account(app: AppHandle, account_id: i64) -> Result<(), AppError> {
    // why: every folder switch asks for this, and a full pass is 0.2-10 s
    // of network and CPU (measured) — clicking through three folders ran
    // three. A minute after one started, the account is fresh enough to
    // browse; push keeps the inbox current, and "Check for new mail"
    // (refresh_account) always runs.
    let recent = app.state::<AppState>().full_pass_within(
        account_id,
        now_epoch(),
        FOLDER_SWITCH_SYNC_WINDOW_SECS,
    );
    if recent {
        return Ok(());
    }
    run_sync(&app, account_id, SyncScope::Everything).await
}

/// The user's explicit "check for new mail". Same pass as `sync_account`,
/// but every folder reconciles end to end instead of just its newest slice,
/// so a change too old for the windowed sweep shows up on demand rather
/// than at the next hourly full sweep.
///
/// why a separate command: `sync_account` also fires on every folder
/// switch, and a full sweep of every folder is far too heavy for that.
#[tauri::command]
pub async fn refresh_account(app: AppHandle, account_id: i64) -> Result<(), AppError> {
    let pool = app.state::<AppState>().pool.clone();
    storage::mailboxes::clear_full_sweeps(&pool, account_id).await?;
    run_sync(&app, account_id, SyncScope::Everything).await
}

/// Open one IMAP session for an account.
async fn connect_account(
    account: &Account,
    credential: &Credential,
) -> Result<mail::imap::ImapSession, AppError> {
    mail::imap::connect(
        &account.imap_host,
        account.imap_port,
        &account.username,
        credential,
    )
    .await
}

const SYNC_LABEL: &str = "sync pass";
/// Ceiling for one sync pass. Generous — a first pass over a large account
/// does real work — but finite, because the pass holds the account's sync
/// slot: a hung one would make every later pass skip, silently, forever.
const SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Ceiling for the background prefetch and backfill. Much larger than a sync
/// pass — mirroring a big mailbox legitimately runs for many minutes — but
/// still finite: the backfill holds its own slot, and both resume where they
/// left off on the next pass, so cutting one short costs nothing but time.
const BACKGROUND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// How long after a full pass a folder switch skips starting another.
const FOLDER_SWITCH_SYNC_WINDOW_SECS: i64 = 60;

/// How much of an account one sync pass covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncScope {
    /// The inbox, then the other folders, the body prefetch and the header
    /// backfill — what the poller, a folder switch and a refresh ask for.
    Everything,
    /// Just the inbox: what an IDLE push announces. Under push, the poller's
    /// interval is the cadence for the folders IDLE can't watch, so a push
    /// needn't redo them. Measured 2026-09-27: every push — often only the
    /// echo of the user's own read flag or archive — cost a full pass over
    /// ~26 folders, 5–10 s of network and CPU for nothing new.
    InboxOnly,
}

/// The sync pass behind the command, callable from background tasks (the
/// poller) that have an AppHandle but no `State` extractor.
pub(crate) async fn run_sync(
    app: &AppHandle,
    account_id: i64,
    scope: SyncScope,
) -> Result<(), AppError> {
    // why the whole pass runs in one spawned task while the caller waits only
    // for the inbox stage: the sync and background slots borrow AppState, so
    // they cannot be handed across a spawn — every stage lives in one task,
    // and the oneshot still lets "check for new mail" report done as soon as
    // the inbox has landed, instead of after all 26 folders.
    if scope == SyncScope::Everything {
        app.state::<AppState>()
            .note_full_pass(account_id, now_epoch());
    }
    let (report, inbox_done) = tokio::sync::oneshot::channel();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        run_pass(&app, account_id, scope, report).await;
    });
    // why Ok on a dropped sender: the task only drops it by panicking, and a
    // failed pass has already recorded its own status for settings to show.
    inbox_done.await.unwrap_or(Ok(()))
}

/// One whole sync pass. `report` is answered as soon as the inbox stage is
/// finished; everything after it is background work the user never waits on.
async fn run_pass(
    app: &AppHandle,
    account_id: i64,
    scope: SyncScope,
    report: tokio::sync::oneshot::Sender<Result<(), AppError>>,
) {
    if let Err(err) = run_pass_inner(app, account_id, scope, report).await {
        eprintln!("sync pass failed for account {account_id}: {err}");
    }
}

async fn run_pass_inner(
    app: &AppHandle,
    account_id: i64,
    scope: SyncScope,
    report: tokio::sync::oneshot::Sender<Result<(), AppError>>,
) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    // why: held (RAII) through the inbox stage — a push or refresh racing
    // the background poll must not sync the inbox twice at once or
    // double-fire notifications. The loser does not run; its request is
    // remembered and this pass reruns the inbox for it (release_or_rerun).
    let Some(slot) = state.try_begin_sync(account_id) else {
        let _ = report.send(Ok(()));
        return Ok(());
    };
    let account = storage::accounts::get(&state.pool, account_id).await?;
    // why: the credential comes from the session cache (one keychain read per
    // account per run) and never leaves this block — never written to state
    // beyond the cache, events, or logs. The open session outlives it: the
    // background work below continues on the very same connection.
    let result = mail::with_timeout(SYNC_LABEL, SYNC_TIMEOUT, async {
        let credential = state.credential(&account).await?;
        let mut session = connect_account(&account, &credential).await?;
        let new_mail = mail::sync::sync_inbox(&state.pool, &account, &mut session).await?;
        Ok::<(mail::imap::ImapSession, Vec<mail::sync::NewMail>), AppError>((session, new_mail))
    })
    .await;

    // why: every sync doubles as a health check — the recorded outcome is
    // what settings shows when credentials go stale server-side.
    let error_text = result.as_ref().err().map(ToString::to_string);
    storage::accounts::set_status(&state.pool, account_id, error_text.as_deref(), now_epoch())
        .await?;
    app.emit("accounts-changed", ())?;

    let (session, new_mail) = match result {
        Ok(pair) => pair,
        // why hand the error over rather than just returning: the caller is
        // blocked on the inbox stage, and a failed check must surface as a
        // failure rather than as a silent success.
        Err(err) => {
            let _ = report.send(Err(err));
            return Ok(());
        }
    };
    app.emit("messages-changed", MessagesChanged::reload(account_id))?;

    // why: after the emit — banners are cosmetic, the fresh list is not.
    // Settings are re-read per sync so a toggle applies to the next pass.
    let defaults = storage::settings::notification_settings(&state.pool).await?;
    crate::notify::show(app, &crate::notify::plan(&account, &defaults, &new_mail));

    // The inbox is in and on screen — whoever asked for a check has their
    // answer.
    let _ = report.send(Ok(()));

    // why rerun before letting go: a sync asked for while this one ran (a
    // push landing mid-stage) was refused a pass of its own, and the mail it
    // announced may have arrived after this pass looked. Until 2026-09 that
    // request was simply dropped and the mail waited for the next poll —
    // half an hour on push setups. Go round again on the same connection
    // until nobody asked; the check and the release are one locked step.
    let mut session = session;
    let mut slot = slot;
    loop {
        match slot.release_or_rerun() {
            None => break,
            Some(held) => {
                slot = held;
                let again = mail::with_timeout(
                    SYNC_LABEL,
                    SYNC_TIMEOUT,
                    mail::sync::sync_inbox(&state.pool, &account, &mut session),
                )
                .await;
                match again {
                    Ok(new_mail) => {
                        app.emit("messages-changed", MessagesChanged::reload(account_id))?;
                        let plan = crate::notify::plan(&account, &defaults, &new_mail);
                        crate::notify::show(app, &plan);
                    }
                    Err(err) => {
                        // A failed command leaves the session mid-stream;
                        // stop here. The slot is released on the way out.
                        eprintln!("inbox resync failed for account {account_id}: {err}");
                        return Ok(());
                    }
                }
            }
        }
    }

    if scope == SyncScope::InboxOnly {
        let _ = session.logout().await;
        return Ok(());
    }

    // The sync slot is free again, so a push from here on gets its own
    // inbox pass at once instead of waiting out the minutes below. The rest
    // of the work runs under the background slot; a concurrent pass finds
    // it taken and leaves that work to this one.
    let Some(_background) = state.try_begin_background(account_id) else {
        let _ = session.logout().await;
        return Ok(());
    };

    let pool = state.pool.clone();
    let app = app.clone();
    {
        // The rest of the account's folders. Measured at 30.2 s across 26
        // folders, which is why the user is no longer kept waiting for it.
        let others = mail::with_timeout(
            "other folders",
            BACKGROUND_TIMEOUT,
            mail::sync::sync_other_folders(&pool, &account, &mut session),
        );
        match others.await {
            Ok(()) => {
                let _ = app.emit("messages-changed", MessagesChanged::reload(account_id));
            }
            Err(err) => {
                eprintln!("folder sync failed for account {account_id}: {err}");
                // Same reasoning as the prefetch below: a timed-out command
                // leaves the session mid-stream, so drop it rather than carry
                // on over a desynced protocol.
                return Ok(());
            }
        }

        // why: bodies download after the list is already usable, and each
        // cached body feeds the FTS index so search covers unopened mail.
        let prefetch = mail::with_timeout(
            "body prefetch",
            BACKGROUND_TIMEOUT,
            mail::sync::prefetch_bodies(&pool, &account, &mut session),
        );
        match prefetch.await {
            // why: snippets just became real — lists and searches should see them.
            Ok(cached) if cached > 0 => {
                let _ = app.emit("messages-changed", MessagesChanged::reload(account_id));
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("body prefetch failed for account {account_id}: {err}");
                // why return: a failed or timed-out command leaves the shared
                // session mid-stream, and the reply it never read would be
                // mistaken for the answer to the next one. Drop it — the next
                // pass reconnects — rather than backfilling over a desynced
                // protocol.
                return Ok(());
            }
        }

        // Header backfill: mirror the rest of every folder so the whole
        // mailbox is eventually local. Runs after the body prefetch, on the
        // same connection, so the two never talk to the server at once. The
        // background slot makes a refresh mid-backfill a no-op instead of a
        // second loop.
        // Each cached batch refreshes the list (and its progress line) live —
        // but no faster than Progress allows.
        let progress = Progress::new();
        let on_batch = || {
            if progress.due(std::time::Instant::now()) {
                let _ = app.emit("messages-changed", MessagesChanged::reload(account_id));
            }
        };
        let backfill = mail::with_timeout(
            "header backfill",
            BACKGROUND_TIMEOUT,
            mail::sync::backfill_headers(&pool, &account, &mut session, on_batch),
        );
        match backfill.await {
            Ok(_) => {
                // why a final event: the throttle above can swallow the last
                // batch's notice, and the newest headers would then sit in
                // the cache unshown until something else refreshed.
                let _ = app.emit("messages-changed", MessagesChanged::reload(account_id));
                let _ = session.logout().await;
            }
            Err(err) => eprintln!("header backfill failed for account {account_id}: {err}"),
        }
    }
    Ok(())
}

/// Mark one message read/unread. Updates the local cache and notifies the UI
/// immediately, then pushes the `\Seen` flag to the server in the background —
/// opening a message never waits on the network for the dot to clear.
#[tauri::command]
pub async fn set_message_read(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    read: bool,
) -> Result<(), AppError> {
    let loc = storage::messages::location(&state.pool, message_id).await?;
    storage::messages::set_read(&state.pool, message_id, read).await?;
    // why the ids rather than a plain reload: the window patches this row in
    // place instead of re-running the whole list query for one dot.
    app.emit(
        "messages-changed",
        MessagesChanged::read(loc.account_id, vec![message_id], read),
    )?;

    // why: fetch account + credential on the command path (cheap, from the
    // session cache) so the spawned task owns everything it needs.
    let account = storage::accounts::get(&state.pool, loc.account_id).await?;
    let credential = state.credential(&account).await?;
    tauri::async_runtime::spawn(async move {
        // why: best effort — if the server STORE fails, the next sync's
        // reconcile adopts the server's flag, so nothing drifts permanently.
        if let Err(err) = push_seen_flag(&account, &credential, &loc.mailbox, loc.uid, read).await {
            eprintln!("failed to push read={read} for message {message_id}: {err}");
        }
    });
    Ok(())
}

/// Mark a whole selection read or unread. Same shape as set_message_read —
/// the cache and the UI update at once, the server catches up in the
/// background — but batched, so each account folder is one STORE.
#[tauri::command]
pub async fn set_messages_read(
    app: AppHandle,
    state: State<'_, AppState>,
    message_ids: Vec<i64>,
    read: bool,
    threads: bool,
) -> Result<(), AppError> {
    let batches = locate_batches(&state, &message_ids, threads).await?;
    for batch in &batch_ids(&batches) {
        storage::messages::set_read(&state.pool, *batch, read).await?;
    }
    for batch in &batches {
        let ids: Vec<i64> = batch.rows.iter().map(|(id, _)| *id).collect();
        app.emit(
            "messages-changed",
            MessagesChanged::read(batch.account_id, ids, read),
        )?;
    }

    for batch in batches {
        let account = storage::accounts::get(&state.pool, batch.account_id).await?;
        let credential = state.credential(&account).await?;
        let uids: Vec<i64> = batch.rows.iter().map(|(_, uid)| *uid).collect();
        tauri::async_runtime::spawn(async move {
            // why: best effort, like the single-message path — a failed STORE
            // is adopted back from the server by the next sync's reconcile.
            if let Err(err) =
                push_seen_flags(&account, &credential, &batch.mailbox, &uids, read).await
            {
                eprintln!("failed to push read={read} for {} rows: {err}", uids.len());
            }
        });
    }
    Ok(())
}

/// Every message row id across a set of batches.
fn batch_ids(batches: &[FolderBatch]) -> Vec<i64> {
    batches
        .iter()
        .flat_map(|batch| batch.rows.iter().map(|(id, _)| *id))
        .collect()
}

/// Move one message to the account's Trash folder, then drop it from the
/// local cache.
#[tauri::command]
pub async fn move_to_trash(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
) -> Result<(), AppError> {
    move_to_special_folder(&app, &state, message_id, &["trash"], "trash").await
}

/// Archive one message: move it to the account's Archive folder, then drop it
/// from the local cache. Same server-confirmed path as trashing.
///
/// why the "all" fallback: Gmail has no \Archive folder — archiving means
/// removing the Inbox label, which over IMAP is a move into "All Mail" (\All).
#[tauri::command]
pub async fn archive_message(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
) -> Result<(), AppError> {
    move_to_special_folder(&app, &state, message_id, &["archive", "all"], "archive").await
}

/// Move one message to a user-chosen folder of its account, then drop it
/// from the local cache. The destination must be a folder discovery has
/// mirrored — an unknown name fails before any IMAP command runs.
#[tauri::command]
pub async fn move_message(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    mailbox: String,
) -> Result<(), AppError> {
    let loc = storage::messages::location(&state.pool, message_id).await?;
    if !storage::mailboxes::exists(&state.pool, loc.account_id, &mailbox).await? {
        return Err(AppError::Imap(format!("no folder named {mailbox}")));
    }
    let rows = vec![(message_id, loc.uid)];
    move_rows_to_mailbox(&app, &state, loc.account_id, &loc.mailbox, &rows, &mailbox).await
}

/// One IMAP session's worth of work: the rows of a single account's folder.
#[derive(Debug, PartialEq, Eq)]
struct FolderBatch {
    account_id: i64,
    mailbox: String,
    /// `(message row id, server uid)` — what move_rows_to_mailbox takes.
    rows: Vec<(i64, i64)>,
}

/// Split located messages so each batch is one account's folder.
///
/// why: a bulk action over a unified-inbox selection can span accounts and
/// folders, and a MOVE only applies to the mailbox that is SELECTed. Grouping
/// first means one connection per folder instead of one per message.
///
/// Batch order is (account, folder name) and rows keep their input order, so
/// a run is reproducible and a partial failure is explainable. A row named
/// more than once — conversation expansion can do that — is kept once.
fn group_by_folder(located: Vec<(i64, storage::messages::MessageLocation)>) -> Vec<FolderBatch> {
    let mut batches: std::collections::BTreeMap<(i64, String), Vec<(i64, i64)>> =
        std::collections::BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    for (message_id, loc) in located {
        if !seen.insert(message_id) {
            continue;
        }
        batches
            .entry((loc.account_id, loc.mailbox))
            .or_default()
            .push((message_id, loc.uid));
    }
    batches
        .into_iter()
        .map(|((account_id, mailbox), rows)| FolderBatch {
            account_id,
            mailbox,
            rows,
        })
        .collect()
}

/// Which failure a bulk run reports: the one it hit first.
///
/// why not the last, and why not abort: every batch runs, so one account
/// with a stale password cannot strand the rows of the others — and the
/// error worth showing is where the run first went wrong.
fn first_error(current: Result<(), AppError>, next: Result<(), AppError>) -> Result<(), AppError> {
    match current {
        Err(err) => Err(err),
        Ok(()) => next,
    }
}

/// `first_error` over a whole run of batch outcomes, in batch order — so a
/// selection that ran concurrently still reports the same error a
/// sequential loop would have.
fn first_error_of(outcomes: Vec<Result<(), AppError>>) -> Result<(), AppError> {
    outcomes.into_iter().fold(Ok(()), first_error)
}

/// Look every selected id up, then group them into per-folder batches.
///
/// `threads` mirrors what the row on screen stands for: in the conversation
/// views a row is a whole thread, so the action has to reach every member in
/// that folder, exactly as the single-row thread commands do. The flat
/// trash/junk/drafts views pass false — there a row is one message, and
/// expanding it would drag in siblings the user never selected.
async fn locate_batches(
    state: &AppState,
    message_ids: &[i64],
    threads: bool,
) -> Result<Vec<FolderBatch>, AppError> {
    let mut located = Vec::with_capacity(message_ids.len());
    for id in message_ids {
        let loc = storage::messages::location(&state.pool, *id).await?;
        if !threads {
            located.push((*id, loc));
            continue;
        }
        for (row_id, uid) in storage::messages::thread_rows_in_mailbox(&state.pool, *id).await? {
            located.push((
                row_id,
                storage::messages::MessageLocation {
                    account_id: loc.account_id,
                    mailbox: loc.mailbox.clone(),
                    uid,
                },
            ));
        }
    }
    Ok(group_by_folder(located))
}

/// Move a whole selection to a user-chosen folder — one server session per
/// account folder the selection touches.
///
/// why it keeps going after a failure: a selection can span accounts, and one
/// stale password must not strand the rows of every other account. The first
/// error is still returned, so the frontend can put the failed rows back.
#[tauri::command]
pub async fn move_messages(
    app: AppHandle,
    state: State<'_, AppState>,
    message_ids: Vec<i64>,
    mailbox: String,
    threads: bool,
) -> Result<(), AppError> {
    let batches = locate_batches(&state, &message_ids, threads).await?;
    // why every account is checked before any move: the batches run
    // together below, so a folder found missing halfway could no longer
    // stop the earlier ones — refusing up front keeps "no folder" from
    // moving anything at all.
    for batch in &batches {
        if !storage::mailboxes::exists(&state.pool, batch.account_id, &mailbox).await? {
            return Err(AppError::Imap(format!("no folder named {mailbox}")));
        }
    }
    // why join_all: each batch is one account folder on its own IMAP
    // connection, so run in sequence they only ever waited on each other's
    // round trips. Each batch still reconciles its own cache only after
    // its server confirms, exactly as before; join_all keeps the outcomes
    // in batch order, so the first error reported is the same one.
    let outcomes = futures::future::join_all(batches.iter().map(|batch| {
        move_rows_to_mailbox(
            &app,
            &state,
            batch.account_id,
            &batch.mailbox,
            &batch.rows,
            &mailbox,
        )
    }))
    .await;
    first_error_of(outcomes)
}

/// The bulk counterpart of `move_to_special_folder`: each batch resolves the
/// destination against its own account, because "archive" and "trash" are
/// per-account folder names, not one shared string.
async fn move_batches_to_special_folder(
    app: &AppHandle,
    state: &AppState,
    message_ids: &[i64],
    threads: bool,
    roles: &[&str],
    label: &str,
) -> Result<(), AppError> {
    let batches = locate_batches(state, message_ids, threads).await?;
    // why the destinations are resolved first: same as move_messages — an
    // account without the folder must refuse the whole selection, not
    // whatever batches happened to finish before it was noticed.
    let mut dests = Vec::with_capacity(batches.len());
    for batch in &batches {
        dests.push(special_folder_dest(state, batch.account_id, roles, label).await?);
    }
    let outcomes = futures::future::join_all(batches.iter().zip(&dests).map(|(batch, dest)| {
        move_rows_to_mailbox(
            app,
            state,
            batch.account_id,
            &batch.mailbox,
            &batch.rows,
            dest,
        )
    }))
    .await;
    first_error_of(outcomes)
}

/// Trash a whole selection — each account's own Trash folder.
#[tauri::command]
pub async fn trash_messages(
    app: AppHandle,
    state: State<'_, AppState>,
    message_ids: Vec<i64>,
    threads: bool,
) -> Result<(), AppError> {
    move_batches_to_special_folder(&app, &state, &message_ids, threads, &["trash"], "trash").await
}

/// Archive a whole selection — each account's own Archive (Gmail: All Mail).
#[tauri::command]
pub async fn archive_messages(
    app: AppHandle,
    state: State<'_, AppState>,
    message_ids: Vec<i64>,
    threads: bool,
) -> Result<(), AppError> {
    move_batches_to_special_folder(
        &app,
        &state,
        &message_ids,
        threads,
        &["archive", "all"],
        "archive",
    )
    .await
}

/// Trash a whole conversation as shown in its folder — every thread member
/// sharing the anchor's mailbox. Flat views (trash/junk/drafts) keep using
/// the single-message commands; the frontend picks per row.
#[tauri::command]
pub async fn trash_thread(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
) -> Result<(), AppError> {
    move_thread_to_special_folder(&app, &state, message_id, &["trash"], "trash").await
}

/// Archive a whole conversation as shown in its folder (Gmail: All Mail).
#[tauri::command]
pub async fn archive_thread(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
) -> Result<(), AppError> {
    move_thread_to_special_folder(&app, &state, message_id, &["archive", "all"], "archive").await
}

/// Move a whole conversation (as shown in the anchor's folder) to a
/// user-chosen folder of its account.
#[tauri::command]
pub async fn move_thread(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    mailbox: String,
) -> Result<(), AppError> {
    let loc = storage::messages::location(&state.pool, message_id).await?;
    if !storage::mailboxes::exists(&state.pool, loc.account_id, &mailbox).await? {
        return Err(AppError::Imap(format!("no folder named {mailbox}")));
    }
    let rows = storage::messages::thread_rows_in_mailbox(&state.pool, message_id).await?;
    move_rows_to_mailbox(&app, &state, loc.account_id, &loc.mailbox, &rows, &mailbox).await
}

/// Move a message to the account's folder for the first matching special-use
/// `role`, then drop it from the local cache. Unlike the read flag this waits
/// on the server — the row must not vanish from the list if the move failed.
async fn move_to_special_folder(
    app: &AppHandle,
    state: &AppState,
    message_id: i64,
    roles: &[&str],
    label: &str,
) -> Result<(), AppError> {
    let loc = storage::messages::location(&state.pool, message_id).await?;
    let dest = special_folder_dest(state, loc.account_id, roles, label).await?;
    let rows = vec![(message_id, loc.uid)];
    move_rows_to_mailbox(app, state, loc.account_id, &loc.mailbox, &rows, &dest).await
}

/// Like `move_to_special_folder`, but for the anchor's whole conversation
/// as shown in its folder — every thread member sharing the mailbox moves
/// in one server session.
async fn move_thread_to_special_folder(
    app: &AppHandle,
    state: &AppState,
    message_id: i64,
    roles: &[&str],
    label: &str,
) -> Result<(), AppError> {
    let loc = storage::messages::location(&state.pool, message_id).await?;
    let dest = special_folder_dest(state, loc.account_id, roles, label).await?;
    let rows = storage::messages::thread_rows_in_mailbox(&state.pool, message_id).await?;
    move_rows_to_mailbox(app, state, loc.account_id, &loc.mailbox, &rows, &dest).await
}

/// The account's folder name for the first matching special-use role.
async fn special_folder_dest(
    state: &AppState,
    account_id: i64,
    roles: &[&str],
    label: &str,
) -> Result<String, AppError> {
    for role in roles {
        if let Some(name) = storage::mailboxes::name_for_role(&state.pool, account_id, role).await?
        {
            return Ok(name);
        }
    }
    Err(AppError::Imap(format!("no {label} folder discovered yet")))
}

/// The one server-confirmed move path: select the source folder, UID MOVE
/// each `(row id, uid)` into `dest`, and drop cached rows only for confirmed
/// moves — a row must not vanish from the list if its move failed. A failure
/// mid-batch keeps the earlier moves (they already happened server-side) and
/// surfaces the error.
async fn move_rows_to_mailbox(
    app: &AppHandle,
    state: &AppState,
    account_id: i64,
    source: &str,
    rows: &[(i64, i64)],
    dest: &str,
) -> Result<(), AppError> {
    let _t = timing::start("cmd::move_rows_to_mailbox");
    // why: moving a message into the folder it already lives in is a no-op
    // (and some servers error on it) — just leave it be.
    if source == dest || rows.is_empty() {
        return Ok(());
    }

    let account = storage::accounts::get(&state.pool, account_id).await?;
    let credential = state.credential(&account).await?;
    let mut session = mail::imap::connect(
        &account.imap_host,
        account.imap_port,
        &account.username,
        &credential,
    )
    .await?;
    session
        .select(source)
        .await
        .map_err(|e| AppError::Imap(format!("select {source}: {e}")))?;
    let mut outcome = Ok(());
    let mut moved: Vec<i64> = Vec::new();
    for (row_id, uid) in rows {
        match mail::imap::move_message(&mut session, *uid, dest).await {
            Ok(()) => moved.push(*row_id),
            Err(err) => {
                outcome = Err(err);
                break;
            }
        }
    }
    let _ = session.logout().await;

    // why: only after the server confirms — each confirmed message left the
    // source folder, so its cached row (and images, via cascade) go too.
    let emit = !moved.is_empty();
    for row_id in moved {
        storage::messages::delete_by_id(&state.pool, row_id).await?;
    }
    if emit {
        app.emit("messages-changed", MessagesChanged::reload(account_id))?;
    }
    outcome
}

/// Connect, select the folder, flip the `\Seen` flag, log out.
async fn push_seen_flag(
    account: &Account,
    credential: &Credential,
    mailbox: &str,
    uid: i64,
    seen: bool,
) -> Result<(), AppError> {
    push_seen_flags(account, credential, mailbox, &[uid], seen).await
}

/// One connection, one SELECT, one STORE over the whole set.
async fn push_seen_flags(
    account: &Account,
    credential: &Credential,
    mailbox: &str,
    uids: &[i64],
    seen: bool,
) -> Result<(), AppError> {
    let _t = timing::start("cmd::push_seen_flags");
    let mut session = mail::imap::connect(
        &account.imap_host,
        account.imap_port,
        &account.username,
        credential,
    )
    .await?;
    session
        .select(mailbox)
        .await
        .map_err(|e| AppError::Imap(format!("select {mailbox}: {e}")))?;
    let result = mail::imap::set_seen_many(&mut session, uids, seen).await;
    let _ = session.logout().await;
    result
}

pub(crate) fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Message list for one folder view: threaded (one row per conversation)
/// everywhere except trash/junk/drafts, which stay flat — an action there
/// must touch exactly one message, never its whole conversation.
#[tauri::command]
pub async fn list_messages(
    state: State<'_, AppState>,
    account_id: Option<i64>,
    mailbox: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<MessageHeader>, AppError> {
    let mailbox = mailbox.as_deref().unwrap_or("INBOX");
    let flat = match account_id {
        Some(id) => matches!(
            storage::mailboxes::role_of(&state.pool, id, mailbox)
                .await?
                .as_deref(),
            Some("trash" | "junk" | "drafts")
        ),
        // why: the unified view only ever shows INBOX folders.
        None => false,
    };
    if flat {
        storage::messages::list(&state.pool, account_id, mailbox, limit).await
    } else {
        storage::messages::list_threaded(&state.pool, account_id, mailbox, limit).await
    }
}

/// Counts for the current list view: header totals plus the backfill
/// progress pair (cached vs. server total). Threading mirrors
/// list_messages' rule so `list_rows` matches what the list shows.
#[tauri::command]
pub async fn view_status(
    state: State<'_, AppState>,
    account_id: Option<i64>,
    mailbox: Option<String>,
) -> Result<crate::models::ViewStatus, AppError> {
    let mailbox = mailbox.as_deref().unwrap_or("INBOX");
    let flat = match account_id {
        Some(id) => matches!(
            storage::mailboxes::role_of(&state.pool, id, mailbox)
                .await?
                .as_deref(),
            Some("trash" | "junk" | "drafts")
        ),
        // why: the unified view only ever shows INBOX folders.
        None => false,
    };
    storage::messages::view_status(&state.pool, account_id, mailbox, !flat).await
}

/// The full conversation of one message (all folders except
/// trash/junk/drafts), oldest first — what the conversation view renders.
#[tauri::command]
pub async fn list_thread(
    state: State<'_, AppState>,
    message_id: i64,
) -> Result<Vec<MessageHeader>, AppError> {
    storage::messages::thread_of(&state.pool, message_id).await
}

/// Autocomplete for compose recipient fields: locally harvested contacts
/// matching `query`, best first. Never touches the network.
#[tauri::command]
pub async fn list_contacts(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<crate::models::Contact>, AppError> {
    storage::contacts::suggest(&state.pool, &query).await
}

/// Folders of one account in sidebar order, from the local mirror.
#[tauri::command]
pub async fn list_mailboxes(
    state: State<'_, AppState>,
    account_id: i64,
) -> Result<Vec<Mailbox>, AppError> {
    storage::mailboxes::list(&state.pool, account_id).await
}

/// Search the local cache with a gmail-style query ("from:x is:unread text").
/// Purely local — never talks to the server.
#[tauri::command]
pub async fn search_messages(
    state: State<'_, AppState>,
    account_id: Option<i64>,
    query: String,
) -> Result<SearchResults, AppError> {
    let parsed = storage::search::parse_query(&query);
    storage::search::search(&state.pool, account_id, &parsed).await
}

/// Stat files dropped on the compose window into chip metadata (name +
/// size); duplicates and directories drop out, missing files error now
/// rather than at send time.
#[tauri::command]
pub async fn inspect_attachments(
    paths: Vec<String>,
) -> Result<Vec<crate::models::AttachmentInfo>, AppError> {
    mail::attachments::inspect(paths).await
}

/// Best-effort thumbnail for one attachment as a data: URI; None means
/// "show the generic file card". Never fails — previews are decoration.
#[tauri::command]
pub async fn attachment_preview(path: String) -> Result<Option<String>, AppError> {
    Ok(mail::attachments::preview(path).await)
}

// note: there is deliberately no direct send command — every outgoing
// message goes through the undoable queue below.

/// How long a queued message can still be undone before it really sends.
const UNDO_WINDOW: std::time::Duration = std::time::Duration::from_secs(8);

/// One id sequence for every send-* badge event, whichever path emits it
/// (undo queue, scheduler) — the outbox keys badges by this id.
static SEND_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Payload of the send-queued / send-finished / send-undone events the main
/// window renders as outbox badges.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SendEvent {
    id: u64,
    subject: String,
    error: Option<String>,
    /// Length of the undo window in milliseconds — drives the countdown
    /// donut in the badge. Only meaningful on send-queued; 0 elsewhere.
    undo_ms: u64,
}

/// Park a composed message for its undo window, then send it. Returns as
/// soon as the message is queued; progress is broadcast as events.
#[tauri::command]
pub async fn queue_send(
    app: AppHandle,
    state: State<'_, AppState>,
    message: OutgoingMessage,
) -> Result<(), AppError> {
    // why: build the MIME now even though it is rebuilt at send time — an
    // invalid address must surface in the compose window immediately, not
    // as a failure badge eight seconds after the window closed.
    let (from_name, from_email) = storage::aliases::sender(&state.pool, &message).await?;
    mail::smtp::build_message(&from_name, &from_email, &message).await?;

    // why best effort: the contacts book is a convenience — a failed write
    // must never block or fail a send that already validated.
    if let Err(err) =
        storage::contacts::harvest_sent(&state.pool, &[&message.to, &message.cc, &message.bcc])
            .await
    {
        eprintln!("failed to record recipients as contacts: {err}");
    }

    let id = SEND_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let subject = message.subject.clone();
    state.park_send(id, message);
    app.emit(
        "send-queued",
        SendEvent {
            id,
            subject: subject.clone(),
            error: None,
            undo_ms: UNDO_WINDOW.as_millis() as u64,
        },
    )?;

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(UNDO_WINDOW).await;
        // why: take_send is the ownership handshake — None means undo won
        // the race and this message must never leave the machine.
        let Some(message) = app.state::<AppState>().take_send(id) else {
            return;
        };
        let error = deliver(&app, &message).await.err();
        if let Some(err) = &error {
            eprintln!("queued send {id} failed: {err}");
            // why: a failed send must never destroy mail — the draft comes
            // back as a fresh compose window while the badge shows the error.
            if let Err(reopen) = open_compose_window(&app, message).await {
                eprintln!("failed to reopen draft for send {id}: {reopen}");
            }
        }
        let _ = app.emit(
            "send-finished",
            SendEvent {
                id,
                subject,
                error: error.map(|e| e.to_string()),
                undo_ms: 0,
            },
        );
    });
    Ok(())
}

/// Cancel a queued send inside its undo window: the draft reopens in a new
/// compose window. A no-op when the timer already fired — the badge will
/// resolve to sent/failed on its own.
#[tauri::command]
pub async fn undo_send(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u64,
) -> Result<(), AppError> {
    if let Some(draft) = state.take_send(id) {
        let subject = draft.subject.clone();
        open_compose_window(&app, draft).await?;
        app.emit(
            "send-undone",
            SendEvent {
                id,
                subject,
                error: None,
                undo_ms: 0,
            },
        )?;
    }
    Ok(())
}

/// Park a composed message in SQLite until its delivery time ("Send Later").
/// Validation mirrors queue_send: problems must surface in the compose
/// window now, not as a failure badge when the time comes.
#[tauri::command]
pub async fn schedule_send(
    app: AppHandle,
    state: State<'_, AppState>,
    message: OutgoingMessage,
    scheduled_at: i64,
) -> Result<(), AppError> {
    let (from_name, from_email) = storage::aliases::sender(&state.pool, &message).await?;
    mail::smtp::build_message(&from_name, &from_email, &message).await?;
    validate_scheduled_at(scheduled_at, now_epoch())?;
    storage::scheduled::insert(&state.pool, &message, scheduled_at).await?;
    // why: no payload — listeners (scheduled list in the sidebar) re-query
    // the DB, which is the single source of truth for parked messages.
    app.emit("scheduled-changed", ())?;
    Ok(())
}

/// Every parked send-later message, soonest first — pending and missed
/// alike; the frontend splits them by status.
#[tauri::command]
pub async fn list_scheduled(
    state: State<'_, AppState>,
) -> Result<Vec<crate::models::ScheduledMessage>, AppError> {
    storage::scheduled::list(&state.pool).await
}

/// "Send now" for one parked message (the catch-up dialog's confirm). The
/// take() is the claim — if the scheduler beat us to it, this is a no-op.
#[tauri::command]
pub async fn send_scheduled_now(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), AppError> {
    let Some(message) = storage::scheduled::take(&state.pool, id).await? else {
        return Ok(());
    };
    app.emit("scheduled-changed", ())?;
    // why spawn: delivery talks to the SMTP server — the dialog gets its
    // answer now, progress arrives as the usual outbox badge events.
    tauri::async_runtime::spawn(async move {
        deliver_scheduled(&app, message.outgoing()).await;
    });
    Ok(())
}

/// Cancel one parked message: it leaves the schedule and reopens as a
/// compose window, so nothing the user wrote is ever destroyed.
#[tauri::command]
pub async fn cancel_scheduled(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), AppError> {
    let Some(message) = storage::scheduled::take(&state.pool, id).await? else {
        return Ok(());
    };
    app.emit("scheduled-changed", ())?;
    open_compose_window(&app, message.outgoing()).await?;
    Ok(())
}

/// A schedule time must be strictly in the future — "now" belongs to the
/// ordinary send button.
fn validate_scheduled_at(scheduled_at: i64, now: i64) -> Result<(), AppError> {
    if scheduled_at <= now {
        return Err(AppError::Invalid(
            "scheduled time is in the past".to_string(),
        ));
    }
    Ok(())
}

/// Deliver a message the scheduler claimed, with the same badge events and
/// failure recovery (reopen as a draft window) as the undo queue — minus the
/// undo window, which already passed when the user picked a time.
pub(crate) async fn deliver_scheduled(app: &AppHandle, message: OutgoingMessage) {
    let id = SEND_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let subject = message.subject.clone();
    let _ = app.emit(
        "send-queued",
        SendEvent {
            id,
            subject: subject.clone(),
            error: None,
            undo_ms: 0,
        },
    );
    let error = deliver(app, &message).await.err();
    if let Some(err) = &error {
        eprintln!("scheduled send {id} failed: {err}");
        // why: a failed send must never destroy mail — same contract as the
        // undo queue's failure path.
        if let Err(reopen) = open_compose_window(app, message).await {
            eprintln!("failed to reopen draft for scheduled send {id}: {reopen}");
        }
    }
    let _ = app.emit(
        "send-finished",
        SendEvent {
            id,
            subject,
            error: error.map(|e| e.to_string()),
            undo_ms: 0,
        },
    );
}

/// The one SMTP delivery path: account row → MIME → session cache → send.
async fn deliver(app: &AppHandle, message: &OutgoingMessage) -> Result<(), AppError> {
    let state = app.state::<AppState>();
    let state = &*state;
    let account = storage::accounts::get(&state.pool, message.account_id).await?;
    let (from_name, from_email) = storage::aliases::sender(&state.pool, message).await?;
    let mime = mail::smtp::build_message(&from_name, &from_email, message).await?;
    // why: the session cache reads the keychain at most once per account per
    // run; the credential never reaches events or logs.
    let credential = state.credential(&account).await?;
    // why: formatted() consumes nothing but we need the raw bytes twice —
    // once for SMTP, once for the Sent-folder copy below.
    let raw = mime.formatted();
    mail::smtp::send(
        &account.smtp_host,
        account.smtp_port,
        &account.username,
        &credential,
        mime,
    )
    .await?;
    // why: best effort — the mail already left the machine, so a failed
    // Sent copy must never surface as a failed send (or reopen the draft).
    if let Err(err) = save_sent_copy(state, &account, &credential, &raw).await {
        eprintln!("sent copy for account {} failed: {err}", account.id);
    }
    // Same rule for the draft cleanup: a leftover draft is cosmetic, the
    // next save/sync can deal with it.
    if let Some(draft_id) = &message.draft_message_id {
        if let Err(err) = delete_sent_draft(app, state, &account, &credential, draft_id).await {
            eprintln!("could not delete draft {draft_id} after send: {err}");
        }
    }
    Ok(())
}

/// Remove one autosaved draft version after a send, so it stops looking
/// like unfinished work: local cache immediately, then the server copy
/// (behind the account's draft-push FIFO lock — the version might still be
/// mid-append).
async fn delete_sent_draft(
    app: &AppHandle,
    state: &AppState,
    account: &Account,
    credential: &Credential,
    draft_id: &str,
) -> Result<(), AppError> {
    let Some(drafts) = storage::mailboxes::name_for_role(&state.pool, account.id, "drafts").await?
    else {
        return Ok(()); // no drafts folder, nothing to clean up
    };
    storage::messages::delete_by_message_id(&state.pool, account.id, draft_id).await?;
    let _ = app.emit("messages-changed", MessagesChanged::reload(account.id));
    let lock = state.draft_push_lock(account.id);
    let _guard = lock.lock().await;
    delete_draft_on_server(app, &state.pool, account, credential, &drafts, draft_id).await
}

/// Mirror a delivered message into the account's IMAP Sent folder so other
/// clients (webmail, phone) see it. Skipped for servers that store their own
/// copy (Gmail), where appending would duplicate the message.
async fn save_sent_copy(
    state: &AppState,
    account: &Account,
    credential: &Credential,
    raw: &[u8],
) -> Result<(), AppError> {
    if mail::smtp::server_saves_sent_copy(&account.smtp_host) {
        return Ok(());
    }
    let Some(sent) = storage::mailboxes::sent_name(&state.pool, account.id).await? else {
        return Err(AppError::Imap("no sent folder discovered yet".to_string()));
    };
    let mut session = mail::imap::connect(
        &account.imap_host,
        account.imap_port,
        &account.username,
        credential,
    )
    .await?;
    let result = mail::imap::append(&mut session, &sent, "(\\Seen)", raw).await;
    let _ = session.logout().await;
    result
}

/// Save one compose draft into its account's Drafts folder on the server,
/// where webmail and other clients see it. Returns the Message-ID of the
/// saved version — the handle for replacing or deleting it later. IMAP has
/// no edit-in-place, so each save appends a fresh version and then deletes
/// the previous one (`previous_draft_id`).
#[tauri::command]
pub async fn save_draft(
    app: AppHandle,
    state: State<'_, AppState>,
    message: OutgoingMessage,
    previous_draft_id: Option<String>,
) -> Result<String, AppError> {
    let account = storage::accounts::get(&state.pool, message.account_id).await?;
    let Some(drafts) =
        storage::mailboxes::name_for_role(&state.pool, message.account_id, "drafts").await?
    else {
        return Err(AppError::Imap(
            "no drafts folder discovered yet".to_string(),
        ));
    };
    let message_id = mail::draft::generate_message_id();
    let (from_name, from_email) = storage::aliases::sender(&state.pool, &message).await?;
    let raw = mail::draft::build_draft(&from_name, &from_email, &message, &message_id).await?;

    // LOCAL-FIRST: mirror the new version into the cache and notify the UI
    // before touching the network — typing, closing and the conversation
    // view must never wait on the server. The provisional row carries a
    // negative uid (invisible to sync bookkeeping); the background push
    // below replaces it with the mirrored server row.
    let mut header = mail::sync::to_provisional_header(
        &raw,
        -provisional_uid(),
        !message.attachments.is_empty(),
    );
    let parsed = mail::parse::parse_body(&raw);
    header.snippet = parsed.snippet.clone();
    storage::messages::upsert_headers(&state.pool, account.id, &drafts, &[header]).await?;
    // why the gate: a text-only draft must not cache mail-parser's
    // synthesized HTML — the reopen path would mistake the conversion
    // for authored content.
    let authored_html = message
        .body_html
        .is_some()
        .then_some(parsed.html.as_deref())
        .flatten();
    if let Some(row_id) =
        storage::messages::find_by_message_id(&state.pool, account.id, &drafts, &message_id).await?
    {
        storage::messages::set_body(
            &state.pool,
            row_id,
            parsed.text.as_deref(),
            authored_html,
            &parsed.snippet,
            &parsed.images,
            &parsed.attachments,
            parsed.auth.as_ref(),
        )
        .await?;
    }
    // The replaced version leaves the cache with the same immediacy.
    if let Some(old_id) = &previous_draft_id {
        storage::messages::delete_by_message_id(&state.pool, account.id, old_id).await?;
    }
    let _ = app.emit("messages-changed", MessagesChanged::reload(account.id));

    // Server push in the background, serialized per account (FIFO lock) so
    // an autosave burst appends and deletes in save order.
    let credential = state.credential(&account).await?;
    let lock = state.draft_push_lock(account.id);
    let pool = state.pool.clone();
    let app = app.clone();
    let pushed_id = message_id.clone();
    tauri::async_runtime::spawn(async move {
        let _guard = lock.lock().await;
        if let Err(err) = push_draft(
            &app,
            &pool,
            &account,
            &credential,
            &drafts,
            &raw,
            &pushed_id,
            previous_draft_id.as_deref(),
        )
        .await
        {
            // The draft is safe in the local cache and reopens from it; the
            // next save (same window or after reopen) pushes again.
            eprintln!("draft push for account {} failed: {err}", account.id);
        }
    });
    Ok(message_id)
}

/// One provisional-row uid magnitude: epoch nanos, unique for any two saves
/// this side of a clock jump backwards within the same nanosecond.
fn provisional_uid() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(1)
}

/// Server side of a draft save: APPEND the new version, drop the previous
/// one, mirror the folder, then retire the provisional local row. Runs
/// under the account's draft-push lock.
#[allow(clippy::too_many_arguments)]
async fn push_draft(
    app: &AppHandle,
    pool: &sqlx::SqlitePool,
    account: &Account,
    credential: &Credential,
    drafts: &str,
    raw: &[u8],
    message_id: &str,
    previous_draft_id: Option<&str>,
) -> Result<(), AppError> {
    let mut session = mail::imap::connect(
        &account.imap_host,
        account.imap_port,
        &account.username,
        credential,
    )
    .await?;
    // why \Seen: the user wrote this text — it must not light up unread
    // badges here or in other clients.
    let appended = mail::imap::append(&mut session, drafts, "(\\Draft \\Seen)", raw).await;
    if appended.is_ok() {
        // why best effort: the new version is safely on the server; a
        // leftover old version is a cosmetic duplicate the user can delete,
        // never lost text. The next save also retries it via its own id.
        if let Some(old_id) = previous_draft_id {
            if let Err(err) = delete_draft_version(&mut session, drafts, old_id).await {
                eprintln!("could not delete draft version {old_id}: {err}");
            }
        }
        match mail::sync::sync_mailbox(pool, account.id, &mut session, drafts).await {
            Ok(_) => {
                // The server copy is mirrored — swap it in for the
                // provisional row, keeping the cached body.
                if let Err(err) =
                    storage::messages::adopt_provisional_draft(pool, account.id, drafts, message_id)
                        .await
                {
                    eprintln!("could not adopt provisional draft {message_id}: {err}");
                }
                let _ = app.emit("messages-changed", MessagesChanged::reload(account.id));
            }
            Err(err) => eprintln!("drafts sync after save failed: {err}"),
        }
    }
    let _ = session.logout().await;
    appended
}

/// "Don't Save" on compose close, and the draft card's Delete: remove the
/// draft locally right away, then from the server in the background. A
/// missing folder or version is fine — the state the user asked for (no
/// draft) already holds.
#[tauri::command]
pub async fn discard_draft(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
    draft_message_id: String,
) -> Result<(), AppError> {
    let account = storage::accounts::get(&state.pool, account_id).await?;
    let Some(drafts) = storage::mailboxes::name_for_role(&state.pool, account_id, "drafts").await?
    else {
        return Ok(()); // no drafts folder, nothing to clean up
    };
    // LOCAL-FIRST: the card leaves the conversation and the list now.
    storage::messages::delete_by_message_id(&state.pool, account_id, &draft_message_id).await?;
    let _ = app.emit("messages-changed", MessagesChanged::reload(account_id));

    // Behind the same FIFO lock as saves — a discard must never overtake
    // the push that is still appending the version it deletes.
    let credential = state.credential(&account).await?;
    let lock = state.draft_push_lock(account_id);
    let pool = state.pool.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _guard = lock.lock().await;
        if let Err(err) = delete_draft_on_server(
            &app,
            &pool,
            &account,
            &credential,
            &drafts,
            &draft_message_id,
        )
        .await
        {
            eprintln!("draft delete for account {} failed: {err}", account.id);
        }
    });
    Ok(())
}

/// Connect, delete one draft version, and reconcile the folder mirror.
/// Callers hold the account's draft-push lock.
async fn delete_draft_on_server(
    app: &AppHandle,
    pool: &sqlx::SqlitePool,
    account: &Account,
    credential: &Credential,
    drafts: &str,
    draft_id: &str,
) -> Result<(), AppError> {
    let mut session = mail::imap::connect(
        &account.imap_host,
        account.imap_port,
        &account.username,
        credential,
    )
    .await?;
    let result = delete_draft_version(&mut session, drafts, draft_id).await;
    if result.is_ok() {
        match mail::sync::sync_mailbox(pool, account.id, &mut session, drafts).await {
            Ok(_) => {
                let _ = app.emit("messages-changed", MessagesChanged::reload(account.id));
            }
            Err(err) => eprintln!("drafts sync after delete failed: {err}"),
        }
    }
    let _ = session.logout().await;
    result
}

/// Delete one draft version by Message-ID from `drafts`, if it still exists.
async fn delete_draft_version(
    session: &mut mail::imap::ImapSession,
    drafts: &str,
    message_id: &str,
) -> Result<(), AppError> {
    session
        .select(drafts)
        .await
        .map_err(|e| AppError::Imap(format!("select {drafts}: {e}")))?;
    if let Some(uid) = mail::imap::find_by_message_id(session, message_id).await? {
        mail::imap::delete_message(session, uid).await?;
    }
    Ok(())
}

/// Verify & Save: prove the submitted credentials against both servers
/// before the account is stored anywhere. The AppError Display strings
/// already name the failing leg ("imap error: …" / "smtp error: …").
#[tauri::command]
pub async fn test_connection(account: NewAccount, password: String) -> Result<(), AppError> {
    mail::verify_servers(&account, &Credential::Password(password)).await
}

/// Body for the viewer — served from cache, lazily fetched on first open.
/// `load_remote` is the per-message "Load images" click; it only has an
/// effect under the Ask policy (Block ignores it, Always needs no click).
#[tauri::command]
pub async fn get_message_body(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    load_remote: Option<bool>,
) -> Result<MessageBody, AppError> {
    let _t = timing::start("cmd::get_message_body");
    let loaded = load_body(&app, &state, message_id).await?;
    // Shown now: its HTML's retention period starts over.
    storage::messages::touch_html(&state.pool, message_id, now_epoch()).await?;

    let redundant = quote_repeats_thread(&state.pool, message_id, loaded.text.as_deref()).await?;
    let mut body = sanitized_body(
        &state.pool,
        loaded.html,
        loaded.text,
        &loaded.images,
        load_remote,
        redundant,
        None,
    )
    .await?;
    // why from the DB, not the parse result: both branches above have already
    // written the metadata (set_body), so one read serves cached and fresh.
    body.attachments = storage::messages::attachments(&state.pool, message_id).await?;
    body.auth = loaded.auth;
    let own = storage::accounts::own_emails(&state.pool).await?;
    body.sender_anomaly =
        storage::contacts::sender_anomaly(&state.pool, &loaded.from, &own).await?;
    Ok(body)
}

/// Whether this message's quoted history just repeats an earlier cached
/// message of its conversation — such a quote renders nothing at all (the
/// content sits one card up in the thread view).
async fn quote_repeats_thread(
    pool: &sqlx::SqlitePool,
    message_id: i64,
    text: Option<&str>,
) -> Result<bool, AppError> {
    let _t = timing::start("cmd::quote_repeats_thread");
    let Some(text) = text else { return Ok(false) };
    let (_, Some(quoted)) = mail::quote::split_text_quote(text) else {
        return Ok(false);
    };
    let thread = storage::messages::thread_of(pool, message_id).await?;
    let mut earlier = Vec::new();
    for header in &thread {
        if header.id == message_id {
            break;
        }
        let row = storage::messages::get_body(pool, header.id).await?;
        if let Some(body_text) = row.body_text {
            earlier.push(body_text);
        }
    }
    Ok(mail::quote::quote_matches_history(&quoted, &earlier))
}

/// Quote material for a reply to this message — served from the same body
/// cache as the viewer; a miss fetches the body first. Thin wrapper over
/// mail::quote::quote_material.
#[tauri::command]
pub async fn get_message_quote(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
) -> Result<MessageQuote, AppError> {
    let loaded = load_body(&app, &state, message_id).await?;
    Ok(mail::quote::quote_material(
        loaded.html.as_deref(),
        loaded.text.as_deref(),
        &loaded.images,
    ))
}

/// One message's body as load_body hands it out: the cached (or freshly
/// fetched) content plus the metadata that rides along with it.
struct LoadedBody {
    html: Option<String>,
    text: Option<String>,
    images: Vec<mail::parse::InlineImage>,
    /// SPF/DKIM/DMARC verdicts; None = unknown.
    auth: Option<AuthResults>,
    /// The stored From line ("Name <addr>"), for the sender check.
    from: String,
}

/// Cached AuthResults JSON back into the struct; unreadable or absent
/// means "unknown", never an error.
fn parse_auth(json: Option<&str>) -> Option<AuthResults> {
    json.and_then(|j| serde_json::from_str(j).ok())
}

/// A message's raw cached body — HTML, text and inline images — fetched
/// from the server on a cache miss, refetched once for cache rows written
/// before message_images/message_attachments existed (they'd otherwise
/// render blanks/nothing forever), and refetched when retention dropped the
/// HTML. When a refetch fails but text is cached (offline, say), the text
/// is served instead of an error.
async fn load_body(
    app: &AppHandle,
    state: &State<'_, AppState>,
    message_id: i64,
) -> Result<LoadedBody, AppError> {
    let row = storage::messages::get_body(&state.pool, message_id).await?;

    let cached = row.body_text.is_some() || row.body_html.is_some();
    let backfill = cached
        && (!row.attachments_scanned
            || (row.body_html.as_deref().is_some_and(|h| h.contains("cid:"))
                && storage::messages::images(&state.pool, message_id)
                    .await?
                    .is_empty()));

    if cached && !backfill && !row.html_dropped {
        let images = storage::messages::images(&state.pool, message_id).await?;
        return Ok(LoadedBody {
            html: row.body_html,
            text: row.body_text,
            images,
            auth: parse_auth(row.auth_results.as_deref()),
            from: row.from_addr,
        });
    }
    let fetched = async {
        let account = storage::accounts::get(&state.pool, row.account_id).await?;
        let credential = state.credential(&account).await?;
        mail::sync::fetch_body_into_cache(
            &state.pool,
            &account,
            &credential,
            message_id,
            &row.mailbox,
            row.uid,
        )
        .await
    }
    .await;
    let parsed = match fetched {
        Ok(parsed) => parsed,
        // why fall back only when something is cached: with nothing to show
        // the error is the answer; with the text still here (HTML dropped by
        // retention, or a legacy row), a read that works beats a failure.
        Err(err) if cached => {
            eprintln!("could not refetch the body of message {message_id}: {err}");
            let images = storage::messages::images(&state.pool, message_id).await?;
            return Ok(LoadedBody {
                html: row.body_html,
                text: row.body_text,
                images,
                auth: parse_auth(row.auth_results.as_deref()),
                from: row.from_addr,
            });
        }
        Err(err) => return Err(err),
    };
    // why: the snippet just became real — lists should refresh.
    app.emit("messages-changed", MessagesChanged::reload(row.account_id))?;
    Ok(LoadedBody {
        html: parsed.html,
        text: parsed.text,
        images: parsed.images,
        auth: parsed.auth,
        from: row.from_addr,
    })
}

/// Bodies for a whole conversation in one call — the conversation view's
/// bulk load. Cached bodies come straight from SQLite; everything missing
/// is fetched over a SINGLE IMAP session, grouped by folder, instead of one
/// TLS connect per message (which made long threads crawl open).
#[tauri::command]
pub async fn thread_bodies(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
) -> Result<std::collections::HashMap<i64, MessageBody>, AppError> {
    let _t = timing::start("cmd::thread_bodies");
    let thread = storage::messages::thread_of(&state.pool, message_id).await?;

    // Members to download, grouped by the folder to select: those with no
    // cached body yet, and those whose HTML retention dropped.
    let mut missing: std::collections::BTreeMap<String, Vec<(i64, i64)>> = Default::default();
    let mut fetch_account = None;
    // True when some member has nothing cached at all — then a failed
    // download is an error. Otherwise every member still has its text.
    let mut must_fetch = false;
    for header in &thread {
        let row = storage::messages::get_body(&state.pool, header.id).await?;
        let empty = row.body_text.is_none() && row.body_html.is_none();
        if empty || row.html_dropped {
            must_fetch |= empty;
            fetch_account = Some(row.account_id);
            missing
                .entry(row.mailbox)
                .or_default()
                .push((header.id, row.uid));
        }
    }

    if let Some(account_id) = fetch_account {
        let downloaded = async {
            let account = storage::accounts::get(&state.pool, account_id).await?;
            let credential = state.credential(&account).await?;
            let mut session = mail::imap::connect(
                &account.imap_host,
                account.imap_port,
                &account.username,
                &credential,
            )
            .await?;
            let fetched = download_thread_bodies(&state.pool, &mut session, &missing).await;
            let _ = session.logout().await;
            fetched
        }
        .await;
        match downloaded {
            // why: snippets just became real — lists should refresh.
            Ok(()) => app.emit("messages-changed", MessagesChanged::reload(account_id))?,
            // Offline, say: every member still has its text to show.
            Err(err) if !must_fetch => {
                eprintln!("could not refetch conversation bodies: {err}");
            }
            Err(err) => return Err(err),
        }
    }

    let own = storage::accounts::own_emails(&state.pool).await?;
    let mut bodies = std::collections::HashMap::new();
    // Oldest first (thread_of order): each message's quote is compared to
    // the bodies before it — a quote that repeats one renders nothing.
    let mut earlier_texts: Vec<String> = Vec::new();
    for header in &thread {
        let row = storage::messages::get_body(&state.pool, header.id).await?;
        let images = storage::messages::images(&state.pool, header.id).await?;
        let redundant = row
            .body_text
            .as_deref()
            .and_then(|t| mail::quote::split_text_quote(t).1)
            .is_some_and(|q| mail::quote::quote_matches_history(&q, &earlier_texts));
        if let Some(body_text) = &row.body_text {
            earlier_texts.push(body_text.clone());
        }
        let auth = parse_auth(row.auth_results.as_deref());
        let mut body = sanitized_body(
            &state.pool,
            row.body_html,
            row.body_text,
            &images,
            None,
            redundant,
            None,
        )
        .await?;
        body.attachments = storage::messages::attachments(&state.pool, header.id).await?;
        body.auth = auth;
        body.sender_anomaly =
            storage::contacts::sender_anomaly(&state.pool, &header.from, &own).await?;
        bodies.insert(header.id, body);
    }
    // Shown now: every member's HTML retention period starts over.
    let now = now_epoch();
    for header in &thread {
        storage::messages::touch_html(&state.pool, header.id, now).await?;
    }
    Ok(bodies)
}

/// Download and cache the bodies listed in `missing` (folder → (row id,
/// uid)) over one session, one SELECT per folder.
async fn download_thread_bodies(
    pool: &sqlx::SqlitePool,
    session: &mut mail::imap::ImapSession,
    missing: &std::collections::BTreeMap<String, Vec<(i64, i64)>>,
) -> Result<(), AppError> {
    for (mailbox, entries) in missing {
        session
            .select(mailbox)
            .await
            .map_err(|e| AppError::Imap(format!("select {mailbox}: {e}")))?;
        for (id, uid) in entries {
            // why skip: a uid can vanish mid-run (deleted elsewhere) —
            // the rest of the conversation must still load.
            let Some(raw) = mail::imap::fetch_body(session, *uid).await? else {
                continue;
            };
            let parsed = mail::parse::parse_body(&raw);
            storage::messages::set_body(
                pool,
                *id,
                parsed.text.as_deref(),
                parsed.html.as_deref(),
                &parsed.snippet,
                &parsed.images,
                &parsed.attachments,
                parsed.auth.as_ref(),
            )
            .await?;
        }
    }
    Ok(())
}

/// Download the raw RFC822 bytes of one cached message from its server.
async fn fetch_raw_message(
    state: &AppState,
    loc: &storage::messages::MessageLocation,
) -> Result<Vec<u8>, AppError> {
    let account = storage::accounts::get(&state.pool, loc.account_id).await?;
    let credential = state.credential(&account).await?;
    let mut session = mail::imap::connect(
        &account.imap_host,
        account.imap_port,
        &account.username,
        &credential,
    )
    .await?;
    session
        .select(&loc.mailbox)
        .await
        .map_err(|e| AppError::Imap(format!("select {}: {e}", loc.mailbox)))?;
    let raw = mail::imap::fetch_body(&mut session, loc.uid).await;
    let _ = session.logout().await;
    raw?.ok_or_else(|| AppError::Imap("message no longer on the server".to_string()))
}

/// Save one attachment to `path` (a native save-dialog pick, which already
/// confirmed any overwrite). The bytes are re-fetched from the server and
/// re-extracted by part index — they never touch the local DB.
#[tauri::command]
pub async fn save_attachment(
    state: State<'_, AppState>,
    attachment_id: i64,
    path: String,
) -> Result<(), AppError> {
    let meta = storage::messages::attachment(&state.pool, attachment_id).await?;
    let loc = storage::messages::location(&state.pool, meta.message_id).await?;
    let raw = fetch_raw_message(&state, &loc).await?;
    let data = mail::parse::attachment_data(&raw, meta.part_index).ok_or_else(|| {
        AppError::Imap(format!(
            "attachment {} not found in the message",
            meta.filename
        ))
    })?;
    tokio::fs::write(&path, data).await?;
    Ok(())
}

/// Save every attachment of a message into `dir` (a native folder pick):
/// one server fetch, each part written under its sanitized filename.
#[tauri::command]
pub async fn save_all_attachments(
    state: State<'_, AppState>,
    message_id: i64,
    dir: String,
) -> Result<(), AppError> {
    let attachments = storage::messages::attachments(&state.pool, message_id).await?;
    if attachments.is_empty() {
        return Ok(());
    }
    let loc = storage::messages::location(&state.pool, message_id).await?;
    let raw = fetch_raw_message(&state, &loc).await?;
    for meta in attachments {
        let data = mail::parse::attachment_data(&raw, meta.part_index).ok_or_else(|| {
            AppError::Imap(format!(
                "attachment {} not found in the message",
                meta.filename
            ))
        })?;
        let path = unique_path(
            std::path::Path::new(&dir),
            &mail::parse::safe_filename(&meta.filename),
        )
        .await?;
        tokio::fs::write(&path, data).await?;
    }
    Ok(())
}

/// `dir/name`, with " (n)" inserted before the extension while taken —
/// Save All picks only a directory, so it must not silently overwrite.
///
/// why async: `Path::exists` is a blocking stat, and this runs on the
/// async executor inside a command; `tokio::fs::try_exists` yields
/// instead, and reports a directory it cannot read rather than treating
/// it as free.
async fn unique_path(dir: &std::path::Path, name: &str) -> Result<std::path::PathBuf, AppError> {
    let candidate = dir.join(name);
    if !tokio::fs::try_exists(&candidate).await? {
        return Ok(candidate);
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (name.to_string(), String::new()),
    };
    // why a loop and not an iterator: `find` cannot await, and a loop that
    // only leaves by returning needs no `expect` to convince the compiler.
    let mut n = 2;
    loop {
        let numbered = dir.join(format!("{stem} ({n}){ext}"));
        if !tokio::fs::try_exists(&numbered).await? {
            return Ok(numbered);
        }
        n += 1;
    }
}

// SECURITY: the single place message HTML is prepared for the frontend —
// everything goes through mail::sanitize::build_srcdoc, cached or fresh.
// Remote images load only under policy Always or an explicit Ask-click;
// under Block a stray load_remote from the frontend changes nothing. The
// policy is read here, next to where it is enforced.
async fn sanitized_body(
    pool: &sqlx::SqlitePool,
    html: Option<String>,
    text: Option<String>,
    images: &[mail::parse::InlineImage],
    load_remote: Option<bool>,
    redundant_quote: bool,
    translations: Option<&std::collections::HashMap<String, String>>,
) -> Result<MessageBody, AppError> {
    let inputs = render_inputs(pool, html.as_deref(), load_remote).await?;
    Ok(render_body(
        html.as_deref(),
        text.as_deref(),
        images,
        &inputs,
        redundant_quote,
        translations,
    ))
}

/// What rendering a body needs besides the message: the remote-image
/// decision and the images fetched for it. Settled once — it awaits the
/// settings and maybe the network — so the body can then be rendered again
/// without waiting, as a translation does after every batch.
struct RenderInputs {
    policy: RemoteImagePolicy,
    load: bool,
    remote: std::collections::HashMap<String, String>,
}

async fn render_inputs(
    pool: &sqlx::SqlitePool,
    html: Option<&str>,
    load_remote: Option<bool>,
) -> Result<RenderInputs, AppError> {
    let policy = storage::settings::remote_image_policy(pool).await?;
    let load = policy == RemoteImagePolicy::Always
        || (policy == RemoteImagePolicy::Ask && load_remote.unwrap_or(false));
    let remote = match html {
        Some(h) if load => {
            let urls = mail::sanitize::remote_image_urls(h);
            mail::remote::load_images(pool, &urls).await
        }
        _ => std::collections::HashMap::new(),
    };
    Ok(RenderInputs {
        policy,
        load,
        remote,
    })
}

/// The body as the viewer gets it, from settled `inputs` — see
/// sanitized_body for the rules it keeps.
fn render_body(
    html: Option<&str>,
    text: Option<&str>,
    images: &[mail::parse::InlineImage],
    inputs: &RenderInputs,
    redundant_quote: bool,
    translations: Option<&std::collections::HashMap<String, String>>,
) -> MessageBody {
    let mut blocked_images = 0;
    let rendered = html.map(|h| {
        let body =
            mail::sanitize::build_srcdoc(h, images, &inputs.remote, redundant_quote, translations);
        blocked_images = body.blocked_remote;
        body.html
    });

    // Text bodies fold their quoted history on the frontend — split here so
    // the detection lives in one place (mail::quote). A quote the caller
    // verified as repeating an earlier thread message renders nothing.
    let (text, quoted_text) = match text {
        Some(t) => {
            let (own, quoted) = mail::quote::split_text_quote(t);
            let own = match translations {
                Some(translations) => mail::translate::apply_text(&own, translations),
                None => own,
            };
            (Some(own), if redundant_quote { None } else { quoted })
        }
        None => (None, None),
    };

    MessageBody {
        html: rendered,
        text,
        quoted_text,
        blocked_images,
        // why: !load, not just Ask — after the click the banner disappears
        // even when some images failed to fetch (no endless "load" loop).
        can_load_remote: inputs.policy == RemoteImagePolicy::Ask
            && !inputs.load
            && blocked_images > 0,
        // Filled by the callers from the DB — sanitization has no say.
        attachments: Vec::new(),
        auth: None,
        sender_anomaly: None,
    }
}

/// Decide how a reopened draft enters the compose window:
/// `(editor text, bodyHtml, parked quote)`.
///
/// 1. Saved with the quote still parked (compose marker present and both
///    bodies match): the quote rides parked behind ••• again, rich.
/// 2. Any other HTML draft — the user expanded the quote into the editor
///    before saving, or another client wrote it: reopen exactly what was
///    saved, the whole HTML as editor content. Re-guessing a quote out of
///    it would downgrade formatting the editor already owns.
/// 3. Text-only drafts: park the recognizable quoted tail (flat, the text
///    rendering is all there is).
///
/// SECURITY (hard rule): every piece of server-held HTML on this path —
/// marker parts and whole bodies alike — passes
/// mail::sanitize::sanitize_fragment before it may reach the compose
/// editor; bodyHtml is rebuilt from (or replaced by) the sanitized form.
fn park_draft_quote(
    body: String,
    body_html: Option<&str>,
) -> (String, Option<String>, Option<crate::models::DraftQuote>) {
    if let Some(split) = body_html.and_then(|html| mail::draft::split_saved_quote(&body, html)) {
        let quote_html = mail::sanitize::sanitize_fragment(&split.quote_html, &[]);
        let own_html = mail::sanitize::sanitize_fragment(&split.own_html, &[]);
        let rebuilt = mail::draft::compose_quoted_html(&own_html, &split.attribution, &quote_html);
        return (
            split.own_text,
            Some(rebuilt),
            Some(crate::models::DraftQuote {
                attribution: split.attribution,
                html: quote_html,
                text: split.quote_text,
            }),
        );
    }
    if let Some(html) = body_html.filter(|h| !h.trim().is_empty()) {
        return (
            body,
            Some(mail::sanitize::sanitize_fragment(html, &[])),
            None,
        );
    }
    let Some(split) = mail::draft::split_plain_quote(&body) else {
        return (body, None, None);
    };
    let quote_html = mail::sanitize::sanitize_fragment(&split.quote_html, &[]);
    (
        split.own_text,
        None,
        Some(crate::models::DraftQuote {
            attribution: split.attribution,
            html: quote_html,
            text: split.quote_text,
        }),
    )
}

/// Reopen a message from a Drafts folder for editing: fetch the raw draft,
/// parse it back into compose fields, and open a compose window that keeps
/// replacing this server version (via its Message-ID) on every save.
#[tauri::command]
pub async fn open_draft(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
) -> Result<(), AppError> {
    let loc = storage::messages::location(&state.pool, message_id).await?;

    // Fast path: a fully-cached draft (body present, no attachments) opens
    // straight from SQLite — no TLS connect, no server round-trip. The save
    // path caches the body it appends, so every draft written by this app
    // hits here; foreign or attachment-carrying drafts fall through to the
    // server fetch below.
    if let Some(cached) = storage::messages::cached_draft(&state.pool, message_id).await? {
        let from_email = storage::contacts::split_address_list(&cached.from_addr)
            .into_iter()
            .next()
            .map(|(_, email)| email);
        let alias = match from_email {
            Some(email) => {
                storage::aliases::find_by_email(&state.pool, loc.account_id, &email).await?
            }
            None => None,
        };
        let none_if_empty = |s: String| (!s.is_empty()).then_some(s);
        let (body, body_html, quote) = park_draft_quote(cached.body, cached.body_html.as_deref());
        return open_compose_window(
            &app,
            OutgoingMessage {
                account_id: loc.account_id,
                alias_id: alias.map(|a| a.id),
                to: cached.to,
                cc: cached.cc,
                bcc: cached.bcc,
                subject: cached.subject,
                body,
                body_html,
                attachments: Vec::new(),
                draft_message_id: none_if_empty(cached.message_id),
                in_reply_to: none_if_empty(cached.in_reply_to),
                references: none_if_empty(cached.references),
                quote,
            },
        )
        .await;
    }

    let account = storage::accounts::get(&state.pool, loc.account_id).await?;
    let credential = state.credential(&account).await?;

    let mut session = mail::imap::connect(
        &account.imap_host,
        account.imap_port,
        &account.username,
        &credential,
    )
    .await?;
    let fetched = async {
        session
            .select(&loc.mailbox)
            .await
            .map_err(|e| AppError::Imap(format!("select {}: {e}", loc.mailbox)))?;
        mail::imap::fetch_body(&mut session, loc.uid).await
    }
    .await;
    let _ = session.logout().await;
    let raw =
        fetched?.ok_or_else(|| AppError::Imap("draft no longer on the server".to_string()))?;

    let parsed = mail::parse::parse_draft(&raw);
    // why fall back to None: a draft From that matches no alias (foreign
    // client, deleted alias) reopens from the account's own address.
    let alias = match parsed.from_addr.as_deref() {
        Some(addr) => storage::aliases::find_by_email(&state.pool, loc.account_id, addr).await?,
        None => None,
    };
    let attachments = stash_draft_attachments(message_id, parsed.attachments).await?;
    let (body, body_html, quote) = park_draft_quote(parsed.body, parsed.body_html.as_deref());
    open_compose_window(
        &app,
        OutgoingMessage {
            account_id: loc.account_id,
            alias_id: alias.map(|a| a.id),
            to: parsed.to,
            cc: parsed.cc,
            bcc: parsed.bcc,
            subject: parsed.subject,
            body,
            body_html,
            attachments,
            draft_message_id: parsed.message_id,
            in_reply_to: parsed.in_reply_to,
            references: parsed.references,
            quote,
        },
    )
    .await
}

/// Write a reopened draft's attachment bytes into per-message temp files,
/// so the compose window can treat them exactly like freshly dropped files
/// (only paths travel through the app). Names were sanitized by parse_draft,
/// so the files always land inside the temp directory.
async fn stash_draft_attachments(
    message_id: i64,
    parts: Vec<mail::parse::DraftAttachment>,
) -> Result<Vec<crate::models::AttachmentRef>, AppError> {
    let mut refs = Vec::with_capacity(parts.len());
    for (index, part) in parts.into_iter().enumerate() {
        let dir = std::env::temp_dir().join(format!("flit-draft-{message_id}-{index}"));
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join(&part.name);
        tokio::fs::write(&path, &part.data).await?;
        refs.push(crate::models::AttachmentRef {
            path: path.to_string_lossy().into_owned(),
            name: part.name,
        });
    }
    Ok(refs)
}

/// Open a native compose window seeded with `draft`. Every call opens its
/// own window (unique label), so several drafts can be in flight at once.
#[tauri::command]
pub async fn open_compose(app: AppHandle, draft: OutgoingMessage) -> Result<(), AppError> {
    open_compose_window(&app, draft).await
}

/// Shared with the send queue, which reopens a draft on undo or failure.
pub(crate) async fn open_compose_window(
    app: &AppHandle,
    draft: OutgoingMessage,
) -> Result<(), AppError> {
    // why: a process-wide counter, not "count of open windows" — labels of
    // closed windows must never be reused while their drafts might linger.
    static COMPOSE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let label = format!(
        "compose-{}",
        COMPOSE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );

    let title = if draft.subject.trim().is_empty() {
        "New Message".to_string()
    } else {
        draft.subject.clone()
    };
    app.state::<AppState>().park_draft(&label, draft);

    let builder = tauri::WebviewWindowBuilder::new(
        app,
        &label,
        // why: all windows serve the same bundle — main.ts picks the root
        // component from the window label.
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title(title)
    .inner_size(640.0, 680.0)
    .min_inner_size(480.0, 400.0);
    // why: overlay puts the traffic lights on the compose toolbar strip,
    // matching the main window's chrome.
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    builder.build()?;
    Ok(())
}

/// One-shot draft pickup for a freshly opened compose window; `None` when
/// the draft was already taken (e.g. after a webview reload).
#[tauri::command]
pub fn take_compose_draft(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Option<OutgoingMessage>, AppError> {
    Ok(state.take_draft(window.label()))
}

/// Close the invoking compose window — windows lack the core close
/// permission, mirroring how settings closes through a command.
#[tauri::command]
pub async fn close_compose(window: tauri::WebviewWindow) -> Result<(), AppError> {
    window.close()?;
    Ok(())
}

#[tauri::command]
pub async fn get_avatar_lookup_enabled(state: State<'_, AppState>) -> Result<bool, AppError> {
    storage::settings::avatar_lookup_enabled(&state.pool).await
}

#[tauri::command]
pub async fn set_avatar_lookup_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), AppError> {
    storage::settings::set_avatar_lookup_enabled(&state.pool, enabled).await?;
    app.emit("settings-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn get_llm_summary_enabled(state: State<'_, AppState>) -> Result<bool, AppError> {
    storage::settings::llm_summary_enabled(&state.pool).await
}

#[tauri::command]
pub async fn set_llm_summary_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), AppError> {
    storage::settings::set_llm_summary_enabled(&state.pool, enabled).await?;
    app.emit("settings-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn get_llm_translate_enabled(state: State<'_, AppState>) -> Result<bool, AppError> {
    storage::settings::llm_translate_enabled(&state.pool).await
}

#[tauri::command]
pub async fn set_llm_translate_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), AppError> {
    storage::settings::set_llm_translate_enabled(&state.pool, enabled).await?;
    app.emit("settings-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn llm_status(app: AppHandle, state: State<'_, AppState>) -> Result<LlmStatus, AppError> {
    llm::status(&app, &state.pool).await
}

#[tauri::command]
pub async fn set_llm_model(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), AppError> {
    if llm::catalog::find(&id).is_none() {
        return Err(AppError::Invalid(format!("Unknown model: {id}")));
    }
    storage::settings::set_llm_model(&state.pool, &id).await?;
    app.emit("settings-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn get_llm_summary_language(state: State<'_, AppState>) -> Result<String, AppError> {
    storage::settings::llm_summary_language(&state.pool).await
}

#[tauri::command]
pub async fn set_llm_summary_language(
    app: AppHandle,
    state: State<'_, AppState>,
    language: String,
) -> Result<(), AppError> {
    storage::settings::set_llm_summary_language(&state.pool, &language).await?;
    app.emit("settings-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn get_llm_translate_language(state: State<'_, AppState>) -> Result<String, AppError> {
    storage::settings::llm_translate_language(&state.pool).await
}

#[tauri::command]
pub async fn set_llm_translate_language(
    app: AppHandle,
    state: State<'_, AppState>,
    language: String,
) -> Result<(), AppError> {
    if !llm::translate::valid_language(&language) {
        return Err(AppError::Invalid(format!("Unknown language: {language}")));
    }
    storage::settings::set_llm_translate_language(&state.pool, &language).await?;
    app.emit("settings-changed", ())?;
    Ok(())
}

/// A summary of one message, written by the picked local model — plain
/// text, bullet lines. Nothing leaves the machine. `fresh` skips the cache
/// (the panel's "summarize again"); pieces stream out as `summary-token`
/// events tagged with `request_id`, which cancel_summary also takes.
#[tauri::command]
pub async fn summarize_message(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    fresh: bool,
    request_id: String,
) -> Result<String, AppError> {
    let (header, text) = message_source(&app, &state, message_id).await?;
    llm::summarize_message(&app, &header, &text, fresh, &request_id).await
}

/// A summary already written for the message (or, with `thread`, for its
/// conversation), without generating anything — what an opened card shows
/// straight away. None when there is none, or the feature is not ready.
#[tauri::command]
pub async fn cached_summary(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    thread: bool,
) -> Result<Option<String>, AppError> {
    if thread {
        let messages = thread_sources(&app, &state, message_id).await?;
        llm::cached_thread_summary(&app, &messages).await
    } else {
        let (header, text) = message_source(&app, &state, message_id).await?;
        llm::cached_message_summary(&app, &header, &text).await
    }
}

/// The message's header and stored text.
async fn message_source(
    app: &AppHandle,
    state: &State<'_, AppState>,
    message_id: i64,
) -> Result<(MessageHeader, String), AppError> {
    let header = storage::messages::thread_of(&state.pool, message_id)
        .await?
        .into_iter()
        .find(|h| h.id == message_id)
        .ok_or_else(|| AppError::Invalid("Message not found.".to_string()))?;
    let text = summarizable_text(app, state, message_id).await?;
    Ok((header, text))
}

/// Every non-draft member of the message's conversation with its stored
/// text, oldest first.
async fn thread_sources(
    app: &AppHandle,
    state: &State<'_, AppState>,
    message_id: i64,
) -> Result<Vec<(MessageHeader, String)>, AppError> {
    let thread = storage::messages::thread_of(&state.pool, message_id).await?;
    let mut messages = Vec::with_capacity(thread.len());
    // why one by one: the conversation view has already pulled every body
    // through thread_bodies by the time its button is clicked, so these
    // are cache hits — a cold thread would open a session per message.
    for header in thread.into_iter().filter(|h| !h.is_draft) {
        let text = summarizable_text(app, state, header.id).await?;
        messages.push((header, text));
    }
    Ok(messages)
}

/// A summary of the whole conversation the message belongs to (drafts
/// left out), oldest message first.
#[tauri::command]
pub async fn summarize_thread(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    fresh: bool,
    request_id: String,
) -> Result<String, AppError> {
    let messages = thread_sources(&app, &state, message_id).await?;
    llm::summarize_thread(&app, &messages, fresh, &request_id).await
}

/// Stop a summary being written under `request_id`; a no-op once it has
/// finished.
#[tauri::command]
pub async fn cancel_summary(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), AppError> {
    state.llm_summaries.cancel(&request_id);
    Ok(())
}

/// The message's body with its text translated by the picked local model
/// into the language picked in Settings → Experimental — the same sanitized
/// render as get_message_body, its texts swapped (see mail::translate).
/// Progress goes out as `translation-progress` events tagged with
/// `request_id`, which cancel_summary also takes. Nothing leaves the machine.
#[tauri::command]
pub async fn translate_message(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    load_remote: Option<bool>,
    request_id: String,
) -> Result<MessageBody, AppError> {
    let language = storage::settings::llm_translate_language(&state.pool).await?;
    // why check a stored value: the name goes into the prompt word for word.
    if !llm::translate::valid_language(&language) {
        return Err(AppError::Invalid(format!(
            "Pick a translation language in Settings → Experimental (not \"{language}\")."
        )));
    }
    let language = language.trim();
    let loaded = load_body(&app, &state, message_id).await?;
    let texts = match (&loaded.html, &loaded.text) {
        (Some(html), _) => {
            mail::translate::segments(&mail::sanitize::sanitize_fragment(html, &loaded.images))
        }
        (None, Some(text)) => {
            mail::translate::text_segments(&mail::quote::split_text_quote(text).0)
        }
        (None, None) => Vec::new(),
    };
    if texts.is_empty() {
        return Err(AppError::Invalid(
            "This message has no text to translate.".to_string(),
        ));
    }
    let translations =
        llm::translate_texts(&app, message_id, &texts, language, &request_id).await?;

    let redundant = quote_repeats_thread(&state.pool, message_id, loaded.text.as_deref()).await?;
    let mut body = sanitized_body(
        &state.pool,
        loaded.html,
        loaded.text,
        &loaded.images,
        load_remote,
        redundant,
        Some(&translations),
    )
    .await?;
    body.attachments = storage::messages::attachments(&state.pool, message_id).await?;
    body.auth = loaded.auth;
    let own = storage::accounts::own_emails(&state.pool).await?;
    body.sender_anomaly =
        storage::contacts::sender_anomaly(&state.pool, &loaded.from, &own).await?;
    Ok(body)
}

/// The message's stored text, fetching the body if it is not cached yet.
async fn summarizable_text(
    app: &AppHandle,
    state: &State<'_, AppState>,
    message_id: i64,
) -> Result<String, AppError> {
    let body = load_body(app, state, message_id).await?;
    // why the html fallback: mail-parser fills body_text for html-only mail
    // at parse time, so this is for the odd cached row that has only html.
    Ok(body
        .text
        .or_else(|| {
            body.html
                .as_deref()
                .map(mail_parser::decoders::html::html_to_text)
        })
        .unwrap_or_default())
}

/// Start downloading a catalog model; returns once the background task is
/// spawned. The only user action that contacts a host other than the
/// user's own mail servers or a sender's image host — see CLAUDE.md.
#[tauri::command]
pub async fn download_llm_model(app: AppHandle, id: String) -> Result<(), AppError> {
    llm::start_download(app, &id)
}

#[tauri::command]
pub async fn cancel_llm_download(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    state.llm_downloads.cancel(&id);
    Ok(())
}

#[tauri::command]
pub async fn remove_llm_model(app: AppHandle, id: String) -> Result<(), AppError> {
    llm::remove_model(&app, &id).await
}

/// Icons for the given sender domains, as data: URIs. Domains without one are
/// simply absent and the list falls back to its monogram.
#[tauri::command]
pub async fn load_domain_avatars(
    state: State<'_, AppState>,
    domains: Vec<String>,
) -> Result<HashMap<String, String>, AppError> {
    // why the gate lives here: mail::avatars does the work unconditionally, so
    // this is the one place that decides whether the network may be touched
    // at all. Off means off — not even a cache read.
    if !storage::settings::avatar_lookup_enabled(&state.pool).await? {
        return Ok(HashMap::new());
    }
    Ok(mail::avatars::load_avatars(&state.pool, &domains).await)
}

#[tauri::command]
pub async fn get_remote_image_policy(
    state: State<'_, AppState>,
) -> Result<RemoteImagePolicy, AppError> {
    storage::settings::remote_image_policy(&state.pool).await
}

#[tauri::command]
pub async fn set_remote_image_policy(
    app: AppHandle,
    state: State<'_, AppState>,
    policy: RemoteImagePolicy,
) -> Result<(), AppError> {
    storage::settings::set_remote_image_policy(&state.pool, policy).await?;
    // why: the settings window mutates, the main window's open message view
    // listens and re-renders with the new policy.
    app.emit("settings-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn get_swipe_actions(state: State<'_, AppState>) -> Result<SwipeActions, AppError> {
    storage::settings::swipe_actions(&state.pool).await
}

#[tauri::command]
pub async fn set_swipe_actions(
    app: AppHandle,
    state: State<'_, AppState>,
    actions: SwipeActions,
) -> Result<(), AppError> {
    storage::settings::set_swipe_actions(&state.pool, actions).await?;
    // why: the settings window mutates, the main window's message list
    // listens and re-reads the gesture config.
    app.emit("settings-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn get_shortcuts(state: State<'_, AppState>) -> Result<Vec<ShortcutBinding>, AppError> {
    storage::settings::shortcuts(&state.pool).await
}

/// Bind `action` to `combo` (`None` unbinds it). Returns every binding, so
/// the settings pane can show which other action lost the combo.
#[tauri::command]
pub async fn set_shortcut(
    app: AppHandle,
    state: State<'_, AppState>,
    action: ShortcutAction,
    combo: Option<String>,
) -> Result<Vec<ShortcutBinding>, AppError> {
    let bindings = storage::settings::set_shortcut(&state.pool, action, combo).await?;
    // why: the main and compose windows each keep their own copy of the
    // bindings and re-read them on this event.
    app.emit("settings-changed", ())?;
    Ok(bindings)
}

#[tauri::command]
pub async fn reset_shortcuts(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<ShortcutBinding>, AppError> {
    storage::settings::reset_shortcuts(&state.pool).await?;
    app.emit("settings-changed", ())?;
    storage::settings::shortcuts(&state.pool).await
}

/// Is timing output switched on for this process (`FLIT_TIMING`)?
///
/// why the frontend asks: one env var has to cover both halves of a click.
/// A release build ships without devtools, so there is no console in which
/// to flip the webview's own flag — and a release build is exactly what the
/// measurements have to come from.
#[tauri::command]
pub fn timing_enabled() -> bool {
    timing::enabled()
}

/// Print one of the webview's timing lines on the backend's stderr.
///
/// why it has to come back here: a webview's console goes to the webview,
/// not to the process output, and a release build has no devtools to open.
/// Routing the lines through the backend is what puts both halves of a click
/// in the one stream `npm run timing` captures.
///
/// why the guard: with timing off this is a no-op, so nothing can use it as
/// a way to write to the app's output.
#[tauri::command]
pub fn log_timing(line: String) {
    if timing::enabled() {
        eprintln!("{line}");
    }
}

#[tauri::command]
pub async fn get_thread_order(state: State<'_, AppState>) -> Result<ThreadOrder, AppError> {
    storage::settings::thread_order(&state.pool).await
}

#[tauri::command]
pub async fn set_thread_order(
    app: AppHandle,
    state: State<'_, AppState>,
    order: ThreadOrder,
) -> Result<(), AppError> {
    storage::settings::set_thread_order(&state.pool, order).await?;
    // why: the settings window mutates, the main window's conversation view
    // listens and re-reads the order.
    app.emit("settings-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn get_appearance(state: State<'_, AppState>) -> Result<Appearance, AppError> {
    storage::settings::appearance(&state.pool).await
}

#[tauri::command]
pub async fn set_appearance(
    app: AppHandle,
    state: State<'_, AppState>,
    appearance: Appearance,
) -> Result<(), AppError> {
    storage::settings::set_appearance(&state.pool, appearance).await?;
    app.set_theme(theme_for(appearance));
    app.emit("settings-changed", ())?;
    Ok(())
}

/// The native theme an appearance pins, or `None` to follow macOS.
///
/// why native instead of a CSS class: on macOS `set_theme` sets the whole
/// app's NSAppearance, so WKWebView flips `prefers-color-scheme` (which the
/// palette in styles.css already keys off) AND the native chrome — traffic
/// lights, `<select>` popups, context menus, scrollbars — follows along.
pub fn theme_for(appearance: Appearance) -> Option<tauri::Theme> {
    match appearance {
        Appearance::System => None,
        Appearance::Light => Some(tauri::Theme::Light),
        Appearance::Dark => Some(tauri::Theme::Dark),
    }
}

#[tauri::command]
pub async fn get_date_time_format(state: State<'_, AppState>) -> Result<DateTimeFormat, AppError> {
    storage::settings::date_time_format(&state.pool).await
}

#[tauri::command]
pub async fn set_date_time_format(
    app: AppHandle,
    state: State<'_, AppState>,
    format: DateTimeFormat,
) -> Result<(), AppError> {
    storage::settings::set_date_time_format(&state.pool, format).await?;
    // why: every window writes dates — the list, the conversation and the
    // compose window's reply attribution all re-read on this event.
    app.emit("settings-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn get_notification_settings(
    state: State<'_, AppState>,
) -> Result<NotificationSettings, AppError> {
    storage::settings::notification_settings(&state.pool).await
}

#[tauri::command]
pub async fn set_notification_settings(
    state: State<'_, AppState>,
    settings: NotificationSettings,
) -> Result<(), AppError> {
    storage::settings::set_notification_settings(&state.pool, &settings).await
}

/// Set (or clear, with nulls) one account's notification overrides, then
/// broadcast so every window sees the fresh account list.
#[tauri::command]
pub async fn set_account_notifications(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    enabled: Option<bool>,
    sound: Option<String>,
) -> Result<(), AppError> {
    storage::accounts::set_notify(&state.pool, id, enabled, sound.as_deref()).await?;
    app.emit("accounts-changed", ())?;
    Ok(())
}

/// Play a short preview of a notification sound (settings pane UX);
/// "default"/"none" have nothing to play and are a no-op.
#[tauri::command]
pub async fn preview_notification_sound(sound: String) -> Result<(), AppError> {
    crate::notify::preview(&sound);
    Ok(())
}

#[tauri::command]
pub async fn list_signatures(state: State<'_, AppState>) -> Result<Vec<Signature>, AppError> {
    storage::signatures::list(&state.pool).await
}

#[tauri::command]
pub async fn create_signature(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
) -> Result<Signature, AppError> {
    let created = storage::signatures::create(&state.pool, &name).await?;
    app.emit("signatures-changed", ())?;
    Ok(created)
}

#[tauri::command]
pub async fn update_signature(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    name: String,
    body: String,
) -> Result<(), AppError> {
    storage::signatures::update(&state.pool, id, &name, &body).await?;
    app.emit("signatures-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_signature(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), AppError> {
    storage::signatures::delete(&state.pool, id).await?;
    app.emit("signatures-changed", ())?;
    // why: ON DELETE SET NULL may have cleared account defaults too.
    app.emit("accounts-changed", ())?;
    Ok(())
}

/// Make one signature the default for exactly the given accounts.
#[tauri::command]
pub async fn set_signature_accounts(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    account_ids: Vec<i64>,
) -> Result<(), AppError> {
    storage::signatures::set_default_for_accounts(&state.pool, id, &account_ids).await?;
    // why: defaults live on the account rows, so account listeners refetch.
    app.emit("accounts-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn list_aliases(state: State<'_, AppState>) -> Result<Vec<Alias>, AppError> {
    storage::aliases::list(&state.pool).await
}

#[tauri::command]
pub async fn add_alias(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
    name: String,
    email: String,
) -> Result<Alias, AppError> {
    let added = storage::aliases::add(&state.pool, account_id, &name, &email).await?;
    // why: aliases are part of an account's send identity — the settings
    // pane and the compose From picker listen on the same account event.
    app.emit("accounts-changed", ())?;
    Ok(added)
}

#[tauri::command]
pub async fn update_alias(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    name: String,
    email: String,
) -> Result<(), AppError> {
    storage::aliases::update(&state.pool, id, &name, &email).await?;
    app.emit("accounts-changed", ())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_alias(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
) -> Result<(), AppError> {
    storage::aliases::delete(&state.pool, id).await?;
    app.emit("accounts-changed", ())?;
    Ok(())
}

/// Pick the identity new mail from this account starts with;
/// `None` = the account's own address.
#[tauri::command]
pub async fn set_default_alias(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
    alias_id: Option<i64>,
) -> Result<(), AppError> {
    storage::aliases::set_default(&state.pool, account_id, alias_id).await?;
    app.emit("accounts-changed", ())?;
    Ok(())
}

// why: async on purpose — Tauri docs warn that creating windows from a sync
// command can deadlock on some platforms (the command runs on the main
// thread there, and window creation needs it too).
#[tauri::command]
pub async fn open_settings(app: AppHandle) -> Result<(), AppError> {
    // why: a closed window is destroyed, so reopen = rebuild; if it's only
    // hidden/behind, just focus it instead of spawning a duplicate.
    if let Some(window) = app.get_webview_window("settings") {
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        &app,
        "settings",
        // why: both windows serve the same bundle — main.ts picks the root
        // component from the window label.
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("Settings")
    .inner_size(720.0, 480.0)
    .min_inner_size(560.0, 360.0)
    .build()?;
    Ok(())
}

#[tauri::command]
pub async fn close_settings(app: AppHandle) -> Result<(), AppError> {
    if let Some(window) = app.get_webview_window("settings") {
        window.close()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_maps_to_a_pinned_theme_or_none_for_system() {
        assert_eq!(theme_for(Appearance::System), None);
        assert_eq!(theme_for(Appearance::Light), Some(tauri::Theme::Light));
        assert_eq!(theme_for(Appearance::Dark), Some(tauri::Theme::Dark));
    }

    #[tokio::test]
    async fn unique_path_numbers_past_every_taken_name() {
        let dir = std::env::temp_dir().join(format!("flit-unique-path-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();

        assert_eq!(
            unique_path(&dir, "report.pdf").await.unwrap(),
            dir.join("report.pdf")
        );

        tokio::fs::write(dir.join("report.pdf"), b"").await.unwrap();
        tokio::fs::write(dir.join("report (2).pdf"), b"")
            .await
            .unwrap();
        assert_eq!(
            unique_path(&dir, "report.pdf").await.unwrap(),
            dir.join("report (3).pdf")
        );

        // No extension: the counter goes on the end.
        tokio::fs::write(dir.join("README"), b"").await.unwrap();
        assert_eq!(
            unique_path(&dir, "README").await.unwrap(),
            dir.join("README (2)")
        );

        // A dotfile keeps its name whole rather than becoming " (2).env".
        tokio::fs::write(dir.join(".env"), b"").await.unwrap();
        assert_eq!(
            unique_path(&dir, ".env").await.unwrap(),
            dir.join(".env (2)")
        );

        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

    #[test]
    fn progress_fires_once_then_waits_out_the_interval() {
        // why explicit instants instead of sleeping: the pacing is the whole
        // point of this type, and a test that waits a real second to prove it
        // is both slow and flaky.
        let progress = Progress::new();
        let start = std::time::Instant::now();

        assert!(progress.due(start), "the first batch reports immediately");
        assert!(!progress.due(start + PROGRESS_INTERVAL / 2));
        assert!(progress.due(start + PROGRESS_INTERVAL));
        // The clock restarts from the event that went out, not from the
        // batches suppressed in between.
        assert!(!progress.due(start + PROGRESS_INTERVAL + PROGRESS_INTERVAL / 2));
    }

    #[test]
    fn schedule_times_must_be_in_the_future() {
        assert!(validate_scheduled_at(1_001, 1_000).is_ok());
        // "now" and the past both belong to the ordinary send button
        assert!(validate_scheduled_at(1_000, 1_000).is_err());
        assert!(validate_scheduled_at(999, 1_000).is_err());
    }

    fn located(
        id: i64,
        account_id: i64,
        mailbox: &str,
        uid: i64,
    ) -> (i64, storage::messages::MessageLocation) {
        (
            id,
            storage::messages::MessageLocation {
                account_id,
                mailbox: mailbox.to_string(),
                uid,
            },
        )
    }

    fn imap_err(text: &str) -> Result<(), AppError> {
        Err(AppError::Imap(text.to_string()))
    }

    #[test]
    fn first_error_of_reports_the_earliest_failure_in_batch_order() {
        assert!(first_error_of(vec![]).is_ok());
        assert!(first_error_of(vec![Ok(()), Ok(())]).is_ok());
        assert_eq!(
            first_error_of(vec![Ok(()), imap_err("second"), imap_err("third")])
                .unwrap_err()
                .to_string(),
            imap_err("second").unwrap_err().to_string()
        );
    }

    #[test]
    fn a_bulk_run_reports_the_first_failure_it_hit() {
        assert!(first_error(Ok(()), Ok(())).is_ok());
        assert_eq!(
            first_error(Ok(()), imap_err("second"))
                .unwrap_err()
                .to_string(),
            AppError::Imap("second".to_string()).to_string(),
        );
        // Later batches still ran; their errors must not mask the first.
        assert_eq!(
            first_error(imap_err("first"), imap_err("second"))
                .unwrap_err()
                .to_string(),
            AppError::Imap("first".to_string()).to_string(),
        );
        assert_eq!(
            first_error(imap_err("first"), Ok(()))
                .unwrap_err()
                .to_string(),
            AppError::Imap("first".to_string()).to_string(),
        );
    }

    #[test]
    fn grouping_an_empty_selection_yields_no_work() {
        assert_eq!(group_by_folder(Vec::new()), Vec::new());
    }

    #[test]
    fn rows_of_one_folder_share_a_single_batch() {
        let batches = group_by_folder(vec![
            located(1, 7, "INBOX", 100),
            located(2, 7, "INBOX", 101),
        ]);

        assert_eq!(
            batches,
            vec![FolderBatch {
                account_id: 7,
                mailbox: "INBOX".to_string(),
                rows: vec![(1, 100), (2, 101)],
            }]
        );
    }

    // Expanding conversations can name the same row twice — two selected
    // rows of one thread, say. Moving a uid twice would fail the second time.
    #[test]
    fn a_row_named_twice_is_moved_once() {
        let batches = group_by_folder(vec![
            located(1, 7, "INBOX", 100),
            located(2, 7, "INBOX", 101),
            located(1, 7, "INBOX", 100),
        ]);

        assert_eq!(
            batches,
            vec![FolderBatch {
                account_id: 7,
                mailbox: "INBOX".to_string(),
                rows: vec![(1, 100), (2, 101)],
            }]
        );
    }

    #[test]
    fn folders_and_accounts_each_get_their_own_batch() {
        // A unified-inbox selection: two accounts, and one of them spans two
        // folders. Every batch is one IMAP session.
        let batches = group_by_folder(vec![
            located(1, 7, "INBOX", 100),
            located(2, 9, "INBOX", 200),
            located(3, 7, "Archive", 300),
            located(4, 7, "INBOX", 101),
        ]);

        assert_eq!(
            batches,
            vec![
                FolderBatch {
                    account_id: 7,
                    mailbox: "Archive".to_string(),
                    rows: vec![(3, 300)],
                },
                FolderBatch {
                    account_id: 7,
                    mailbox: "INBOX".to_string(),
                    rows: vec![(1, 100), (4, 101)],
                },
                FolderBatch {
                    account_id: 9,
                    mailbox: "INBOX".to_string(),
                    rows: vec![(2, 200)],
                },
            ]
        );
    }
}
