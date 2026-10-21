import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  Menu,
  type MenuItemOptions,
  type PredefinedMenuItemOptions,
  type SubmenuOptions,
} from "@tauri-apps/api/menu";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ask, open, save } from "@tauri-apps/plugin-dialog";
import type {
  Account,
  AuthKind,
  Alias,
  AttachmentInfo,
  Contact,
  LlmDownloadProgress,
  LlmStatus,
  SummaryToken,
  TranslatedBody,
  TranslationPartial,
  TranslationProgress,
  DateTimeFormat,
  Mailbox,
  MessageAttachment,
  MessageBody,
  MessageQuote,
  MessageHeader,
  MessagesChanged,
  NewAccount,
  NotificationSettings,
  OutgoingMessage,
  RemoteImagePolicy,
  ScheduledMessage,
  SearchResults,
  SendEvent,
  ShortcutAction,
  ShortcutBinding,
  Signature,
  SwipeActions,
  ThreadOrder,
  Appearance,
  ViewStatus,
} from "./types";
import type { MenuEntry, MenuPick } from "./rowMenu";

export function listAccounts(): Promise<Account[]> {
  return invoke<Account[]>("list_accounts");
}

export function addAccount(
  account: NewAccount,
  password: string,
): Promise<Account> {
  return invoke<Account>("add_account", { account, password });
}

/** The OAuth providers this build can sign in with (none without client ids). */
export function oauthProviders(): Promise<AuthKind[]> {
  return invoke<AuthKind[]>("oauth_providers");
}

/** Add an account by signing in with `provider` in the browser. Resolves
 * once the user finished there and both servers accepted the sign-in. */
export function addOAuthAccount(
  provider: AuthKind,
  name: string,
): Promise<Account> {
  return invoke<Account>("add_oauth_account", { provider, name });
}

/** End a browser sign-in still waiting for the user. */
export function cancelOAuthSignIn(): Promise<void> {
  return invoke<void>("cancel_oauth_sign_in");
}

/** Sign an OAuth account in again after its sign-in expired. */
export function reconnectAccount(id: number): Promise<void> {
  return invoke<void>("reconnect_account", { id });
}

export function deleteAccount(id: number): Promise<void> {
  return invoke<void>("delete_account", { id });
}

/** Set (or clear, with `null`) an account's accent color. */
export function setAccountColor(
  id: number,
  color: string | null,
): Promise<void> {
  return invoke<void>("set_account_color", { id, color });
}

/** Verify & Save: prove IMAP + SMTP credentials before saving the account. */
export function testConnection(
  account: NewAccount,
  password: string,
): Promise<void> {
  return invoke<void>("test_connection", { account, password });
}

/** Native confirmation sheet; resolves to true when the user confirms. */
export function confirmAccountDeletion(account: Account): Promise<boolean> {
  return ask(
    `Delete the account “${account.name}” (${account.email})?\n\n` +
      "Its password will be removed from the keychain. " +
      "This cannot be undone.",
    { title: "Delete Account", kind: "warning" },
  );
}

/** Compose autocomplete: locally harvested contacts matching the query. */
export function listContacts(query: string): Promise<Contact[]> {
  return invoke<Contact[]>("list_contacts", { query });
}

/** Folders of one account in sidebar order (roles first, then customs). */
export function listMailboxes(accountId: number): Promise<Mailbox[]> {
  return invoke<Mailbox[]>("list_mailboxes", { accountId });
}

/** `accountId: null` = the given mailbox across all accounts. `limit` caps
 * the rows (the list reveals more on scroll); `null` = all. */
export function listMessages(
  accountId: number | null,
  mailbox = "INBOX",
  limit: number | null = null,
): Promise<MessageHeader[]> {
  return invoke<MessageHeader[]>("list_messages", {
    accountId,
    mailbox,
    limit,
  });
}

/** Counts for the current list view: header totals + backfill progress. */
export function viewStatus(
  accountId: number | null,
  mailbox = "INBOX",
): Promise<ViewStatus> {
  return invoke<ViewStatus>("view_status", { accountId, mailbox });
}

/** The full conversation of one message (all folders except trash, junk
 * and drafts), oldest first — what the conversation view renders. */
