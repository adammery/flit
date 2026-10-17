//! Running the local model: one worker thread owns llama.cpp and answers
//! completion requests one at a time.
//!
//! why a dedicated std thread instead of spawn_blocking: llama.cpp's model
//! and context handles are not `Send`, and a loaded model is gigabytes we
//! want to keep warm between requests — a thread that owns them for its
//! whole life is the simplest way to have both. Requests arrive on a
//! channel, so callers never touch the handles; that also serialises
//! inference, which is what a single laptop GPU wants anyway.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::{send_logs_to_tracing, LogOptions};

use crate::error::AppError;

/// Tokens of context per request: prompt plus answer. The prompt builders
/// budget against this (see summarize.rs); the engine only enforces it.
/// why 16k: a 24k-char newsletter in a diacritics-heavy language can run
/// past 8k tokens; the KV cache for 16k is still a few hundred MB.
pub const N_CTX: u32 = 16384;
/// Tokens fed per decode call while reading the prompt.
const N_BATCH: u32 = 512;
/// A loaded model is dropped after this long without a request, giving the
/// memory back — reading mail must not cost gigabytes of RAM all day.
const IDLE_UNLOAD: Duration = Duration::from_secs(5 * 60);
/// Sampling: near-deterministic, with a light repetition penalty — a
/// summary should be faithful, not creative.
const TEMPERATURE: f32 = 0.3;
const TOP_P: f32 = 0.9;
const REPEAT_LAST_N: i32 = 64;
const REPEAT_PENALTY: f32 = 1.1;

/// One turn of a chat prompt; `role` is "system", "user" or "assistant".
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: &'static str,
    pub content: String,
}

/// A raisable stop signal: the generation loop polls it between tokens.
pub type CancelFlag = Arc<AtomicBool>;
/// Receives the answer piece by piece as it is written.
pub type TokenSink = Box<dyn FnMut(&str) + Send>;

/// A completion request: which model file, the conversation so far, and
/// how many tokens the answer may take.
pub struct Request {
    pub model_path: PathBuf,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: usize,
    /// Text placed after the assistant header, as if the model had already
    /// written it. Models with a "thinking" mode take an empty think block
    /// here (see catalog) so the answer starts right away instead of after
    /// hundreds of tokens of deliberation.
    pub assistant_prefix: String,
    /// Called with each piece of the answer as it is written — the text a
    /// reader may see, so a stray think block is held back, never shown.
    pub on_token: Option<TokenSink>,
    /// Raised to stop early; the request then fails as cancelled and
    /// nothing is cached.
    pub cancel: CancelFlag,
}

type Reply = tokio::sync::oneshot::Sender<Result<String, AppError>>;

enum Job {
    Complete(Request, Reply),
    /// Drop the loaded model and answer when done. why it exists: llama.cpp's
    /// Metal backend asserts at process exit if a model is still loaded, and
    /// a static destructor is not a place to free gigabytes — the app calls
    /// this on its way out.
    Unload(tokio::sync::oneshot::Sender<()>),
}

/// Handle to the worker thread. Cheap to keep in AppState: the thread —
/// and llama.cpp's backend — start on the first request, not at app boot.
#[derive(Default)]
pub struct Engine {
    sender: OnceLock<mpsc::Sender<Job>>,
}

impl Engine {
    /// Run one completion; resolves when the answer is complete.
    pub async fn complete(&self, request: Request) -> Result<String, AppError> {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.sender()
            .send(Job::Complete(request, reply))
            .map_err(|_| AppError::Llm("the model worker has stopped".to_string()))?;
        answer
            .await
            .map_err(|_| AppError::Llm("the model worker dropped the request".to_string()))?
    }

    /// Free the loaded model, if any; resolves once it is gone. A no-op
    /// when the worker never started.
    pub async fn unload(&self) {
        let Some(sender) = self.sender.get() else {
            return;
        };
        let (done, gone) = tokio::sync::oneshot::channel();
        if sender.send(Job::Unload(done)).is_ok() {
            let _ = gone.await;
        }
    }

