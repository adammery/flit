//! Experimental on-device summaries: which models the user may pick, where
//! their files live, and (later) the engine that runs them.
//!
//! PRIVACY: nothing here ever sends message content anywhere. The only
//! network use in this module is fetching a model file the user explicitly
//! chose and clicked to download — see CLAUDE.md's hard rules.

pub mod catalog;
pub mod download;
pub mod engine;
pub mod store;
pub mod summarize;
pub mod translate;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::AppError;
use crate::models::{
    LlmDownloadProgress, LlmModel, LlmModelState, LlmStatus, MessageHeader, SummaryToken,
    TranslationProgress,
};
use crate::state::AppState;
use crate::storage::{self, settings};

/// Progress events are throttled to this — the UI needs a moving bar, not
/// one event per network chunk.
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

/// The current state of the feature, for Settings and the main window.
pub async fn status(app: &AppHandle, pool: &SqlitePool) -> Result<LlmStatus, AppError> {
    let enabled = settings::llm_summary_enabled(pool).await?;
    let translate_enabled = settings::llm_translate_enabled(pool).await?;
    let translate_language = settings::llm_translate_language(pool).await?;
    let active = settings::llm_model(pool).await?;
    let downloads = app.state::<AppState>().llm_downloads.snapshot();
    Ok(describe(
        &store::models_dir(app)?,
        enabled,
        translate_enabled,
        translate_language,
        active,
        &downloads,
    ))
}

/// Assemble the status from its inputs — kept free of app handles and the
/// database so it can be tested against a scratch directory.
fn describe(
    dir: &Path,
    enabled: bool,
    translate_enabled: bool,
    translate_language: String,
    active_model: Option<String>,
    downloads: &HashMap<String, download::Snapshot>,
) -> LlmStatus {
    let models: Vec<LlmModel> = catalog::MODELS
        .iter()
        .map(|spec| LlmModel {
            id: spec.id.to_string(),
            name: spec.name.to_string(),
            description: spec.description.to_string(),
            size: spec.size,
            recommended: spec.recommended,
            state: match downloads.get(spec.id) {
                Some(download::Snapshot {
                    error: Some(error), ..
                }) => LlmModelState::Failed {
                    error: error.clone(),
                },
                Some(snapshot) => LlmModelState::Downloading {
                    received: snapshot.received,
                    total: snapshot.total,
                },
                None if store::is_ready(dir, spec) => LlmModelState::Ready,
                None => LlmModelState::Missing,
            },
        })
        .collect();
    let model_ready = active_model
        .as_deref()
        .and_then(catalog::find)
        .is_some_and(|spec| store::is_ready(dir, spec));
    LlmStatus {
        enabled,
        translate_enabled,
        active_model,
        ready: enabled && model_ready,
        translate_ready: translate_enabled && model_ready,
        translate_language,
        models,
    }
}

/// Summaries being written, keyed by the frontend's request id, so a
/// cancel command can reach the right generation.
#[derive(Default)]
pub struct Summaries {
    inner: Mutex<HashMap<String, engine::CancelFlag>>,
}

impl Summaries {
    fn begin(&self, request_id: &str) -> engine::CancelFlag {
        let cancel = engine::CancelFlag::default();
        self.lock().insert(request_id.to_string(), cancel.clone());
        cancel
    }

    fn finish(&self, request_id: &str) {
        self.lock().remove(request_id);
    }