export function listThread(messageId: number): Promise<MessageHeader[]> {
  return invoke<MessageHeader[]>("list_thread", { messageId });
}

/** Bodies for the whole conversation, keyed by message id. Missing bodies
 * are fetched over a single server connection instead of one per message. */
export function threadBodies(
  messageId: number,
): Promise<Record<number, MessageBody>> {
  return invoke<Record<number, MessageBody>>("thread_bodies", { messageId });
}

/** Quote material for a reply to this message — served from the same body
 * cache as the viewer; a miss fetches the body first. */
export function getMessageQuote(messageId: number): Promise<MessageQuote> {
  return invoke<MessageQuote>("get_message_quote", { messageId });
}

/**
 * Search the local cache with a gmail-style query
 * (`from:x is:unread faktúra`). `accountId: null` searches all accounts.
 * A word that matches nothing is searched as the cached word it most
 * likely meant; `corrections` lists those swaps.
 */
export function searchMessages(
  accountId: number | null,
  query: string,
): Promise<SearchResults> {
  return invoke<SearchResults>("search_messages", { accountId, query });
}

/**
 * Body from cache, lazily fetched from the server on first open.
 * `loadRemote` is the per-message "Load Images" click; the backend honors
 * it only under the "ask" policy.
 */
export function getMessageBody(
  messageId: number,
  loadRemote = false,
): Promise<MessageBody> {
  return invoke<MessageBody>("get_message_body", { messageId, loadRemote });
}

/** Fire-and-forget sync of one account: folder list + all folders' headers. */
export function syncAccount(accountId: number): Promise<void> {
  return invoke<void>("sync_account", { accountId });
}

/**
 * The user's explicit "check for new mail": same pass as syncAccount, but
 * every folder reconciles end to end, so changes made elsewhere that are too
 * old for the routine windowed sweep show up on demand.
 */
export function refreshAccount(accountId: number): Promise<void> {
  return invoke<void>("refresh_account", { accountId });
}

/**
 * Mark a message read/unread. Updates the local cache and fires
 * messages-changed at once; the `\Seen` flag is pushed to the server in the
 * background by the backend.
 */
export function setMessageRead(
  messageId: number,
  read: boolean,
): Promise<void> {
  return invoke<void>("set_message_read", { messageId, read });
}

/**
 * Move a message to the account's Trash folder on the server, then drop it
 * from the local cache. Rejects (and keeps the row) if there is no trash
 * folder or the server move fails.
 */
export function moveToTrash(messageId: number): Promise<void> {
  return invoke<void>("move_to_trash", { messageId });
}

/** Trash the whole conversation as shown in the message's folder. */
export function trashThread(messageId: number): Promise<void> {
  return invoke<void>("trash_thread", { messageId });
}

/** Archive the whole conversation as shown in the message's folder. */
export function archiveThread(messageId: number): Promise<void> {
  return invoke<void>("archive_thread", { messageId });
}

/** Move the whole conversation (as shown in the message's folder). */
export function moveThread(messageId: number, mailbox: string): Promise<void> {
  return invoke<void>("move_thread", { messageId, mailbox });
}

/**
 * Archive a message: move it to the account's Archive folder on the server,
 * then drop it from the local cache. Rejects (and keeps the row) if there is
 * no archive folder or the server move fails.
 */
export function archiveMessage(messageId: number): Promise<void> {
  return invoke<void>("archive_message", { messageId });
}

/**
 * Move a message to a folder of its account on the server, then drop it from
 * the local cache. Rejects (and keeps the row) if the folder is unknown or
 * the server move fails.
 */
export function moveMessage(
  messageId: number,
  mailbox: string,
): Promise<void> {
  return invoke<void>("move_message", { messageId, mailbox });
}

/**
 * The bulk counterparts, for a multi-row selection. Each opens one server
 * session per account folder the selection touches, not one per message.
 *
 * `threads` says what a selected row stands for: a whole conversation in the
 * threaded views, one message in the flat trash/junk/drafts views.
 */
export function trashMessages(
  messageIds: number[],
  threads: boolean,
): Promise<void> {
  return invoke<void>("trash_messages", { messageIds, threads });
}