    fn sender(&self) -> &mpsc::Sender<Job> {
        self.sender.get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            // why ignore the spawn error: a thread that could not start
            // drops `receiver`, and every send then fails with a clear
            // "worker has stopped" instead of a panic here.
            let _ = std::thread::Builder::new()
                .name("llm".to_string())
                .spawn(move || worker(receiver));
            sender
        })
    }
}

/// The worker loop: load on demand, answer, unload when idle.
fn worker(receiver: mpsc::Receiver<Job>) {
    // llama.cpp logs every model detail to stderr by default — keep it quiet.
    send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
    let backend = match LlamaBackend::init() {
        Ok(backend) => backend,
        Err(err) => {
            // Fail every request with the reason rather than exit silently.
            while let Ok(job) = receiver.recv() {
                match job {
                    Job::Complete(_, reply) => {
                        let _ = reply
                            .send(Err(AppError::Llm(format!("cannot start llama.cpp: {err}"))));
                    }
                    Job::Unload(done) => {
                        let _ = done.send(());
                    }
                }
            }
            return;
        }
    };
    let mut loaded: Option<Loaded> = None;
    loop {
        match receiver.recv_timeout(IDLE_UNLOAD) {
            Ok(Job::Complete(mut request, reply)) => {
                let result = complete(&backend, &mut loaded, &mut request);
                let _ = reply.send(result);
            }
            Ok(Job::Unload(done)) => {
                loaded = None;
                let _ = done.send(());
            }
            Err(RecvTimeoutError::Timeout) => loaded = None,
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

struct Loaded {
    path: PathBuf,
    model: LlamaModel,
}

fn complete(
    backend: &LlamaBackend,
    loaded: &mut Option<Loaded>,
    request: &mut Request,
) -> Result<String, AppError> {
    let model = load(backend, loaded, &request.model_path)?;

    let template = model
        .chat_template(None)
        .map_err(|e| AppError::Llm(format!("model has no chat template: {e}")))?;
    let messages = request
        .messages
        .iter()
        .map(|m| LlamaChatMessage::new(m.role.to_string(), m.content.clone()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Llm(format!("bad prompt text: {e}")))?;
    let mut prompt = model
        .apply_chat_template(&template, &messages, true)
        .map_err(|e| AppError::Llm(format!("cannot apply chat template: {e}")))?;
    prompt.push_str(&request.assistant_prefix);
    // AddBos::Always means "if this model wants one" — llama.cpp consults
    // the vocabulary, so Gemma gets its <bos> and Qwen stays without.
    let tokens = model
        .str_to_token(&prompt, AddBos::Always)
        .map_err(|e| AppError::Llm(format!("cannot tokenize prompt: {e}")))?;
    if tokens.len() + request.max_tokens > N_CTX as usize {
        return Err(AppError::Llm(format!(
            "prompt of {} tokens does not fit the {N_CTX}-token context",
            tokens.len()
        )));
    }

    let params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(N_CTX))
        .with_n_batch(N_BATCH);
    let mut ctx = model
        .new_context(backend, params)
        .map_err(|e| AppError::Llm(format!("cannot create context: {e}")))?;

    // Feed the prompt in batches; logits are only needed for its last token.
    let mut batch = LlamaBatch::new(N_BATCH as usize, 1);
    let last = tokens.len() - 1;
    for (start, chunk) in tokens
        .chunks(N_BATCH as usize)
        .enumerate()
        .map(|(i, c)| (i * N_BATCH as usize, c))
    {
        batch.clear();
        for (offset, token) in chunk.iter().enumerate() {
            let pos = start + offset;
            batch
                .add(*token, pos as i32, &[0], pos == last)
                .map_err(|e| AppError::Llm(format!("cannot batch prompt: {e}")))?;
        }
        ctx.decode(&mut batch)
            .map_err(|e| AppError::Llm(format!("cannot read prompt: {e}")))?;
    }

    let mut sampler = LlamaSampler::chain_simple([
        LlamaSampler::penalties(model.n_vocab(), REPEAT_LAST_N, REPEAT_PENALTY, 0.0, 0.0),
        LlamaSampler::temp(TEMPERATURE),
        LlamaSampler::top_p(TOP_P, 1),
        LlamaSampler::dist(1234),
    ]);
    let mut decoder = encoding_rs::UTF_8.new_decoder();
    let mut output = String::new();
    // How much of `output` has been handed to on_token so far.
    let mut streamed = 0;
    for pos in (tokens.len()..).take(request.max_tokens) {
        if request.cancel.load(Ordering::Relaxed) {
            return Err(AppError::Invalid("Summary cancelled.".to_string()));
        }
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        sampler.accept(token);
        if model.is_eog_token(token) {
            break;
        }
        let piece = model
            .token_to_piece(token, &mut decoder, false, None)
            .map_err(|e| AppError::Llm(format!("cannot decode token: {e}")))?;
        output.push_str(&piece);
        if let Some(on_token) = request.on_token.as_mut() {
            let visible = streamable(&output);
            if visible.len() > streamed {
                on_token(&visible[streamed..]);
                streamed = visible.len();
            }
        }

        batch.clear();
        batch
            .add(token, pos as i32, &[0], true)
            .map_err(|e| AppError::Llm(format!("cannot batch token: {e}")))?;
        ctx.decode(&mut batch)
            .map_err(|e| AppError::Llm(format!("generation failed: {e}")))?;
    }
    Ok(strip_thinking(&output).trim().to_string())
}

/// The part of a growing answer a reader may see: everything, unless the
/// answer opened with a think block — then only what follows its end, and
/// nothing while the opening tag is still being spelled out.
fn streamable(output: &str) -> &str {
    let trimmed = output.trim_start();
    if !trimmed.is_empty() && "<think>".starts_with(trimmed) {
        return "";
    }
    strip_thinking(output)
}

/// Drop a leading `<think>…</think>` block — a model that deliberated
/// despite the prefill must not have its notes shown as the summary. An
/// unterminated block means it ran out of tokens while thinking: nothing
/// usable was produced.
fn strip_thinking(output: &str) -> &str {
    let trimmed = output.trim_start();
    let Some(rest) = trimmed.strip_prefix("<think>") else {
        return output;
    };
    match rest.find("</think>") {
        Some(end) => &rest[end + "</think>".len()..],
        None => "",
    }
}

/// The model for `path`, loading it — and dropping the previous one first,
/// so two models never sit in memory together — when it is not the one
/// already loaded.
fn load<'a>(
    backend: &LlamaBackend,
    loaded: &'a mut Option<Loaded>,
    path: &PathBuf,
) -> Result<&'a LlamaModel, AppError> {
    if loaded.as_ref().is_none_or(|l| &l.path != path) {
        *loaded = None;
        // All layers on the GPU where there is one (Metal on Apple
        // Silicon); without a GPU backend the number is simply ignored.
        let params = LlamaModelParams::default().with_n_gpu_layers(1000);
        let model = LlamaModel::load_from_file(backend, path, &params)
            .map_err(|e| AppError::Llm(format!("cannot load model: {e}")))?;
        *loaded = Some(Loaded {
            path: path.clone(),
            model,
        });
    }
    Ok(&loaded.as_ref().expect("just loaded").model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_nothing_while_a_think_block_is_open_or_being_typed() {
        assert_eq!(streamable("<th"), "");
        assert_eq!(streamable("<think>"), "");
        assert_eq!(streamable("<think>\nhmm"), "");
        assert_eq!(streamable("<think>\nhmm\n</think>\n\n- A").trim(), "- A");
        assert_eq!(streamable("- A point"), "- A point");
        // A tag that is not a think tag streams as text.
        assert_eq!(streamable("<b>bold"), "<b>bold");
    }

    #[test]
    fn strips_a_leading_think_block() {
        assert_eq!(
            strip_thinking("<think>\nhmm\n</think>\n\nThe answer.").trim(),
            "The answer."
        );
        assert_eq!(strip_thinking("The answer."), "The answer.");
        assert_eq!(strip_thinking("<think>\nstill thinking"), "");
        // Only a leading block is a thinking block; the tag mid-text stays.
        assert_eq!(strip_thinking("A <think> B"), "A <think> B");
    }

    /// One engine for every model test: llama.cpp's backend initialises
    /// once per process, and the app itself only ever has one engine.
    fn test_engine() -> &'static Engine {
        static ENGINE: OnceLock<Engine> = OnceLock::new();
        ENGINE.get_or_init(Engine::default)
    }

    /// Needs a real model file: `FLIT_TEST_MODEL=/path/to/model.gguf cargo
    /// test llm::engine -- --ignored`. Loads it and checks that a trivial
    /// instruction gets the expected answer.
    #[tokio::test]
    #[ignore]
    async fn answers_a_trivial_instruction() {
        let path = std::env::var("FLIT_TEST_MODEL").expect("FLIT_TEST_MODEL not set");
        let engine = test_engine();

        let answer = engine
            .complete(Request {
                model_path: PathBuf::from(path),
                messages: vec![
                    ChatMessage {
                        role: "system",
                        content: "You answer with a single word.".to_string(),
                    },
                    ChatMessage {
                        role: "user",
                        content: "Say the word: pong".to_string(),
                    },
                ],
                max_tokens: 16,
                assistant_prefix: "<think>\n\n</think>\n\n".to_string(),
                on_token: None,
                cancel: CancelFlag::default(),
            })
            .await
            .unwrap();

        assert!(answer.to_lowercase().contains("pong"), "{answer:?}");
        engine.unload().await;
    }

    /// Manual quality probe (prints, asserts only the obvious): the short
    /// invoice mail that once got four invented bullets. Run with
    /// --nocapture and read the answer.
    #[tokio::test]
    #[ignore]
    async fn summarizes_a_short_invoice_mail_without_inventing() {
        use crate::llm::summarize::{message_prompt, Source, AUTO_LANGUAGE};
        let path = std::env::var("FLIT_TEST_MODEL").expect("FLIT_TEST_MODEL not set");
        let source = Source {
            from: "EB Cars <office@eb-cars.example>".to_string(),
            date: "2026-09-04T22:03:00Z".to_string(),
            subject: "FA 08/2026".to_string(),
            text: "Ahoj v prílohe  posielam FA za 08/2026 \n\n--\nS pozdravom,\nDávid Budinský \nkonateľ spoločnosti\n\nEB-Cars, s.r.o.\nKrajná 4/B, 900 29 Nová Dedinka\nIČO: 53596030\nIČ DPH: SK2121417331\n\n ".to_string(),
            attachments: vec!["Faktúra vystavená - 20260158.pdf".to_string()],
        };

        let answer = test_engine()
            .complete(Request {
                model_path: PathBuf::from(path),
                messages: message_prompt(&source, AUTO_LANGUAGE),
                max_tokens: 200,
                assistant_prefix: "<think>\n\n</think>\n\n".to_string(),
                on_token: None,
                cancel: CancelFlag::default(),
            })
            .await
            .unwrap();
        eprintln!("--- invoice mail summary ---\n{answer}\n---");

        assert!(answer.contains("08/2026"), "{answer:?}");
        let bullets = answer
            .lines()
            .filter(|l| l.trim_start().starts_with('-'))
            .count();
        assert!(bullets <= 3, "{answer:?}");
        test_engine().unload().await;
    }

    /// Manual probe on any mail: `FLIT_PROBE_FILE=<body.txt>
    /// FLIT_PROBE_SUBJECT=<subject> FLIT_TEST_MODEL=<model> cargo test
    /// llm::engine::tests::probe_a_mail -- --ignored --nocapture` prints
    /// the summary the current prompt gets for it. `FLIT_PROBE_LANGUAGE`
    /// plays the summary-language setting ("auto" when unset).
    #[tokio::test]
    #[ignore]
    async fn probe_a_mail_from_a_file() {
        use crate::llm::summarize::{
            message_prompt, translation_prompt, Source, AUTO_LANGUAGE, MAX_ANSWER_TOKENS,
        };
        let path = std::env::var("FLIT_TEST_MODEL").expect("FLIT_TEST_MODEL not set");
        let file = std::env::var("FLIT_PROBE_FILE").expect("FLIT_PROBE_FILE not set");
        let source = Source {
            from: "probe <probe@example.com>".to_string(),
            date: String::new(),
            subject: std::env::var("FLIT_PROBE_SUBJECT").unwrap_or_default(),
            text: std::fs::read_to_string(file).unwrap(),
            attachments: Vec::new(),
        };

        let language = std::env::var("FLIT_PROBE_LANGUAGE").unwrap_or(AUTO_LANGUAGE.to_string());
        // FLIT_PROBE_MODE=translate plays the two-pass path: an English
        // summary, then its translation (what the app does for models
        // flagged two_pass_translation).
        let mode = std::env::var("FLIT_PROBE_MODE").unwrap_or_default();
        let request = |messages: Vec<ChatMessage>| Request {
            model_path: PathBuf::from(&path),
            messages,
            max_tokens: MAX_ANSWER_TOKENS,
            assistant_prefix: "<think>\n\n</think>\n\n".to_string(),
            on_token: None,
            cancel: CancelFlag::default(),
        };

        let started = std::time::Instant::now();
        let answer = if mode == "translate" {
            let english = test_engine()
                .complete(request(message_prompt(&source, "English")))
                .await
                .unwrap();
            eprintln!("--- english pass ---\n{english}\n---");
            test_engine()
                .complete(request(translation_prompt(&english, &language)))
                .await
                .unwrap()
        } else {
            test_engine()
                .complete(request(message_prompt(&source, &language)))
                .await
                .unwrap()
        };
        eprintln!(
            "--- probe summary ({:.1}s) ---\n{answer}\n---",
            started.elapsed().as_secs_f32()
        );
        test_engine().unload().await;
    }

    /// Manual probe of in-place translation: `FLIT_PROBE_HTML=<mail.html>
    /// FLIT_PROBE_LANGUAGE=Slovak FLIT_TEST_MODEL=<model> cargo test
    /// llm::engine::tests::probe_translation -- --ignored --nocapture`
    /// prints time and skipped pieces per batch, and writes the rendered
    /// result next to the input as `<mail.html>.translated.html`.
    #[tokio::test]
    #[ignore]
    async fn probe_translation_of_an_html_mail() {
        use crate::llm::translate::{batches, max_tokens, parse, prompt, within_cap, ANSWER_START};
        use crate::mail::{sanitize, translate as markup};
        let path = std::env::var("FLIT_TEST_MODEL").expect("FLIT_TEST_MODEL not set");
        let file = std::env::var("FLIT_PROBE_HTML").expect("FLIT_PROBE_HTML not set");
        let language = std::env::var("FLIT_PROBE_LANGUAGE").unwrap_or("Slovak".to_string());
        let html = std::fs::read_to_string(&file).unwrap();
        let texts = markup::segments(&sanitize::sanitize_fragment(&html, &[]));
        let texts = within_cap(&texts);

        let started = std::time::Instant::now();
        let mut translations = std::collections::HashMap::new();
        for range in batches(texts) {
            let batch: Vec<&str> = texts[range.clone()].iter().map(String::as_str).collect();
            let chars: usize = batch.iter().map(|t| t.chars().count()).sum();
            let at = std::time::Instant::now();
            let answer = test_engine()
                .complete(Request {
                    model_path: PathBuf::from(&path),
                    messages: prompt(&batch, &language),
                    max_tokens: max_tokens(&batch),
                    assistant_prefix: format!("<think>\n\n</think>\n\n{ANSWER_START}"),
                    on_token: None,
                    cancel: CancelFlag::default(),
                })
                .await
                .unwrap();
            let parsed = parse(&format!("{ANSWER_START}{answer}"), batch.len());
            let missing = parsed.iter().filter(|p| p.is_none()).count();
            eprintln!(
                "batch {range:?}: {} pieces, {chars} chars, {:.1}s, {missing} skipped",
                batch.len(),
                at.elapsed().as_secs_f32()
            );
            for (source, translation) in batch.iter().zip(parsed) {
                if let Some(translation) = translation {
                    eprintln!("  {source}\n  → {translation}");
                    translations.insert(source.to_string(), translation);
                }
            }
        }
        // Markers: did every marked piece get all its markers back?
        let markers = |text: &str| {
            let mut found: Vec<String> = text
                .split('⟦')
                .skip(1)
                .filter_map(|rest| rest.split_once('⟧').map(|(m, _)| m.trim().to_string()))
                .collect();
            found.sort();
            found
        };
        let marked: Vec<&String> = texts.iter().filter(|t| t.contains('⟦')).collect();
        let kept = marked
            .iter()
            .filter(|t| {
                translations
                    .get(**t)
                    .is_some_and(|tr| markers(tr) == markers(t))
            })
            .count();
        eprintln!(
            "--- {}/{} pieces in {:.1}s; markers kept in {kept}/{} marked pieces ---",
            translations.len(),
            texts.len(),
            started.elapsed().as_secs_f32(),
            marked.len()
        );
        let doc = sanitize::build_srcdoc(
            &html,
            &[],
            &std::collections::HashMap::new(),
            false,
            Some(&translations),
        );
        std::fs::write(format!("{file}.translated.html"), doc.html).unwrap();
        test_engine().unload().await;
    }

    /// Same setup as above; checks that the streamed pieces add up to the
    /// returned answer and that a raised flag stops generation.
    #[tokio::test]
    #[ignore]
    async fn streams_pieces_and_honours_cancel() {
        let path = std::env::var("FLIT_TEST_MODEL").expect("FLIT_TEST_MODEL not set");
        let engine = test_engine();
        let pieces = Arc::new(std::sync::Mutex::new(String::new()));
        let sink = pieces.clone();
        let request = |on_token: Option<TokenSink>, cancel: CancelFlag| Request {
            model_path: PathBuf::from(&path),
            messages: vec![ChatMessage {
                role: "user",
                content: "Count from one to twenty in words, comma separated.".to_string(),
            }],
            max_tokens: 64,
            assistant_prefix: "<think>\n\n</think>\n\n".to_string(),
            on_token,
            cancel,
        };

        let answer = engine
            .complete(request(
                Some(Box::new(move |piece| sink.lock().unwrap().push_str(piece))),
                CancelFlag::default(),
            ))
            .await
            .unwrap();
        assert_eq!(pieces.lock().unwrap().trim(), answer);

        let cancel = CancelFlag::default();
        let flag = cancel.clone();
        let err = engine
            .complete(request(
                Some(Box::new(move |_| flag.store(true, Ordering::Relaxed))),
                cancel,
            ))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("cancelled"), "{err}");
        engine.unload().await;
    }

    #[tokio::test]
    async fn unload_before_any_request_is_a_no_op() {
        // Must not start the worker (nor llama.cpp) just to unload nothing.
        let engine = Engine::default();
        engine.unload().await;
        assert!(engine.sender.get().is_none());
    }
}
