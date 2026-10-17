use std::collections::HashMap;

use sqlx::{SqliteConnection, SqlitePool};

use crate::error::AppError;
use crate::models::{
    Appearance, DateFormat, DateTimeFormat, NotificationSettings, RemoteImagePolicy,
    ShortcutAction, ShortcutBinding, SwipeAction, SwipeActions, ThreadOrder, TimeFormat,
};

const REMOTE_IMAGES_KEY: &str = "remote_images";
const THREAD_ORDER_KEY: &str = "thread_order";
const APPEARANCE_KEY: &str = "appearance";
const DATE_FORMAT_KEY: &str = "date_format";
const TIME_FORMAT_KEY: &str = "time_format";
const NOTIFICATIONS_ENABLED_KEY: &str = "notifications_enabled";
const NOTIFICATION_SOUND_KEY: &str = "notification_sound";
const SYNC_INTERVAL_KEY: &str = "sync_interval_minutes";
const PUSH_ENABLED_KEY: &str = "push_enabled";
const SWIPE_LEFT_KEY: &str = "swipe_left";
const SWIPE_RIGHT_KEY: &str = "swipe_right";
const AVATAR_LOOKUP_KEY: &str = "avatar_lookup";
const LLM_SUMMARY_KEY: &str = "llm_summary";
const LLM_TRANSLATE_KEY: &str = "llm_translate";
const LLM_MODEL_KEY: &str = "llm_model";
const LLM_SUMMARY_LANGUAGE_KEY: &str = "llm_summary_language";
const LLM_TRANSLATE_LANGUAGE_KEY: &str = "llm_translate_language";
const MAINTENANCE_REV_KEY: &str = "maintenance_rev";

async fn value(pool: &SqlitePool, key: &str) -> Result<Option<String>, AppError> {
    Ok(
        sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(pool)
            .await?,
    )
}