export function archiveMessages(
  messageIds: number[],
  threads: boolean,
): Promise<void> {
  return invoke<void>("archive_messages", { messageIds, threads });
}

export function moveMessages(
  messageIds: number[],
  mailbox: string,
  threads: boolean,
): Promise<void> {
  return invoke<void>("move_messages", { messageIds, mailbox, threads });
}

export function setMessagesRead(
  messageIds: number[],
  read: boolean,
  threads: boolean,
): Promise<void> {
  return invoke<void>("set_messages_read", { messageIds, read, threads });
}

// The menu showContextMenu popped up last. A native menu lives in the
// backend's resource table until it is closed, and popup() resolves as soon
// as the menu shows — there is no "dismissed" to wait for. Freeing the
// previous menu when the next one opens keeps at most one alive.
let lastMenu: Menu | null = null;

/**
 * Pop a native context menu up at the mouse. Choosing an item runs `onPick`
 * with its pick; dismissing the menu runs nothing. Resolves once it shows.
 */
export async function showContextMenu(
  entries: MenuEntry[],
  onPick: (pick: MenuPick) => void,
): Promise<void> {
  // why numbered ids: the backend keeps one callback per item id and never
  // drops it. Generated ids would add a handful with every right-click;
  // the same numbers each time just overwrite the previous menu's.
  let next = 0;
  const toNative = (
    entry: MenuEntry,
  ): MenuItemOptions | SubmenuOptions | PredefinedMenuItemOptions => {
    switch (entry.kind) {
      case "separator":
        return { item: "Separator" };
      case "submenu":
        return {
          text: entry.label,
          enabled: entry.items.length > 0,
          items: entry.items.map(toNative),
        };
      case "item": {
        const { pick } = entry;
        const item = { id: `row-menu-${next++}`, text: entry.label };
        return pick === null
          ? { ...item, enabled: false }
          : { ...item, action: () => onPick(pick) };
      }
    }
  };
  const menu = await Menu.new({ id: "row-menu", items: entries.map(toNative) });
  // why not awaited: freeing the old menu must not delay the new one.
  lastMenu
    ?.close()
    .catch((err: unknown) => console.error("failed to free a menu:", err));
  lastMenu = menu;
  await menu.popup();
}

/**
 * Save one received attachment: a native save dialog picks the destination,
 * then the backend re-fetches the message from the server and writes the
 * extracted part there. Resolves true once saved, false when the dialog
 * was cancelled.
 */
export async function saveAttachment(
  attachment: MessageAttachment,
): Promise<boolean> {
  const path = await save({ defaultPath: attachment.filename });
  if (path === null) return false;
  await invoke<void>("save_attachment", { attachmentId: attachment.id, path });
  return true;
}

/**
 * Save every attachment of a message into a folder picked in a native
 * dialog; one server fetch for all of them. Resolves true once saved, false
 * when the dialog was cancelled.
 */
export async function saveAllAttachments(messageId: number): Promise<boolean> {
  const dir = await open({ directory: true, title: "Save Attachments" });
  if (dir === null) return false;
  await invoke<void>("save_all_attachments", { messageId, dir });
  return true;
}

/** Open an attachment saved this run in its default app. */
export function openAttachment(attachmentId: number): Promise<void> {
  return invoke<void>("open_attachment", { attachmentId });
}

/**
 * Quick Look an attachment, as Space does in Finder: its saved file, or a
 * copy the backend fetches first. Again on the same one closes the panel.
 */
export function quickLookAttachment(attachmentId: number): Promise<void> {
  return invoke<void>("quick_look_attachment", { attachmentId });
}

/** Show an attachment saved this run selected in Finder. */
export function revealAttachment(attachmentId: number): Promise<void> {
  return invoke<void>("reveal_attachment", { attachmentId });
}

/**
 * Stat dropped file paths into chip metadata (name + size). Duplicates and
 * directories drop out; rejects if a file is missing or unreadable.
 */
export function inspectAttachments(
  paths: string[],
): Promise<AttachmentInfo[]> {
  return invoke<AttachmentInfo[]>("inspect_attachments", { paths });
}

/**
 * Best-effort thumbnail for one attachment as a `data:` URI; null when the
 * file is not a previewable image (png/jpeg/webp).
 */
