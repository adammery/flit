pub mod auth;
mod commands;
pub mod error;
mod idle;
pub mod llm;
pub mod mail;
mod models;
mod notify;
mod poller;
mod scheduler;
pub mod state;
pub mod storage;
pub mod timing;
mod wake;

use tauri::{Manager, Url};

use crate::state::AppState;

/// Should a new-window request from the webview be handed to the user's
/// default browser? Only web links are: the URL comes from a message, so its
/// scheme is untrusted input, and handing an arbitrary one to the OS would let
/// a sender choose which application launches.
fn opens_in_browser(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // why first: before any task below can read a password at the
            // same time as another (see auth::init_keychain). Not fatal —
            // without a store every read fails and says so per account.
            if let Err(err) = auth::init_keychain() {
                eprintln!("keychain store unavailable: {err}");
            }
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            // why: .setup() is synchronous, so block_on finishes the async DB
            // init (open + migrate) before any command can fire — commands may
            // then assume the pool always exists in state.
            let pool = tauri::async_runtime::block_on(storage::init(&data_dir.join("flit.db")))?;

            // why before any window is built: the main window is created
            // further down, so it opens already in the stored scheme instead
            // of flashing the system one first.
            let appearance = tauri::async_runtime::block_on(storage::settings::appearance(&pool))?;
            app.handle().set_theme(commands::theme_for(appearance));

            // why spawned instead of awaited in here: these are one-off
            // repairs of data that is already cached, so nothing on screen
            // waits for them — while block_on'ing them cost 1.6 s of a 1.7 s
            // cold start on a real mailbox, every launch. See run_maintenance.
            let maintenance = pool.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = storage::run_maintenance(&maintenance).await {
                    eprintln!("database maintenance failed: {err}");
                }
            });

            app.manage(AppState::new(pool));

            // SECURITY (message-body rendering): the main window is built here
            // instead of by tauri.conf.json ("create": false) because only the
            // builder can carry a new-window handler — and that handler is how
            // message-body links reach the browser. Bodies render with
            // `<base target="_blank">` in a frame whose sandbox blocks both
            // navigation and script (WebKit runs no listener there at all, see
            // mail::sanitize), so a link click surfaces as a new-window
            // request. Every one of them is DENIED — no second webview is ever
            // created, nothing remote loads inside the app — and a plain
            // http(s) link is handed to the user's default browser instead.
            let window_config = app
                .config()
                .app
                .windows
                .iter()
                .find(|window| window.label == "main")
                .cloned()
                .ok_or("no main window in tauri.conf.json")?;
            tauri::WebviewWindowBuilder::from_config(app.handle(), &window_config)?
                .on_new_window(|url, _features| {
                    if opens_in_browser(&url) {
                        if let Err(err) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>)
                        {
                            eprintln!("failed to open {url} in the default browser: {err}");
                        }
                    }
                    tauri::webview::NewWindowResponse::Deny
                })
                .build()?;

            // The send-later scheduler: delivers parked messages when their
            // time comes; runs for the whole life of the app.
            scheduler::spawn(app.handle().clone());

            // The background new-mail poll — syncs accounts on the interval
            // configured in Settings › Notifications so notifications work
            // while the app idles.
            poller::spawn(app.handle().clone());

            // IMAP IDLE listeners, when push is on: an open connection per
            // account so new inbox mail arrives as the server announces it.
            // The poll above keeps running — under push its interval covers
            // the folders IDLE cannot watch.
            idle::spawn(app.handle().clone());

            // why: macOS convention puts "Settings…" (⌘,) in the app menu; we
            // extend Tauri's default menu instead of rebuilding it from
            // scratch so all standard items (Edit, Window, …) stay intact.
            #[cfg(target_os = "macos")]
            {
                use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem};

                let menu = Menu::default(app.handle())?;
                if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.first() {
                    // why: position 2 = right after "About" and its
                    // separator, where the HIG places Settings.
                    app_menu.insert_items(
                        &[
                            &MenuItem::with_id(app, "settings", "Settings…", true, Some("Cmd+,"))?,
                            &PredefinedMenuItem::separator(app)?,
                        ],
                        2,
                    )?;
                }
                app.set_menu(menu)?;
                app.on_menu_event(|app, event| {
                    if event.id() == "settings" {
                        let app = app.clone();
                        // why: menu handlers are sync — spawn the async
                        // window-opening command instead of blocking here.
                        tauri::async_runtime::spawn(async move {
                            if let Err(err) = commands::open_settings(app).await {
                                eprintln!("failed to open settings window: {err}");
                            }
                        });
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_accounts,
            commands::add_account,
            commands::oauth_providers,
            commands::add_oauth_account,
            commands::cancel_oauth_sign_in,
            commands::reconnect_account,
            commands::delete_account,
            commands::set_account_color,
            commands::list_contacts,
            commands::list_mailboxes,
            commands::list_messages,
            commands::view_status,
            commands::list_thread,
            commands::search_messages,
            commands::sync_account,
            commands::refresh_account,
            commands::set_message_read,
            commands::move_to_trash,
            commands::archive_message,
            commands::move_message,
            commands::move_messages,
            commands::trash_messages,
            commands::archive_messages,
            commands::set_messages_read,
            commands::trash_thread,
            commands::archive_thread,
            commands::move_thread,
            commands::get_message_body,
            commands::get_message_quote,
            commands::thread_bodies,
            commands::inspect_attachments,
            commands::save_attachment,
            commands::save_all_attachments,
            commands::attachment_preview,
            commands::queue_send,
            commands::undo_send,
            commands::schedule_send,
            commands::list_scheduled,
            commands::send_scheduled_now,
            commands::cancel_scheduled,
            commands::save_draft,
            commands::discard_draft,
            commands::open_draft,
            commands::test_connection,
            commands::open_compose,
            commands::take_compose_draft,
            commands::close_compose,
            commands::open_settings,
            commands::close_settings,
            commands::get_remote_image_policy,
            commands::set_remote_image_policy,
            commands::get_avatar_lookup_enabled,
            commands::set_avatar_lookup_enabled,
            commands::load_domain_avatars,
            commands::get_llm_summary_enabled,
            commands::set_llm_summary_enabled,
            commands::llm_status,
            commands::set_llm_model,
            commands::get_llm_summary_language,
            commands::set_llm_summary_language,
            commands::get_llm_translate_language,
            commands::set_llm_translate_language,
            commands::get_llm_translate_enabled,
            commands::set_llm_translate_enabled,
            commands::download_llm_model,
            commands::cancel_llm_download,
            commands::remove_llm_model,
            commands::summarize_message,
            commands::summarize_thread,
            commands::cancel_summary,
            commands::translate_message,
            commands::cached_summary,
            commands::get_notification_settings,
            commands::set_notification_settings,
            commands::set_account_notifications,
            commands::preview_notification_sound,
            commands::get_swipe_actions,
            commands::set_swipe_actions,
            commands::get_shortcuts,
            commands::set_shortcut,
            commands::reset_shortcuts,
            commands::timing_enabled,
            commands::log_timing,
            commands::get_appearance,
            commands::set_appearance,
            commands::get_thread_order,
            commands::set_thread_order,
            commands::get_date_time_format,
            commands::set_date_time_format,
            commands::list_signatures,
            commands::create_signature,
            commands::update_signature,
            commands::delete_signature,
            commands::set_signature_accounts,
            commands::list_aliases,
            commands::add_alias,
            commands::update_alias,
            commands::delete_alias,
            commands::set_default_alias
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // why: llama.cpp's Metal backend asserts in a static destructor
            // when a model is still loaded at process exit — free it first.
            // Exit fires once, right before Tauri calls process::exit.
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<state::AppState>();
                tauri::async_runtime::block_on(state.llm_engine.unload());
            }
        });
}