    /// Raise the request's stop flag; false when nothing runs under that id
    /// (already finished, or a cache hit that never ran).
    pub fn cancel(&self, request_id: &str) -> bool {
        match self.lock().get(request_id) {
            Some(flag) => {
                flag.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    // why unwrap_or_else(into_inner): a poisoned lock only means a task
    // panicked mid-update; the map is still coherent.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, engine::CancelFlag>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Run one generation under `request_id`: with `stream`, pieces go out as
/// `summary-token` events; a cancel command reaches it through the
/// registry, and the entry is dropped however it ends.
async fn generate(
    app: &AppHandle,
    request_id: &str,
    model_path: &std::path::Path,
    messages: Vec<engine::ChatMessage>,
    max_tokens: usize,
    assistant_prefix: &str,
    stream: bool,
) -> Result<String, AppError> {
    let state = app.state::<AppState>();
    let cancel = state.llm_summaries.begin(request_id);
    let sink: Option<engine::TokenSink> = stream.then(|| {
        let app = app.clone();
        let request_id = request_id.to_string();
        Box::new(move |piece: &str| {
            let _ = app.emit(
                "summary-token",
                SummaryToken {
                    request_id: request_id.clone(),
                    text: piece.to_string(),
                },
            );
        }) as engine::TokenSink
    });
    let result = state
        .llm_engine
        .complete(engine::Request {
            model_path: model_path.to_path_buf(),
            messages,
            max_tokens,
            assistant_prefix: assistant_prefix.to_string(),
            on_token: sink,
            cancel,
        })
        .await;
    state.llm_summaries.finish(request_id);
    result
}

/// The summary itself: one call in the target language, or — for models
/// flagged for it, when the target is not English — an English summary
/// followed by a translation. `prompt_for` builds the summary prompt for a
/// given language name (or the auto setting); `detect_from` is the text
/// the language is detected from.
async fn summarize_with(
    app: &AppHandle,
    request_id: &str,
    picked: &Picked,
    language_setting: &str,
    detect_from: &str,
    max_tokens: usize,
    prompt_for: impl Fn(&str) -> Vec<engine::ChatMessage>,
) -> Result<String, AppError> {
    let Picked {
        spec,
        path: model_path,
    } = picked;
    let target = summarize::resolve_language(language_setting, detect_from);
    let two_pass = spec.two_pass_translation && target.as_deref().is_some_and(|l| l != "English");
    if !two_pass {
        return generate(
            app,
            request_id,
            model_path,
            prompt_for(language_setting),
            max_tokens,
            spec.assistant_prefix,
            true,
        )
        .await;
    }
    let target = target.unwrap_or_default();
    // The English pass stays silent — the reader would see one language
    // replaced by another mid-way; only the translation streams.
    let english = generate(
        app,
        request_id,
        model_path,
        prompt_for("English"),
        max_tokens,
        spec.assistant_prefix,
        false,
    )
    .await?;
    generate(
        app,
        request_id,
        model_path,
        summarize::translation_prompt(&english, &target),
        max_tokens,
        spec.assistant_prefix,
        true,
    )
    .await
}

/// The picked model and where its file is.
struct Picked {
    spec: &'static catalog::ModelSpec,
    path: std::path::PathBuf,
}

/// Which experimental feature wants the model — each has its own switch.
#[derive(Debug, Clone, Copy)]
enum Feature {
    Summaries,
    Translation,
}

/// The picked model, only when the feature is on and its file is here;
/// otherwise an error that reads as guidance for the user.
async fn ready_model(
    app: &AppHandle,
    pool: &SqlitePool,
    feature: Feature,
) -> Result<Picked, AppError> {
    let switched_on = match feature {
        Feature::Summaries => settings::llm_summary_enabled(pool).await?,
        Feature::Translation => settings::llm_translate_enabled(pool).await?,
    };
    if !switched_on {
        let name = match feature {
            Feature::Summaries => "Summaries are",
            Feature::Translation => "Translation is",
        };
        return Err(AppError::Invalid(format!(
            "{name} switched off — see Settings → Experimental."
        )));
    }
    let spec = settings::llm_model(pool)
        .await?
        .as_deref()
        .and_then(catalog::find)
        .ok_or_else(|| {
            AppError::Invalid("Pick and download a model in Settings → Experimental.".to_string())
        })?;
    let dir = store::models_dir(app)?;
    if !store::is_ready(&dir, spec) {
        return Err(AppError::Invalid(format!(
            "{} is not downloaded — see Settings → Experimental.",
            spec.name
        )));
    }
    Ok(Picked {
        spec,
        path: store::model_path(&dir, spec),
    })
}

/// File names of the message's attachments, for the prompt.
async fn attachment_names(pool: &SqlitePool, message_id: i64) -> Result<Vec<String>, AppError> {
    Ok(storage::messages::attachments(pool, message_id)
        .await?
        .into_iter()
        .map(|a| a.filename)
        .collect())
}

/// Cache key of one message's summary under the current model and language.
fn message_key(
    spec: &catalog::ModelSpec,
    language: &str,
    header: &MessageHeader,
    text: &str,
) -> String {
    let id = header.id.to_string();
    summarize::cache_key("message", spec.id, language, [id.as_str(), text])
}

/// Cache key of a conversation's summary: every member's id and text, in
/// order, so a conversation that gained a message hashes differently.
fn thread_key(
    spec: &catalog::ModelSpec,
    language: &str,
    messages: &[(MessageHeader, String)],
) -> String {
    let ids: Vec<String> = messages.iter().map(|(h, _)| h.id.to_string()).collect();
    let parts = messages
        .iter()
        .zip(&ids)
        .flat_map(|((_, text), id)| [id.as_str(), text.as_str()]);
    summarize::cache_key("thread", spec.id, language, parts)
}

/// A summary already written for this message, if any — nothing is
/// generated. `None` also when the feature is off or no model is picked,
/// so an opened card can simply ask.
pub async fn cached_message_summary(
    app: &AppHandle,
    header: &MessageHeader,
    body_text: &str,
) -> Result<Option<String>, AppError> {
    let state = app.state::<AppState>();
    let Ok(picked) = ready_model(app, &state.pool, Feature::Summaries).await else {
        return Ok(None);
    };
    let language = settings::llm_summary_language(&state.pool).await?;
    storage::summaries::get(
        &state.pool,
        &message_key(picked.spec, &language, header, body_text),
    )
    .await
}

/// The conversation's already-written summary, if any (see
/// `cached_message_summary`).
pub async fn cached_thread_summary(
    app: &AppHandle,
    messages: &[(MessageHeader, String)],
) -> Result<Option<String>, AppError> {
    let state = app.state::<AppState>();
    let Ok(picked) = ready_model(app, &state.pool, Feature::Summaries).await else {
        return Ok(None);
    };
    let language = settings::llm_summary_language(&state.pool).await?;
    storage::summaries::get(&state.pool, &thread_key(picked.spec, &language, messages)).await
}

/// Summarize one message with the picked model. `body_text` is the raw
/// stored text; the prompt builder trims it. A cached summary of the same
/// inputs is returned unless `fresh` asks for a new one.
pub async fn summarize_message(
    app: &AppHandle,
    header: &MessageHeader,
    body_text: &str,
    fresh: bool,
    request_id: &str,
) -> Result<String, AppError> {
    let state = app.state::<AppState>();
    let picked = ready_model(app, &state.pool, Feature::Summaries).await?;
    if summarize::prepare_text(body_text, summarize::MESSAGE_CHARS).is_empty() {
        return Err(AppError::Invalid(
            "This message has no text to summarize.".to_string(),
        ));
    }
    let language = settings::llm_summary_language(&state.pool).await?;
    let key = message_key(picked.spec, &language, header, body_text);
    if !fresh {
        if let Some(cached) = storage::summaries::get(&state.pool, &key).await? {
            return Ok(cached);
        }
    }
    let source = summarize::Source {
        from: header.from.clone(),
        date: header.date.clone(),
        subject: header.subject.clone(),
        text: body_text.to_string(),
        attachments: attachment_names(&state.pool, header.id).await?,
    };
    let prepared = summarize::prepare_text(body_text, summarize::MESSAGE_CHARS);
    let text = summarize_with(
        app,
        request_id,
        &picked,
        &language,
        &prepared,
        summarize::MAX_ANSWER_TOKENS,
        |lang| summarize::message_prompt(&source, lang),
    )
    .await?;
    storage::summaries::put(&state.pool, &key, &text).await?;
    Ok(text)
}

/// Summarize a whole conversation, `messages` oldest first as
/// `(header, stored body text)`. Cached like `summarize_message`; a
/// conversation that gained a message hashes differently and is redone.
pub async fn summarize_thread(
    app: &AppHandle,
    messages: &[(MessageHeader, String)],
    fresh: bool,
    request_id: &str,
) -> Result<String, AppError> {
    let state = app.state::<AppState>();
    let picked = ready_model(app, &state.pool, Feature::Summaries).await?;
    let mut sources = Vec::with_capacity(messages.len());
    for (header, text) in messages {
        if summarize::prepare_text(text, summarize::MESSAGE_CHARS).is_empty() {
            continue;
        }
        sources.push(summarize::Source {
            from: header.from.clone(),
            date: header.date.clone(),
            subject: header.subject.clone(),
            text: text.clone(),
            attachments: attachment_names(&state.pool, header.id).await?,
        });
    }
    if sources.is_empty() {
        return Err(AppError::Invalid(
            "This conversation has no text to summarize.".to_string(),
        ));
    }
    let language = settings::llm_summary_language(&state.pool).await?;
    let key = thread_key(picked.spec, &language, messages);
    if !fresh {
        if let Some(cached) = storage::summaries::get(&state.pool, &key).await? {
            return Ok(cached);
        }
    }
    let all_text: String = sources
        .iter()
        .map(|s| summarize::prepare_text(&s.text, summarize::MESSAGE_CHARS))
        .collect::<Vec<_>>()
        .join("\n");
    let text = summarize_with(
        app,
        request_id,
        &picked,
        &language,
        &all_text,
        summarize::MAX_THREAD_ANSWER_TOKENS,
        |lang| summarize::thread_prompt(&sources, lang),
    )
    .await?;
    storage::summaries::put(&state.pool, &key, &text).await?;
    Ok(text)
}

/// Translate a message's pieces of text (see mail::translate) into
/// `language` with the picked model: source text → translation, for every
/// piece the model answered. A piece it skipped twice, and everything past
/// `translate::MAX_CHARS`, stays as it is. Cached like summaries; progress
/// goes out as `translation-progress` events as pieces finish, cancel_summary
/// stops it under `request_id`, and `on_batch` gets the translations so far
/// after every batch — the caller shows the body growing (a cache hit
/// skips straight to the result).
pub async fn translate_texts(
    app: &AppHandle,
    message_id: i64,
    texts: &[String],
    language: &str,
    request_id: &str,
    mut on_batch: impl FnMut(&HashMap<String, String>),
) -> Result<HashMap<String, String>, AppError> {
    let state = app.state::<AppState>();
    let picked = ready_model(app, &state.pool, Feature::Translation).await?;
    let texts = translate::within_cap(texts);
    let id = message_id.to_string();
    let parts = [id.as_str(), translate::RULES]
        .into_iter()
        .chain(texts.iter().map(String::as_str));
    let key = summarize::cache_key("translation", picked.spec.id, language, parts);
    if let Some(cached) = storage::summaries::get(&state.pool, &key).await? {
        // why ignore a parse failure: the row is only a cache — redo it.
        if let Ok(translations) = serde_json::from_str(&cached) {
            return Ok(translations);
        }
    }
    let cancel = state.llm_summaries.begin(request_id);
    let result = translate_batches(
        app,
        &picked,
        texts,
        language,
        request_id,
        &cancel,
        &mut on_batch,
    )
    .await;
    state.llm_summaries.finish(request_id);
    let translations = result?;
    if let Ok(json) = serde_json::to_string(&translations) {
        storage::summaries::put(&state.pool, &key, &json).await?;
    }
    Ok(translations)
}

/// Every batch through the model, then once more for the pieces it skipped
/// — a small model now and then drops a line of a numbered list.
async fn translate_batches(
    app: &AppHandle,
    picked: &Picked,
    texts: &[String],
    language: &str,
    request_id: &str,
    cancel: &engine::CancelFlag,
    on_batch: &mut impl FnMut(&HashMap<String, String>),
) -> Result<HashMap<String, String>, AppError> {
    let engine = &app.state::<AppState>().llm_engine;
    let mut translations = HashMap::new();
    let mut pending = texts.to_vec();
    // The count shows from the start, not only once the first batch is in
    // — loading the model alone takes seconds.
    emit_translation_progress(app, request_id, 0, texts.len());
    for _pass in 0..2 {
        let mut skipped = Vec::new();
        for range in translate::batches(&pending) {
            let batch: Vec<&str> = pending[range].iter().map(String::as_str).collect();
            // why a sink: the count moves with every piece the model
            // finishes, not once per batch of 20 — the reader sees work.
            let sink: engine::TokenSink = {
                let app = app.clone();
                let request_id = request_id.to_string();
                let (base, total, count) = (translations.len(), texts.len(), batch.len());
                let mut streamed = String::new();
                let mut shown = 0;
                Box::new(move |piece: &str| {
                    streamed.push_str(piece);
                    let finished = translate::finished_so_far(&streamed, count);
                    if finished > shown {
                        shown = finished;
                        emit_translation_progress(&app, &request_id, base + finished, total);
                    }
                })
            };
            let answer = engine
                .complete(engine::Request {
                    model_path: picked.path.clone(),
                    messages: translate::prompt(&batch, language),
                    max_tokens: translate::max_tokens(&batch),
                    assistant_prefix: format!(
                        "{}{}",
                        picked.spec.assistant_prefix,
                        translate::ANSWER_START
                    ),
                    on_token: Some(sink),
                    cancel: cancel.clone(),
                })
                .await?;
            let answer = format!("{}{answer}", translate::ANSWER_START);
            for (source, translation) in batch.iter().zip(translate::parse(&answer, &batch)) {
                match translation {
                    Some(translation) => {
                        translations.insert(source.to_string(), translation);
                    }
                    None => skipped.push(source.to_string()),
                }
            }
            emit_translation_progress(app, request_id, translations.len(), texts.len());
            on_batch(&translations);
        }
        if skipped.is_empty() {
            break;
        }
        pending = skipped;
    }
    Ok(translations)
}

fn emit_translation_progress(app: &AppHandle, request_id: &str, done: usize, total: usize) {
    let _ = app.emit(
        "translation-progress",
        TranslationProgress {
            request_id: request_id.to_string(),
            done,
            total,
        },
    );
}

/// Start fetching a model in the background; returns as soon as the task is
/// spawned. Progress arrives as `llm-download-progress` events and the end —
/// done, cancelled or failed — as `llm-models-changed`.
pub fn start_download(app: AppHandle, id: &str) -> Result<(), AppError> {
    let spec =
        catalog::find(id).ok_or_else(|| AppError::Invalid(format!("Unknown model: {id}")))?;
    let dir = store::models_dir(&app)?;
    if store::is_ready(&dir, spec) {
        return Ok(());
    }
    let Some(cancel) = app
        .state::<AppState>()
        .llm_downloads
        .begin(spec.id, spec.size)
    else {
        return Ok(()); // already running — the running task covers it
    };
    let client = download::client()?;

    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let mut last_emit = Instant::now();
        let result = download::fetch(&client, spec, &dir, &cancel, |received| {
            state.llm_downloads.progress(spec.id, received);
            if last_emit.elapsed() >= PROGRESS_EVERY {
                last_emit = Instant::now();
                let _ = app.emit(
                    "llm-download-progress",
                    LlmDownloadProgress {
                        id: spec.id.to_string(),
                        received,
                        total: spec.size,
                    },
                );
            }
        })
        .await;

        match result {
            Ok(download::Outcome::Done) => {
                state.llm_downloads.finish(spec.id);
                // The first model to arrive becomes the pick — the user
                // downloaded it to use it, and an explicit choice is one
                // radio click away.
                if let Ok(None) = settings::llm_model(&state.pool).await {
                    let _ = settings::set_llm_model(&state.pool, spec.id).await;
                }
            }
            Ok(download::Outcome::Cancelled) => state.llm_downloads.finish(spec.id),
            Err(err) => state.llm_downloads.fail(spec.id, err.to_string()),
        }
        let _ = app.emit("llm-models-changed", ());
    });
    Ok(())
}

/// Delete a downloaded model file (or dismiss a failed download). A
/// download still running must be cancelled first.
pub async fn remove_model(app: &AppHandle, id: &str) -> Result<(), AppError> {
    let spec =
        catalog::find(id).ok_or_else(|| AppError::Invalid(format!("Unknown model: {id}")))?;
    let downloads = &app.state::<AppState>().llm_downloads;
    if downloads.is_running(spec.id) {
        return Err(AppError::Invalid(
            "This model is still downloading — cancel the download first.".to_string(),
        ));
    }
    downloads.finish(spec.id);
    let path = store::model_path(&store::models_dir(app)?, spec);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    app.emit("llm-models-changed", ())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn cancel_reaches_only_a_running_request() {
        let summaries = Summaries::default();
        let flag = summaries.begin("r1");

        assert!(!summaries.cancel("r2"));
        assert!(!flag.load(Ordering::Relaxed));
        assert!(summaries.cancel("r1"));
        assert!(flag.load(Ordering::Relaxed));

        summaries.finish("r1");
        assert!(!summaries.cancel("r1"));
    }

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flit-llm-status-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Fake a complete download: a file of exactly the pinned size.
    fn fake_download(dir: &Path, id: &str) {
        let spec = catalog::find(id).unwrap();
        let file = std::fs::File::create(store::model_path(dir, spec)).unwrap();
        file.set_len(spec.size).unwrap();
    }

    #[test]
    fn lists_every_catalog_model_as_missing_on_a_fresh_machine() {
        let status = describe(
            &scratch_dir("fresh"),
            true,
            false,
            "English".into(),
            None,
            &HashMap::new(),
        );

        assert_eq!(status.models.len(), catalog::MODELS.len());
        assert!(status
            .models
            .iter()
            .all(|m| m.state == LlmModelState::Missing));
        assert!(!status.ready);
    }

    #[test]
    fn ready_needs_the_switch_and_the_picked_models_file() {
        let dir = scratch_dir("ready");
        fake_download(&dir, "qwen3.5-2b");

        let none = HashMap::new();
        // File present but the feature is off.
        assert!(
            !describe(
                &dir,
                false,
                false,
                "English".into(),
                Some("qwen3.5-2b".into()),
                &none
            )
            .ready
        );
        // On, but nothing picked.
        assert!(!describe(&dir, true, false, "English".into(), None, &none).ready);
        // On, but the pick is a model that is not downloaded.
        assert!(
            !describe(
                &dir,
                true,
                false,
                "English".into(),
                Some("qwen3.5-4b".into()),
                &none
            )
            .ready
        );

        let status = describe(
            &dir,
            true,
            false,
            "English".into(),
            Some("qwen3.5-2b".into()),
            &none,
        );
        assert!(status.ready);
        assert_eq!(
            status
                .models
                .iter()
                .find(|m| m.id == "qwen3.5-2b")
                .map(|m| m.state.clone()),
            Some(LlmModelState::Ready)
        );
    }

    #[test]
    fn translation_is_ready_on_its_own_switch() {
        let dir = scratch_dir("translate");
        fake_download(&dir, "qwen3.5-2b");
        let none = HashMap::new();

        let status = describe(
            &dir,
            false,
            true,
            "English".into(),
            Some("qwen3.5-2b".into()),
            &none,
        );
        assert!(status.translate_ready);
        assert!(!status.ready);

        let status = describe(
            &dir,
            true,
            false,
            "English".into(),
            Some("qwen3.5-2b".into()),
            &none,
        );
        assert!(!status.translate_ready);
        assert!(status.ready);
    }

    #[test]
    fn a_download_in_flight_or_failed_shows_as_such() {
        let dir = scratch_dir("downloading");
        let mut downloads = HashMap::new();
        downloads.insert(
            "qwen3.5-2b".to_string(),
            download::Snapshot {
                received: 5,
                total: 10,
                error: None,
            },
        );
        downloads.insert(
            "qwen3.5-4b".to_string(),
            download::Snapshot {
                received: 3,
                total: 10,
                error: Some("boom".to_string()),
            },
        );

        let status = describe(
            &dir,
            true,
            false,
            "English".into(),
            Some("qwen3.5-2b".into()),
            &downloads,
        );

        let state_of = |id: &str| {
            status
                .models
                .iter()
                .find(|m| m.id == id)
                .map(|m| m.state.clone())
                .unwrap()
        };
        assert_eq!(
            state_of("qwen3.5-2b"),
            LlmModelState::Downloading {
                received: 5,
                total: 10
            }
        );
        assert_eq!(
            state_of("qwen3.5-4b"),
            LlmModelState::Failed {
                error: "boom".to_string()
            }
        );
        assert_eq!(state_of("gemma-4-e2b"), LlmModelState::Missing);
        // Picked but still downloading is not ready.
        assert!(!status.ready);
    }
}