export function attachmentPreview(path: string): Promise<string | null> {
  return invoke<string | null>("attachment_preview", { path });
}

/**
 * Native file drag & drop over this window. Tauri intercepts the OS drag
 * before the DOM sees it (HTML5 drop events never carry paths in a webview),
 * so dropped file paths arrive through this listener instead.
 */
export function onFileDrop(callbacks: {
  onHover: (hovering: boolean) => void;
  onDrop: (paths: string[]) => void;
}): Promise<UnlistenFn> {
  return getCurrentWebview().onDragDropEvent((event) => {
    if (event.payload.type === "drop") {
      callbacks.onHover(false);
      callbacks.onDrop(event.payload.paths);
    } else if (event.payload.type === "leave") {
      callbacks.onHover(false);
    } else {
      // "enter" and "over" both mean a drag is above the window.
      callbacks.onHover(true);
    }
  });
}

/**
 * Queue a message for sending after its undo window. Resolves as soon as
 * the message is validated and queued; progress arrives via onSendQueued /
 * onSendFinished / onSendUndone.
 */
export function queueSend(message: OutgoingMessage): Promise<void> {
  return invoke<void>("queue_send", { message });
}

/** Cancel a queued send — the draft reopens in a new compose window. */
export function undoSend(id: number): Promise<void> {
  return invoke<void>("undo_send", { id });
}

/**
 * Save a draft into its account's Drafts folder on the server (visible to
 * webmail and other clients). Returns the Message-ID of the saved version;
 * pass it back as `previousDraftId` on the next save so the server copy is
 * replaced instead of duplicated.
 */
export function saveDraft(
  message: OutgoingMessage,
  previousDraftId: string | null,
): Promise<string> {
  return invoke<string>("save_draft", { message, previousDraftId });
}

/**
 * "Don't Save" on compose close: delete the autosaved draft version from
 * the account's server Drafts folder.
 */
export function discardDraft(
  accountId: number,
  draftMessageId: string,
): Promise<void> {
  return invoke<void>("discard_draft", { accountId, draftMessageId });
}

/**
 * Park a message until `scheduledAt` (unix seconds) — "Send Later". The
 * message is validated now; delivery happens in the backend scheduler.
 */
export function scheduleSend(
  message: OutgoingMessage,
  scheduledAt: number,
): Promise<void> {
  return invoke<void>("schedule_send", { message, scheduledAt });
}

/** Every parked send-later message, soonest first (pending and missed). */
export function listScheduled(): Promise<ScheduledMessage[]> {
  return invoke<ScheduledMessage[]>("list_scheduled");
}

/** Deliver one parked message immediately (the catch-up dialog's confirm). */
export function sendScheduledNow(id: number): Promise<void> {
  return invoke<void>("send_scheduled_now", { id });
}

/** Unschedule one parked message — it reopens as a compose window. */
export function cancelScheduled(id: number): Promise<void> {
  return invoke<void>("cancel_scheduled", { id });
}

/** The set of parked messages changed (scheduled, sent, or cancelled). */
export function onScheduledChanged(callback: () => void): Promise<UnlistenFn> {
  return listen<void>("scheduled-changed", () => callback());
}

/** Some scheduled messages were missed and wait for the user's decision. */
export function onScheduledMissed(callback: () => void): Promise<UnlistenFn> {
  return listen<void>("scheduled-missed", () => callback());
}

/** A message entered its undo window. */
export function onSendQueued(
  callback: (event: SendEvent) => void,
): Promise<UnlistenFn> {
  return listen<SendEvent>("send-queued", (e) => callback(e.payload));
}

/** A queued message finished: delivered when `error` is null, failed otherwise. */
export function onSendFinished(
  callback: (event: SendEvent) => void,
): Promise<UnlistenFn> {
  return listen<SendEvent>("send-finished", (e) => callback(e.payload));
}

/** A queued message was undone and handed back to a compose window. */
export function onSendUndone(
  callback: (event: SendEvent) => void,
): Promise<UnlistenFn> {
  return listen<SendEvent>("send-undone", (e) => callback(e.payload));
}

/** Open a native compose window seeded with the draft. */
export function openCompose(draft: OutgoingMessage): Promise<void> {
  return invoke<void>("open_compose", { draft });
}