async fn upsert(pool: &SqlitePool, key: &str, value: &str) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Which round of one-off data repairs this database has already had.
/// Missing or unparsable reads as 0, so a database that has never been
/// through maintenance gets it.
pub async fn maintenance_rev(pool: &SqlitePool) -> Result<i64, AppError> {
    Ok(value(pool, MAINTENANCE_REV_KEY)
        .await?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

pub async fn set_maintenance_rev(pool: &SqlitePool, rev: i64) -> Result<(), AppError> {
    upsert(pool, MAINTENANCE_REV_KEY, &rev.to_string()).await
}

/// Stored notification settings. Each field that is missing or unparsable
/// falls back to its default independently — one corrupt row never drags the
/// other settings down with it.
pub async fn notification_settings(pool: &SqlitePool) -> Result<NotificationSettings, AppError> {
    let defaults = NotificationSettings::default();
    let enabled = match value(pool, NOTIFICATIONS_ENABLED_KEY).await?.as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => defaults.enabled,
    };
    let sound = value(pool, NOTIFICATION_SOUND_KEY)
        .await?
        .filter(|sound| !sound.is_empty())
        .unwrap_or(defaults.sound);
    let sync_interval_minutes = value(pool, SYNC_INTERVAL_KEY)
        .await?
        .and_then(|minutes| minutes.parse::<i64>().ok())
        .filter(|minutes| *minutes >= 0)
        .unwrap_or(defaults.sync_interval_minutes);
    let push_enabled = match value(pool, PUSH_ENABLED_KEY).await?.as_deref() {
        Some("true") => true,
        Some("false") => false,
        _ => defaults.push_enabled,
    };
    Ok(NotificationSettings {
        enabled,
        sound,
        sync_interval_minutes,
        push_enabled,
    })
}

pub async fn set_notification_settings(
    pool: &SqlitePool,
    settings: &NotificationSettings,
) -> Result<(), AppError> {
    let enabled = if settings.enabled { "true" } else { "false" };
    upsert(pool, NOTIFICATIONS_ENABLED_KEY, enabled).await?;
    upsert(pool, NOTIFICATION_SOUND_KEY, &settings.sound).await?;
    upsert(
        pool,
        SYNC_INTERVAL_KEY,
        &settings.sync_interval_minutes.to_string(),
    )
    .await?;
    let push = if settings.push_enabled {
        "true"
    } else {
        "false"
    };
    upsert(pool, PUSH_ENABLED_KEY, push).await?;
    Ok(())
}

/// Whether sender-domain avatar lookups are switched on. Anything other than
/// a stored "true" — missing, corrupt, explicitly off — reads as off, because
/// this is the switch that permits calls to hosts beyond the user's own mail
/// servers (see the hard rules in CLAUDE.md).
pub async fn avatar_lookup_enabled(pool: &SqlitePool) -> Result<bool, AppError> {
    Ok(value(pool, AVATAR_LOOKUP_KEY).await?.as_deref() == Some("true"))
}

pub async fn set_avatar_lookup_enabled(pool: &SqlitePool, enabled: bool) -> Result<(), AppError> {
    upsert(
        pool,
        AVATAR_LOOKUP_KEY,
        if enabled { "true" } else { "false" },
    )
    .await
}

/// Whether the experimental on-device summaries are switched on. Same
/// "only a stored true counts" rule as the avatar lookup: this switch is what
/// lets the user fetch a model file from a host that is not their mail
/// server, so a missing or corrupt value must read as off.
pub async fn llm_summary_enabled(pool: &SqlitePool) -> Result<bool, AppError> {
    Ok(value(pool, LLM_SUMMARY_KEY).await?.as_deref() == Some("true"))
}

pub async fn set_llm_summary_enabled(pool: &SqlitePool, enabled: bool) -> Result<(), AppError> {
    upsert(
        pool,
        LLM_SUMMARY_KEY,
        if enabled { "true" } else { "false" },
    )
    .await
}

/// The model picked for summaries, or None when nothing was picked yet or
/// the stored id is one this build does not know (written by another
/// version) — an unknown pick must not be trusted as "ready".
pub async fn llm_model(pool: &SqlitePool) -> Result<Option<String>, AppError> {
    Ok(value(pool, LLM_MODEL_KEY)
        .await?
        .filter(|id| crate::llm::catalog::find(id).is_some()))
}

pub async fn set_llm_model(pool: &SqlitePool, id: &str) -> Result<(), AppError> {
    upsert(pool, LLM_MODEL_KEY, id).await
}

/// The language summaries are written in: "auto" (the message's own) or an
/// English language name the prompt uses verbatim ("Slovak"). Missing or
/// blank reads as auto.
pub async fn llm_summary_language(pool: &SqlitePool) -> Result<String, AppError> {
    Ok(value(pool, LLM_SUMMARY_LANGUAGE_KEY)
        .await?
        .filter(|language| !language.trim().is_empty())
        .unwrap_or_else(|| crate::llm::summarize::AUTO_LANGUAGE.to_string()))
}

pub async fn set_llm_summary_language(pool: &SqlitePool, language: &str) -> Result<(), AppError> {
    upsert(pool, LLM_SUMMARY_LANGUAGE_KEY, language.trim()).await
}

/// The language messages are translated into: the stored pick, else the
/// summary language when that names one, else English. Never "auto" — a
/// translation needs a language to go to.
pub async fn llm_translate_language(pool: &SqlitePool) -> Result<String, AppError> {
    if let Some(language) = value(pool, LLM_TRANSLATE_LANGUAGE_KEY)
        .await?
        .filter(|language| !language.trim().is_empty())
    {
        return Ok(language);
    }
    let summary = llm_summary_language(pool).await?;
    Ok(if summary == crate::llm::summarize::AUTO_LANGUAGE {
        "English".to_string()
    } else {
        summary
    })
}

pub async fn set_llm_translate_language(pool: &SqlitePool, language: &str) -> Result<(), AppError> {
    upsert(pool, LLM_TRANSLATE_LANGUAGE_KEY, language.trim()).await
}

/// The Experimental switch for on-device translation — its own switch, so
/// a reader can have translation without summaries. Same "only a stored
/// true counts" rule as the summaries switch.
pub async fn llm_translate_enabled(pool: &SqlitePool) -> Result<bool, AppError> {
    Ok(value(pool, LLM_TRANSLATE_KEY).await?.as_deref() == Some("true"))
}

pub async fn set_llm_translate_enabled(pool: &SqlitePool, enabled: bool) -> Result<(), AppError> {
    upsert(
        pool,
        LLM_TRANSLATE_KEY,
        if enabled { "true" } else { "false" },
    )
    .await
}

/// The stored remote-image policy, falling back to the default (Ask) when
/// nothing was saved yet.
pub async fn remote_image_policy(pool: &SqlitePool) -> Result<RemoteImagePolicy, AppError> {
    let value: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(REMOTE_IMAGES_KEY)
        .fetch_optional(pool)
        .await?;
    Ok(value
        .as_deref()
        .map(RemoteImagePolicy::parse)
        .unwrap_or_default())
}

pub async fn set_remote_image_policy(
    pool: &SqlitePool,
    policy: RemoteImagePolicy,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(REMOTE_IMAGES_KEY)
    .bind(policy.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

/// The stored conversation-view order; missing or corrupt values fall back
/// to the default (newest message at the bottom).
pub async fn thread_order(pool: &SqlitePool) -> Result<ThreadOrder, AppError> {
    Ok(value(pool, THREAD_ORDER_KEY)
        .await?
        .as_deref()
        .map(ThreadOrder::parse)
        .unwrap_or_default())
}

pub async fn set_thread_order(pool: &SqlitePool, order: ThreadOrder) -> Result<(), AppError> {
    upsert(pool, THREAD_ORDER_KEY, order.as_str()).await
}

/// The stored colour-scheme preference; missing or corrupt values fall back
/// to following the system.
pub async fn appearance(pool: &SqlitePool) -> Result<Appearance, AppError> {
    Ok(value(pool, APPEARANCE_KEY)
        .await?
        .as_deref()
        .map(Appearance::parse)
        .unwrap_or_default())
}

pub async fn set_appearance(pool: &SqlitePool, appearance: Appearance) -> Result<(), AppError> {
    upsert(pool, APPEARANCE_KEY, appearance.as_str()).await
}

/// The stored date/time display format. The halves are read independently,
/// so an unreadable date pattern still leaves the clock preference intact.
pub async fn date_time_format(pool: &SqlitePool) -> Result<DateTimeFormat, AppError> {
    Ok(DateTimeFormat {
        date: value(pool, DATE_FORMAT_KEY)
            .await?
            .as_deref()
            .map(DateFormat::parse)
            .unwrap_or_default(),
        time: value(pool, TIME_FORMAT_KEY)
            .await?
            .as_deref()
            .map(TimeFormat::parse)
            .unwrap_or_default(),
    })
}

pub async fn set_date_time_format(
    pool: &SqlitePool,
    format: DateTimeFormat,
) -> Result<(), AppError> {
    upsert(pool, DATE_FORMAT_KEY, format.date.as_str()).await?;
    upsert(pool, TIME_FORMAT_KEY, format.time.as_str()).await?;
    Ok(())
}

/// The configured swipe actions; a missing or corrupt side falls back to
/// that side's shipped default (left = Archive, right = ToggleRead).
pub async fn swipe_actions(pool: &SqlitePool) -> Result<SwipeActions, AppError> {
    let defaults = SwipeActions::default();
    Ok(SwipeActions {
        left: swipe_side(pool, SWIPE_LEFT_KEY)
            .await?
            .unwrap_or(defaults.left),
        right: swipe_side(pool, SWIPE_RIGHT_KEY)
            .await?
            .unwrap_or(defaults.right),
    })
}

async fn swipe_side(pool: &SqlitePool, key: &str) -> Result<Option<SwipeAction>, AppError> {
    Ok(value(pool, key)
        .await?
        .as_deref()
        .and_then(SwipeAction::parse))
}

pub async fn set_swipe_actions(pool: &SqlitePool, actions: SwipeActions) -> Result<(), AppError> {
    upsert(pool, SWIPE_LEFT_KEY, actions.left.as_str()).await?;
    upsert(pool, SWIPE_RIGHT_KEY, actions.right.as_str()).await?;
    Ok(())
}

/// Settings key prefix for keyboard shortcuts: one row per action,
/// `shortcut.<action-id>`. A missing row means the default; `""` means the
/// user unbound it.
const SHORTCUT_PREFIX: &str = "shortcut.";

/// Keys a combo may end in besides `A`–`Z`, `0`–`9` and `F1`–`F12` (those
/// are checked by shape). Punctuation uses the `KeyboardEvent.code` names.
const SHORTCUT_KEYS: &[&str] = &[
    "Enter",
    "Backspace",
    "Delete",
    "Space",
    "Escape",
    "Tab",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Comma",
    "Period",
    "Slash",
    "Semicolon",
    "Quote",
    "BracketLeft",
    "BracketRight",
    "Backslash",
    "Minus",
    "Equal",
    "Backquote",
];

/// Combos Tauri's default macOS menu already owns (checked against tauri
/// 2.11.5's `menu.rs`) — Quit, Hide, Undo, Copy and the rest.
const MACOS_COMBOS: &[&str] = &[
    "Meta+Q",
    "Meta+W",
    "Meta+H",
    "Alt+Meta+H",
    "Meta+M",
    "Meta+Z",
    "Shift+Meta+Z",
    "Meta+X",
    "Meta+C",
    "Meta+V",
    "Meta+A",
    "Ctrl+Meta+F",
];

/// Combos Flit itself keeps fixed: the custom ⌘, Settings item and the list
/// navigation. Escape and Tab are refused in any combination as well.
const FIXED_COMBOS: &[&str] = &[
    "Meta+Comma",
    "ArrowUp",
    "ArrowDown",
    "Shift+ArrowUp",
    "Shift+ArrowDown",
];

/// Modifiers in the one order a canonical combo spells them.
const MODIFIERS: [&str; 4] = ["Ctrl", "Alt", "Shift", "Meta"];

fn is_shortcut_key(key: &str) -> bool {
    let single_char = |pred: fn(&u8) -> bool| key.len() == 1 && key.as_bytes().iter().all(pred);
    let function_key = key
        .strip_prefix('F')
        .filter(|n| !n.starts_with('0'))
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=12).contains(&n));
    single_char(u8::is_ascii_uppercase)
        || single_char(u8::is_ascii_digit)
        || function_key
        || SHORTCUT_KEYS.contains(&key)
}

/// Is `combo` a well-formed canonical combo that is free for the user to bind?
fn check_combo(combo: &str) -> Result<(), AppError> {
    let malformed = || AppError::Invalid("That is not a valid shortcut.".into());
    let mut parts: Vec<&str> = combo.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    // why a moving start index: each modifier has to appear later in
    // MODIFIERS than the previous one, which rules out both duplicates and
    // the wrong order in one pass.
    let mut next = 0;
    for part in parts {
        match MODIFIERS[next..].iter().position(|m| *m == part) {
            Some(offset) => next += offset + 1,
            None => return Err(malformed()),
        }
    }
    if !is_shortcut_key(key) {
        return Err(malformed());
    }
    // why two messages: "reserved" alone left the user guessing whether
    // macOS or Flit holds the combo — and only Flit's own can never move.
    if MACOS_COMBOS.contains(&combo) {
        return Err(AppError::Invalid(
            "macOS already uses this shortcut.".into(),
        ));
    }
    if key == "Escape" || key == "Tab" || FIXED_COMBOS.contains(&combo) {
        return Err(AppError::Invalid(
            "Flit already uses this shortcut, and it can't be changed.".into(),
        ));
    }
    Ok(())
}

fn shortcut_key(action: ShortcutAction) -> String {
    format!("{SHORTCUT_PREFIX}{}", action.as_str())
}

/// Read every binding over one connection.
///
/// why `&mut SqliteConnection` instead of the pool: `set_shortcut` has to
/// read inside its transaction, and a transaction derefs to a connection —
/// the plain read just checks one out of the pool and passes it in.
async fn load_shortcuts(conn: &mut SqliteConnection) -> Result<Vec<ShortcutBinding>, AppError> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM settings WHERE key LIKE 'shortcut.%'")
            .fetch_all(&mut *conn)
            .await?;
    let stored: HashMap<String, String> = rows.into_iter().collect();
    Ok(ShortcutAction::ALL
        .into_iter()
        .map(|action| {
            let default = action.default_combo();
            let combo = match stored.get(&shortcut_key(action)) {
                None => Some(default.to_string()),
                Some(value) if value.is_empty() => None,
                Some(value) if check_combo(value).is_ok() => Some(value.clone()),
                // A corrupt value costs only this one action its custom binding.
                Some(_) => Some(default.to_string()),
            };
            ShortcutBinding {
                action,
                combo,
                default_combo: default.to_string(),
            }
        })
        .collect())
}