#[cfg(test)]
mod tests {
    use super::opens_in_browser;
    use tauri::Url;

    fn url(raw: &str) -> Url {
        Url::parse(raw).expect("test url parses")
    }

    #[test]
    fn hands_web_links_to_the_browser() {
        assert!(opens_in_browser(&url("https://example.com/offer")));
        assert!(opens_in_browser(&url("http://example.com")));
    }

    /// The app commands lib.rs registers, read from its own source.
    fn registered_commands() -> std::collections::BTreeSet<String> {
        let lib = include_str!("lib.rs");
        let handler = lib
            .split("generate_handler![")
            .nth(1)
            .and_then(|rest| rest.split("])").next())
            .expect("lib.rs registers its commands");
        handler
            .split(',')
            .filter_map(|entry| entry.trim().strip_prefix("commands::"))
            .map(str::to_string)
            .collect()
    }

    /// App-command permissions (`allow-<command>`) granted per capability.
    fn granted() -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/capabilities");
        std::fs::read_dir(dir)
            .expect("capabilities dir")
            .map(|entry| {
                let path = entry.expect("dir entry").path();
                let json: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&path).expect("readable"))
                        .expect("valid capability JSON");
                let commands = json["permissions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.as_str()?.strip_prefix("allow-"))
                    .map(|name| name.replace('-', "_"))
                    .collect();
                (
                    json["identifier"].as_str().unwrap_or("").to_string(),
                    commands,
                )
            })
            .collect()
    }

    #[test]
    fn the_app_manifest_declares_exactly_the_registered_commands() {
        // A command registered but not declared in build.rs gets no
        // permission, so no window could call it (deny by default).
        let build = include_str!("../build.rs");
        let declared: std::collections::BTreeSet<String> = build
            .split("APP_COMMANDS: &[&str] = &[")
            .nth(1)
            .and_then(|rest| rest.split("];").next())
            .expect("build.rs declares APP_COMMANDS")
            .split(',')
            .map(|entry| entry.trim().trim_matches('"').to_string())
            .filter(|name| !name.is_empty())
            .collect();

        assert_eq!(declared, registered_commands());
    }

    #[test]
    fn every_command_is_granted_to_some_window() {
        let granted: std::collections::BTreeSet<String> =
            granted().into_values().flatten().collect();
        let orphans: Vec<String> = registered_commands()
            .into_iter()
            .filter(|command| !granted.contains(command))
            .collect();

        assert!(orphans.is_empty(), "granted to no window: {orphans:?}");
    }

    #[test]
    fn each_window_gets_only_the_powers_it_needs() {
        // why these pairs: the compose window holds mail-derived HTML (the
        // sanctioned quote exception), so it must not be able to write files
        // where it likes; settings has no business sending mail.
        let granted = granted();
        let compose = &granted["compose"];
        for command in ["save_attachment", "save_all_attachments", "delete_account"] {
            assert!(!compose.contains(command), "compose may {command}");
        }
        let settings = &granted["settings"];
        for command in ["queue_send", "schedule_send", "save_attachment"] {
            assert!(!settings.contains(command), "settings may {command}");
        }
    }

    #[test]
    fn the_app_csp_closes_what_default_src_does_not_cover() {
        // default-src is no fallback for base-uri or form-action, and
        // object-src should never depend on it. The message iframe (srcdoc)
        // inherits this policy too.
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("valid config");
        let security = &config["app"]["security"];
        let csp = security["csp"].as_str().expect("a CSP string");
        for directive in ["object-src 'none'", "base-uri 'none'", "form-action 'none'"] {
            assert!(csp.contains(directive), "CSP lacks {directive}: {csp}");
        }
        // Frozen built-in prototypes: a polluted Object.prototype can't
        // reach Tauri's IPC glue.
        assert_eq!(security["freezePrototype"], true);
    }

    #[test]
    fn release_builds_target_a_macos_llama_cpp_compiles_for() {
        // The Tauri CLI hands minimumSystemVersion to the compiler as
        // MACOSX_DEPLOYMENT_TARGET, defaulting to 10.13 — and llama.cpp uses
        // std::filesystem, which macOS only has from 10.15. Every release
        // build failed on it (the 0.1.2 run, both architectures); dev builds
        // never set the variable, so nothing showed locally.
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("valid config");
        let minimum = config["bundle"]["macOS"]["minimumSystemVersion"]
            .as_str()
            .expect("an explicit minimum macOS version");
        let major: u32 = minimum
            .split('.')
            .next()
            .and_then(|m| m.parse().ok())
            .expect("a numeric version");
        assert!(major >= 11, "{minimum} is below what llama.cpp builds for");
    }

    #[test]
    fn keeps_every_other_scheme_inside_the_app() {
        // A message names the URL, so the scheme is untrusted input: handing
        // an arbitrary one to the OS would let a sender pick which app
        // launches. Only web links leave, everything else dies here.
        assert!(!opens_in_browser(&url("file:///etc/passwd")));
        assert!(!opens_in_browser(&url("javascript:alert(1)")));
        assert!(!opens_in_browser(&url("mailto:x@example.com")));
        assert!(!opens_in_browser(&url("data:text/html,<b>x")));
    }

    #[test]
    fn bundle_quarantines_the_files_the_app_writes() {
        // Saved attachments are sender-controlled files. Gatekeeper only
        // checks a file carrying the quarantine flag, and a non-sandboxed
        // app sets none unless its Info.plist opts in — without this key a
        // mailed unsigned app opens with no warning.
        let plist = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Info.plist"))
            .expect("src-tauri/Info.plist exists");
        let compact: String = plist.split_whitespace().collect();
        assert!(compact.contains("<key>LSFileQuarantineEnabled</key><true/>"));
    }
}