/**
 * Reopen a message from a Drafts folder for editing: the backend fetches
 * and parses the raw draft, then opens a compose window that keeps
 * replacing this server version on every save.
 */
export function openDraft(messageId: number): Promise<void> {
  return invoke<void>("open_draft", { messageId });
}

/** One-shot pickup of this compose window's draft; null after a reload. */
export function takeComposeDraft(): Promise<OutgoingMessage | null> {
  return invoke<OutgoingMessage | null>("take_compose_draft");
}

/** Close the compose window this call comes from. */
export function closeCompose(): Promise<void> {
  return invoke<void>("close_compose");
}

/** Fires whenever the message cache changes (any account, any window). */
export function onMessagesChanged(
  callback: (change: MessagesChanged) => void,
): Promise<UnlistenFn> {
  return listen<MessagesChanged>("messages-changed", (event) =>
    callback(event.payload),
  );
}

/** Current remote-image policy ("ask" until the user picks otherwise). */
export function getRemoteImagePolicy(): Promise<RemoteImagePolicy> {
  return invoke<RemoteImagePolicy>("get_remote_image_policy");
}

export function setRemoteImagePolicy(
  policy: RemoteImagePolicy,
): Promise<void> {
  return invoke<void>("set_remote_image_policy", { policy });
}

/** Whether sender-domain avatar lookups are on. Off until switched on. */
export function getAvatarLookupEnabled(): Promise<boolean> {
  return invoke<boolean>("get_avatar_lookup_enabled");
}

export function setAvatarLookupEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("set_avatar_lookup_enabled", { enabled });
}

/** Whether the experimental on-device summaries are on. Off until switched on. */
export function getLlmSummaryEnabled(): Promise<boolean> {
  return invoke<boolean>("get_llm_summary_enabled");
}

export function setLlmSummaryEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("set_llm_summary_enabled", { enabled });
}

/** State of the on-device summaries: the switch, the picked model, and
 *  every catalog model with its download state. */
export function llmStatus(): Promise<LlmStatus> {
  return invoke<LlmStatus>("llm_status");
}

export function setLlmModel(id: string): Promise<void> {
  return invoke<void>("set_llm_model", { id });
}

/** Language summaries are written in: "auto" (the message's own) or an
 *  English language name such as "Slovak". */
export function getLlmSummaryLanguage(): Promise<string> {
  return invoke<string>("get_llm_summary_language");
}

export function setLlmSummaryLanguage(language: string): Promise<void> {
  return invoke<void>("set_llm_summary_language", { language });
}

/** Whether on-device translation is on — its own switch, apart from
 *  summaries. Off until switched on. */
export function getLlmTranslateEnabled(): Promise<boolean> {
  return invoke<boolean>("get_llm_translate_enabled");
}

export function setLlmTranslateEnabled(enabled: boolean): Promise<void> {
  return invoke<void>("set_llm_translate_enabled", { enabled });
}

/** Language messages are translated into — an English language name such
 *  as "Slovak"; never "auto". */
export function getLlmTranslateLanguage(): Promise<string> {
  return invoke<string>("get_llm_translate_language");
}

export function setLlmTranslateLanguage(language: string): Promise<void> {
  return invoke<void>("set_llm_translate_language", { language });
}

/** A plain-text summary of one message, written by the local model.
 *  Rejects with guidance when the feature is off or no model is ready.
 *  Cached per inputs; `fresh` asks for a new one. */
export function summarizeMessage(
  messageId: number,
  fresh = false,
  requestId = newRequestId(),
): Promise<string> {
  return invoke<string>("summarize_message", { messageId, fresh, requestId });
}

/** A plain-text summary of the conversation the message belongs to. */
export function summarizeThread(
  messageId: number,
  fresh = false,
  requestId = newRequestId(),
): Promise<string> {
  return invoke<string>("summarize_thread", { messageId, fresh, requestId });
}

/** A summary already written for the message — or, with `thread`, for its
 *  conversation — without generating anything. Null when there is none or
 *  the feature is not ready. */
export function cachedSummary(
  messageId: number,
  thread = false,
): Promise<string | null> {
  return invoke<string | null>("cached_summary", { messageId, thread });
}