async fn upsert_on(conn: &mut SqliteConnection, key: &str, value: &str) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Every action's current binding, in `ShortcutAction::ALL` order.
pub async fn shortcuts(pool: &SqlitePool) -> Result<Vec<ShortcutBinding>, AppError> {
    let mut conn = pool.acquire().await?;
    load_shortcuts(&mut conn).await
}

/// Bind `action` to `combo` (`None` unbinds it) and return the new bindings.
/// Whichever other action held that combo is left unbound: a combo belongs
/// to at most one action.
pub async fn set_shortcut(
    pool: &SqlitePool,
    action: ShortcutAction,
    combo: Option<String>,
) -> Result<Vec<ShortcutBinding>, AppError> {
    if let Some(combo) = &combo {
        check_combo(combo)?;
    }
    // why the lock: this transaction reads before it writes, and SQLite has a
    // single writer — see storage::WRITE_LOCK.
    let _write = super::WRITE_LOCK.lock().await;
    // why a transaction: the conflict check reads the current bindings and
    // then writes two rows (unbind the old owner, bind the new one). Doing it
    // atomically means a crash halfway can never leave one combo on two
    // actions. Dropping `tx` without commit() on an early `?` rolls back.
    let mut tx = pool.begin().await?;
    if let Some(combo) = &combo {
        for binding in load_shortcuts(&mut tx).await? {
            if binding.action != action && binding.combo.as_ref() == Some(combo) {
                upsert_on(&mut tx, &shortcut_key(binding.action), "").await?;
            }
        }
    }
    let value = combo.as_deref().unwrap_or("");
    upsert_on(&mut tx, &shortcut_key(action), value).await?;
    let bindings = load_shortcuts(&mut tx).await?;
    tx.commit().await?;
    Ok(bindings)
}