/** Stop a summary being written; a no-op once it has finished. */
export function cancelSummary(requestId: string): Promise<void> {
  return invoke<void>("cancel_summary", { requestId });
}

/** Pieces of summaries being written, tagged with their request id. */
export function onSummaryToken(
  callback: (token: SummaryToken) => void,
): Promise<UnlistenFn> {
  return listen<SummaryToken>("summary-token", (event) =>
    callback(event.payload),
  );
}

/** The message's text translated by the local model into the language
 *  picked in Settings — rendered like getMessageBody, its texts swapped.
 *  Progress arrives on onTranslationProgress, the body so far on
 *  onTranslationPartial; cancelSummary with the same request id stops it. */
export function translateMessage(
  messageId: number,
  loadRemote: boolean,
  requestId = newRequestId(),
): Promise<TranslatedBody> {
  return invoke<TranslatedBody>("translate_message", {
    messageId,
    loadRemote,
    requestId,
  });
}

/** Progress of translations, tagged with their request id. */
export function onTranslationProgress(
  callback: (progress: TranslationProgress) => void,
): Promise<UnlistenFn> {
  return listen<TranslationProgress>("translation-progress", (event) =>
    callback(event.payload),
  );
}

/** The body of translations being written, after every batch. */
export function onTranslationPartial(
  callback: (partial: TranslationPartial) => void,
): Promise<UnlistenFn> {
  return listen<TranslationPartial>("translation-partial", (event) =>
    callback(event.payload),
  );
}

/** A request id for summaries — unique within this window's lifetime. */
export function newRequestId(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

/** Start fetching a catalog model in the background. Progress arrives on
 *  onLlmDownloadProgress; the end (done, cancelled, failed) on
 *  onLlmModelsChanged. */
export function downloadLlmModel(id: string): Promise<void> {
  return invoke<void>("download_llm_model", { id });
}

export function cancelLlmDownload(id: string): Promise<void> {
  return invoke<void>("cancel_llm_download", { id });
}

/** Delete a downloaded model file, or dismiss a failed download. */
export function removeLlmModel(id: string): Promise<void> {
  return invoke<void>("remove_llm_model", { id });
}

export function onLlmDownloadProgress(
  callback: (progress: LlmDownloadProgress) => void,
): Promise<UnlistenFn> {
  return listen<LlmDownloadProgress>("llm-download-progress", (event) =>
    callback(event.payload),
  );
}

/** Fires when a model download ends or a model file is removed. */
export function onLlmModelsChanged(callback: () => void): Promise<UnlistenFn> {
  return listen("llm-models-changed", callback);
}

/** Icons for sender domains, keyed by domain and ready to use as an img src.
 *  A domain with no icon is simply absent — the list falls back to its
 *  monogram. Returns nothing at all while the lookup switch is off. */
export function loadDomainAvatars(
  domains: string[],
): Promise<Record<string, string>> {
  return invoke<Record<string, string>>("load_domain_avatars", { domains });
}

/** Global notification defaults + background check cadence. */
export function getNotificationSettings(): Promise<NotificationSettings> {
  return invoke<NotificationSettings>("get_notification_settings");
}

export function setNotificationSettings(
  settings: NotificationSettings,
): Promise<void> {
  return invoke<void>("set_notification_settings", { settings });
}

/** Preview a notification sound pick; "default"/"none" play nothing. */
export function previewNotificationSound(sound: string): Promise<void> {
  return invoke<void>("preview_notification_sound", { sound });
}

/** Per-account notification overrides; null = inherit the global default. */
export function setAccountNotifications(
  id: number,
  enabled: boolean | null,
  sound: string | null,
): Promise<void> {
  return invoke<void>("set_account_notifications", { id, enabled, sound });
}

/** Configured swipe actions (left archives, right toggles read until
 * the user picks otherwise). */
export function getSwipeActions(): Promise<SwipeActions> {
  return invoke<SwipeActions>("get_swipe_actions");
}

export function setSwipeActions(actions: SwipeActions): Promise<void> {
  return invoke<void>("set_swipe_actions", { actions });
}

/** Every action's keyboard binding, in settings order. */
export function getShortcuts(): Promise<ShortcutBinding[]> {
  return invoke<ShortcutBinding[]>("get_shortcuts");
}

/** Bind an action to a combo (null unbinds). Whichever action held the combo
 * before is left unbound; the returned list shows the result. Rejects with a
 * readable message for a reserved or malformed combo. */
export function setShortcut(
  action: ShortcutAction,
  combo: string | null,
): Promise<ShortcutBinding[]> {
  return invoke<ShortcutBinding[]>("set_shortcut", { action, combo });
}

export function resetShortcuts(): Promise<ShortcutBinding[]> {
  return invoke<ShortcutBinding[]>("reset_shortcuts");
}

/** The app's colour scheme (follows the system until changed). Setting it
 * re-themes every window natively, so the CSS needs no class toggle. */
export function getAppearance(): Promise<Appearance> {
  return invoke<Appearance>("get_appearance");
}

export function setAppearance(appearance: Appearance): Promise<void> {
  return invoke<void>("set_appearance", { appearance });
}

/** Conversation-view order (newest at the bottom until changed). */
export function getThreadOrder(): Promise<ThreadOrder> {
  return invoke<ThreadOrder>("get_thread_order");
}

export function setThreadOrder(order: ThreadOrder): Promise<void> {
  return invoke<void>("set_thread_order", { order });
}

/** How the UI writes dates and times (the OS locale until changed). */
export function getDateTimeFormat(): Promise<DateTimeFormat> {
  return invoke<DateTimeFormat>("get_date_time_format");
}

export function setDateTimeFormat(format: DateTimeFormat): Promise<void> {
  return invoke<void>("set_date_time_format", { format });
}

/** Fires whenever any window changes an app-wide setting. */
export function onSettingsChanged(callback: () => void): Promise<UnlistenFn> {
  return listen("settings-changed", callback);
}

export function openSettings(): Promise<void> {
  return invoke<void>("open_settings");
}

export function closeSettings(): Promise<void> {
  return invoke<void>("close_settings");
}

/** Fires whenever any window mutates the account list. */
export function onAccountsChanged(callback: () => void): Promise<UnlistenFn> {
  return listen("accounts-changed", callback);
}

export function listSignatures(): Promise<Signature[]> {
  return invoke<Signature[]>("list_signatures");
}

export function createSignature(name: string): Promise<Signature> {
  return invoke<Signature>("create_signature", { name });
}

export function updateSignature(
  id: number,
  name: string,
  body: string,
): Promise<void> {
  return invoke<void>("update_signature", { id, name, body });
}

export function deleteSignature(id: number): Promise<void> {
  return invoke<void>("delete_signature", { id });
}

/** Make one signature the default for exactly the given accounts. */
export function setSignatureAccounts(
  id: number,
  accountIds: number[],
): Promise<void> {
  return invoke<void>("set_signature_accounts", { id, accountIds });
}

export function listAliases(): Promise<Alias[]> {
  return invoke<Alias[]>("list_aliases");
}

export function addAlias(
  accountId: number,
  name: string,
  email: string,
): Promise<Alias> {
  return invoke<Alias>("add_alias", { accountId, name, email });
}

export function updateAlias(
  id: number,
  name: string,
  email: string,
): Promise<void> {
  return invoke<void>("update_alias", { id, name, email });
}

export function deleteAlias(id: number): Promise<void> {
  return invoke<void>("delete_alias", { id });
}

/** Pick the identity new mail from this account starts with;
 * null = the account's own address. */
export function setDefaultAlias(
  accountId: number,
  aliasId: number | null,
): Promise<void> {
  return invoke<void>("set_default_alias", { accountId, aliasId });
}

/** Fires whenever any window mutates the signature list. */
export function onSignaturesChanged(callback: () => void): Promise<UnlistenFn> {
  return listen("signatures-changed", callback);
}

/** Does the backend run with FLIT_TIMING set? Mirrored into the webview so
 * one env var switches timing on across both processes. */
export function fetchTimingEnabled(): Promise<boolean> {
  return invoke<boolean>("timing_enabled");
}

/** Hand one timing line to the backend so it lands on the process's stderr —
 * a webview's console goes to the webview, and a release build has no
 * devtools to open. */
export function logTiming(line: string): Promise<void> {
  return invoke<void>("log_timing", { line });
}