/// Forget every custom binding, so each action is back on its default.
pub async fn reset_shortcuts(pool: &SqlitePool) -> Result<(), AppError> {
    sqlx::query("DELETE FROM settings WHERE key LIKE 'shortcut.%'")
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_pool;

    #[tokio::test]
    async fn avatar_lookup_is_off_until_switched_on() {
        let pool = test_pool().await;

        // Off by default: this is the switch that allows calls to hosts other
        // than the user's own mail servers.
        assert!(!avatar_lookup_enabled(&pool).await.unwrap());

        set_avatar_lookup_enabled(&pool, true).await.unwrap();
        assert!(avatar_lookup_enabled(&pool).await.unwrap());

        set_avatar_lookup_enabled(&pool, false).await.unwrap();
        assert!(!avatar_lookup_enabled(&pool).await.unwrap());
    }

    #[tokio::test]
    async fn a_corrupt_avatar_lookup_value_reads_as_off() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO settings (key, value) VALUES ('avatar_lookup', 'yolo')")
            .execute(&pool)
            .await
            .unwrap();

        // why off rather than the stored garbage: an unreadable value must
        // never be the reason the app starts reaching out to the network.
        assert!(!avatar_lookup_enabled(&pool).await.unwrap());
    }

    #[tokio::test]
    async fn llm_summary_is_off_until_switched_on() {
        let pool = test_pool().await;

        assert!(!llm_summary_enabled(&pool).await.unwrap());

        set_llm_summary_enabled(&pool, true).await.unwrap();
        assert!(llm_summary_enabled(&pool).await.unwrap());

        set_llm_summary_enabled(&pool, false).await.unwrap();
        assert!(!llm_summary_enabled(&pool).await.unwrap());
    }

    #[tokio::test]
    async fn llm_translate_is_off_until_switched_on_apart_from_summaries() {
        let pool = test_pool().await;
        set_llm_summary_enabled(&pool, true).await.unwrap();

        assert!(!llm_translate_enabled(&pool).await.unwrap());

        set_llm_translate_enabled(&pool, true).await.unwrap();
        set_llm_summary_enabled(&pool, false).await.unwrap();
        assert!(llm_translate_enabled(&pool).await.unwrap());
    }

    #[tokio::test]
    async fn a_corrupt_llm_summary_value_reads_as_off() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO settings (key, value) VALUES ('llm_summary', 'yolo')")
            .execute(&pool)
            .await
            .unwrap();

        assert!(!llm_summary_enabled(&pool).await.unwrap());
    }

    #[tokio::test]
    async fn llm_model_is_unset_until_picked() {
        let pool = test_pool().await;

        assert_eq!(llm_model(&pool).await.unwrap(), None);

        set_llm_model(&pool, "qwen3.5-2b").await.unwrap();
        assert_eq!(
            llm_model(&pool).await.unwrap(),
            Some("qwen3.5-2b".to_string())
        );
    }

    #[tokio::test]
    async fn an_unknown_llm_model_id_reads_as_unset() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO settings (key, value) VALUES ('llm_model', 'gpt-5')")
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(llm_model(&pool).await.unwrap(), None);
    }

    #[tokio::test]
    async fn llm_summary_language_is_auto_until_picked() {
        let pool = test_pool().await;

        assert_eq!(llm_summary_language(&pool).await.unwrap(), "auto");

        set_llm_summary_language(&pool, " Slovak ").await.unwrap();
        assert_eq!(llm_summary_language(&pool).await.unwrap(), "Slovak");

        set_llm_summary_language(&pool, "").await.unwrap();
        assert_eq!(llm_summary_language(&pool).await.unwrap(), "auto");
    }

    #[tokio::test]
    async fn translate_language_follows_the_summary_one_until_picked() {
        let pool = test_pool().await;

        assert_eq!(llm_translate_language(&pool).await.unwrap(), "English");

        set_llm_summary_language(&pool, "Slovak").await.unwrap();
        assert_eq!(llm_translate_language(&pool).await.unwrap(), "Slovak");

        set_llm_translate_language(&pool, " German ").await.unwrap();
        assert_eq!(llm_translate_language(&pool).await.unwrap(), "German");
    }

    #[tokio::test]
    async fn defaults_to_ask_and_roundtrips() {
        let pool = test_pool().await;

        assert_eq!(
            remote_image_policy(&pool).await.unwrap(),
            RemoteImagePolicy::Ask
        );

        set_remote_image_policy(&pool, RemoteImagePolicy::Always)
            .await
            .unwrap();
        assert_eq!(
            remote_image_policy(&pool).await.unwrap(),
            RemoteImagePolicy::Always
        );

        // Changing it again overwrites instead of duplicating the key.
        set_remote_image_policy(&pool, RemoteImagePolicy::Block)
            .await
            .unwrap();
        assert_eq!(
            remote_image_policy(&pool).await.unwrap(),
            RemoteImagePolicy::Block
        );
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn swipe_actions_default_and_roundtrip() {
        let pool = test_pool().await;

        // Nothing stored yet — the shipped defaults (today's hardcoded gesture).
        assert_eq!(
            swipe_actions(&pool).await.unwrap(),
            SwipeActions {
                left: SwipeAction::Archive,
                right: SwipeAction::ToggleRead,
            }
        );

        let chosen = SwipeActions {
            left: SwipeAction::Trash,
            right: SwipeAction::Reply,
        };
        set_swipe_actions(&pool, chosen).await.unwrap();
        assert_eq!(swipe_actions(&pool).await.unwrap(), chosen);

        // Saving again overwrites the two keys instead of duplicating them.
        let again = SwipeActions {
            left: SwipeAction::None,
            right: SwipeAction::Archive,
        };
        set_swipe_actions(&pool, again).await.unwrap();
        assert_eq!(swipe_actions(&pool).await.unwrap(), again);
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 2);
    }

    #[tokio::test]
    async fn unknown_swipe_value_falls_back_per_side() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO settings (key, value) VALUES ('swipe_left', 'yolo')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO settings (key, value) VALUES ('swipe_right', 'reply')")
            .execute(&pool)
            .await
            .unwrap();

        // The corrupt side falls back to its own default; the valid side sticks.
        assert_eq!(
            swipe_actions(&pool).await.unwrap(),
            SwipeActions {
                left: SwipeAction::Archive,
                right: SwipeAction::Reply,
            }
        );
    }

    #[tokio::test]
    async fn unknown_stored_value_falls_back_to_ask() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO settings (key, value) VALUES ('remote_images', 'yolo')")
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            remote_image_policy(&pool).await.unwrap(),
            RemoteImagePolicy::Ask
        );
    }

    #[tokio::test]
    async fn thread_order_defaults_to_newest_last_and_roundtrips() {
        let pool = test_pool().await;

        assert_eq!(thread_order(&pool).await.unwrap(), ThreadOrder::NewestLast);

        set_thread_order(&pool, ThreadOrder::NewestFirst)
            .await
            .unwrap();
        assert_eq!(thread_order(&pool).await.unwrap(), ThreadOrder::NewestFirst);

        // Changing it again overwrites instead of duplicating the key.
        set_thread_order(&pool, ThreadOrder::NewestLast)
            .await
            .unwrap();
        assert_eq!(thread_order(&pool).await.unwrap(), ThreadOrder::NewestLast);
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn corrupt_thread_order_falls_back_to_newest_last() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO settings (key, value) VALUES ('thread_order', 'yolo')")
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(thread_order(&pool).await.unwrap(), ThreadOrder::NewestLast);
    }

    #[tokio::test]
    async fn appearance_defaults_to_system_and_roundtrips() {
        let pool = test_pool().await;

        assert_eq!(appearance(&pool).await.unwrap(), Appearance::System);

        set_appearance(&pool, Appearance::Dark).await.unwrap();
        assert_eq!(appearance(&pool).await.unwrap(), Appearance::Dark);

        // Changing it again overwrites instead of duplicating the key.
        set_appearance(&pool, Appearance::Light).await.unwrap();
        assert_eq!(appearance(&pool).await.unwrap(), Appearance::Light);
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[tokio::test]
    async fn corrupt_appearance_falls_back_to_system() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO settings (key, value) VALUES ('appearance', 'sepia')")
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(appearance(&pool).await.unwrap(), Appearance::System);
    }

    #[tokio::test]
    async fn date_time_format_defaults_to_the_system_locale_and_roundtrips() {
        let pool = test_pool().await;

        assert_eq!(
            date_time_format(&pool).await.unwrap(),
            DateTimeFormat::default()
        );

        let wanted = DateTimeFormat {
            date: DateFormat::DayMonthYearDot,
            time: TimeFormat::Hour24,
        };
        set_date_time_format(&pool, wanted).await.unwrap();
        assert_eq!(date_time_format(&pool).await.unwrap(), wanted);

        // Saving again overwrites the same two rows instead of adding more.
        set_date_time_format(&pool, wanted).await.unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 2);
    }

    #[tokio::test]
    async fn each_corrupt_format_half_falls_back_on_its_own() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO settings (key, value) VALUES ('date_format', 'yolo')")
            .execute(&pool)
            .await
            .unwrap();
        set_date_time_format(
            &pool,
            DateTimeFormat {
                date: DateFormat::System,
                time: TimeFormat::Hour12,
            },
        )
        .await
        .unwrap();
        sqlx::query("UPDATE settings SET value = 'yolo' WHERE key = 'date_format'")
            .execute(&pool)
            .await
            .unwrap();

        let format = date_time_format(&pool).await.unwrap();

        // The unreadable half reverts, the readable one is kept.
        assert_eq!(format.date, DateFormat::System);
        assert_eq!(format.time, TimeFormat::Hour12);
    }

    #[tokio::test]
    async fn notification_settings_default_to_on_with_system_sound() {
        let pool = test_pool().await;

        let settings = notification_settings(&pool).await.unwrap();

        assert!(settings.enabled);
        assert_eq!(settings.sound, "default");
        assert_eq!(settings.sync_interval_minutes, 3);
        // Push holds an open connection per account — opt-in, never assumed.
        assert!(!settings.push_enabled);
    }

    #[tokio::test]
    async fn notification_settings_roundtrip_without_duplicating_keys() {
        let pool = test_pool().await;
        let wanted = NotificationSettings {
            enabled: false,
            sound: "Ping".to_string(),
            sync_interval_minutes: 15,
            push_enabled: true,
        };

        set_notification_settings(&pool, &wanted).await.unwrap();
        assert_eq!(notification_settings(&pool).await.unwrap(), wanted);

        // Saving again overwrites the same four rows instead of adding more.
        set_notification_settings(&pool, &wanted).await.unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 4);
    }

    #[tokio::test]
    async fn corrupt_notification_values_fall_back_to_defaults() {
        let pool = test_pool().await;
        for (key, value) in [
            ("notifications_enabled", "yolo"),
            ("notification_sound", ""),
            ("sync_interval_minutes", "-5"),
            ("push_enabled", "sure"),
        ] {
            sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?)")
                .bind(key)
                .bind(value)
                .execute(&pool)
                .await
                .unwrap();
        }

        let settings = notification_settings(&pool).await.unwrap();

        assert_eq!(settings, NotificationSettings::default());
    }

    async fn stored(pool: &SqlitePool, key: &str, value: &str) {
        sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?)")
            .bind(key)
            .bind(value)
            .execute(pool)
            .await
            .unwrap();
    }

    fn combo_of(bindings: &[ShortcutBinding], action: ShortcutAction) -> Option<String> {
        bindings
            .iter()
            .find(|binding| binding.action == action)
            .and_then(|binding| binding.combo.clone())
    }

    #[tokio::test]
    async fn shortcuts_default_to_the_shipped_bindings() {
        let pool = test_pool().await;

        let bindings = shortcuts(&pool).await.unwrap();

        assert_eq!(bindings.len(), ShortcutAction::ALL.len());
        for binding in &bindings {
            assert_eq!(
                binding.combo.as_deref(),
                Some(binding.action.default_combo())
            );
            assert_eq!(binding.default_combo, binding.action.default_combo());
        }
        assert_eq!(
            combo_of(&bindings, ShortcutAction::NewMessage).as_deref(),
            Some("Meta+N")
        );
    }

    #[tokio::test]
    async fn a_shortcut_roundtrips_and_can_be_unbound() {
        let pool = test_pool().await;

        let bindings = set_shortcut(&pool, ShortcutAction::Reply, Some("Alt+Meta+R".into()))
            .await
            .unwrap();
        assert_eq!(
            combo_of(&bindings, ShortcutAction::Reply).as_deref(),
            Some("Alt+Meta+R")
        );
        assert_eq!(shortcuts(&pool).await.unwrap(), bindings);

        let bindings = set_shortcut(&pool, ShortcutAction::Reply, None)
            .await
            .unwrap();
        assert_eq!(combo_of(&bindings, ShortcutAction::Reply), None);
        assert_eq!(shortcuts(&pool).await.unwrap(), bindings);
    }

    #[tokio::test]
    async fn taking_another_actions_combo_unbinds_that_action() {
        let pool = test_pool().await;

        // New Message holds Meta+N only by default — no row stored for it.
        let bindings = set_shortcut(&pool, ShortcutAction::Reply, Some("Meta+N".into()))
            .await
            .unwrap();

        assert_eq!(
            combo_of(&bindings, ShortcutAction::Reply).as_deref(),
            Some("Meta+N")
        );
        assert_eq!(combo_of(&bindings, ShortcutAction::NewMessage), None);
        // Everyone else keeps theirs.
        assert_eq!(
            combo_of(&bindings, ShortcutAction::Forward).as_deref(),
            Some("Shift+Meta+F")
        );
        assert_eq!(shortcuts(&pool).await.unwrap(), bindings);
    }

    #[tokio::test]
    async fn a_corrupt_shortcut_falls_back_to_that_actions_default() {
        let pool = test_pool().await;
        stored(&pool, "shortcut.reply", "Meta+Yolo").await;
        stored(&pool, "shortcut.archive", "Meta+Q").await;
        stored(&pool, "shortcut.forward", "Alt+Meta+F").await;
        stored(&pool, "shortcut.trash", "").await;

        let bindings = shortcuts(&pool).await.unwrap();

        assert_eq!(
            combo_of(&bindings, ShortcutAction::Reply).as_deref(),
            Some("Meta+R")
        );
        assert_eq!(
            combo_of(&bindings, ShortcutAction::Archive).as_deref(),
            Some("Ctrl+Meta+A")
        );
        assert_eq!(
            combo_of(&bindings, ShortcutAction::Forward).as_deref(),
            Some("Alt+Meta+F")
        );
        assert_eq!(combo_of(&bindings, ShortcutAction::Trash), None);
    }

    #[tokio::test]
    async fn a_reserved_combo_says_who_owns_it() {
        let pool = test_pool().await;
        let message = |combo: &str| {
            let pool = pool.clone();
            let combo = combo.to_string();
            async move {
                set_shortcut(&pool, ShortcutAction::Reply, Some(combo))
                    .await
                    .unwrap_err()
                    .to_string()
            }
        };

        for combo in ["Meta+C", "Meta+Q", "Ctrl+Meta+F"] {
            assert_eq!(
                message(combo).await,
                "macOS already uses this shortcut.",
                "{combo}"
            );
        }
        for combo in [
            "Meta+Comma",
            "ArrowDown",
            "Shift+ArrowUp",
            "Escape",
            "Meta+Tab",
        ] {
            assert_eq!(
                message(combo).await,
                "Flit already uses this shortcut, and it can't be changed.",
                "{combo}"
            );
        }
    }

    #[tokio::test]
    async fn reserved_and_malformed_combos_are_rejected() {
        let pool = test_pool().await;

        for combo in [
            "Meta+Q",
            "Meta+W",
            "Meta+C",
            "Shift+Meta+Z",
            "Ctrl+Meta+F",
            "Meta+Comma",
            "ArrowDown",
            "Shift+ArrowUp",
            "Escape",
            "Tab",
            "Meta+Tab",
        ] {
            let result = set_shortcut(&pool, ShortcutAction::Reply, Some(combo.into())).await;
            assert!(
                matches!(result, Err(AppError::Invalid(_))),
                "{combo} should be reserved"
            );
        }
        for combo in [
            "",
            "Meta",
            "Meta+",
            "Meta+Ctrl+R",
            "Meta+Meta+R",
            "Meta+r",
            "Meta+Yolo",
            "Hyper+R",
        ] {
            let result = set_shortcut(&pool, ShortcutAction::Reply, Some(combo.into())).await;
            assert!(
                matches!(result, Err(AppError::Invalid(_))),
                "{combo:?} should be malformed"
            );
        }

        // Nothing was written by the rejected attempts.
        assert_eq!(
            combo_of(&shortcuts(&pool).await.unwrap(), ShortcutAction::Reply).as_deref(),
            Some("Meta+R")
        );
    }

    #[tokio::test]
    async fn well_formed_combos_are_accepted() {
        let pool = test_pool().await;

        for combo in [
            "R",
            "Shift+7",
            "Ctrl+Alt+Shift+Meta+F12",
            "Meta+BracketLeft",
            "Alt+Space",
            "Delete",
            "Meta+ArrowLeft",
        ] {
            set_shortcut(&pool, ShortcutAction::Reply, Some(combo.into()))
                .await
                .unwrap_or_else(|err| panic!("{combo} rejected: {err}"));
        }
    }

    #[tokio::test]
    async fn resetting_shortcuts_restores_every_default() {
        let pool = test_pool().await;
        set_shortcut(&pool, ShortcutAction::Reply, Some("Meta+N".into()))
            .await
            .unwrap();
        set_shortcut(&pool, ShortcutAction::Trash, None)
            .await
            .unwrap();
        set_swipe_actions(&pool, SwipeActions::default())
            .await
            .unwrap();

        reset_shortcuts(&pool).await.unwrap();

        for binding in shortcuts(&pool).await.unwrap() {
            assert_eq!(
                binding.combo.as_deref(),
                Some(binding.action.default_combo())
            );
        }
        // Only the shortcut rows go — other settings stay.
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM settings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 2);
    }
}
