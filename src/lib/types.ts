// Mirrors src-tauri/src/models.rs — keep in sync (serde renames to camelCase).

export interface Account {
  id: number;
  name: string;
  email: string;
  imapHost: string;
  imapPort: number;
  smtpHost: string;
  smtpPort: number;
  username: string;
  /** Error message from the last connection check; null = healthy. */
  lastError: string | null;
  /** Unix seconds of the last check; null = never checked yet. */
  checkedAt: number | null;
  /** User-chosen accent color (e.g. "#ff9f0a"); null = no color set. */
  color: string | null;
  /** Default signature for mail composed from this account; null = none. */
  signatureId: number | null;
  /** New-mail notification override; null = inherit the global setting. */
  notifyEnabled: boolean | null;
  /** Sound override ("none" or a macOS sound name); null = inherit. */
  notifySound: string | null;
  /** Send-as identity new mail starts with; null = the account's address. */
  defaultAliasId: number | null;
  /** How the account signs in to its servers. */
  auth: AuthKind;
}

/** "password" = keychain password; "google" = Google OAuth sign-in. */
export type AuthKind = "password" | "google";

/** One send-as alias: an extra address the account's mail server accepts
 * as sender. Only stored aliases may appear as From. */
export interface Alias {
  id: number;
  accountId: number;
  /** Display name for the From header; empty = address only. */
  name: string;
  email: string;
}

/** One reusable e-mail signature; `body` is editor HTML. */
export interface Signature {
  id: number;
  name: string;
  body: string;
}

/** Payload for add_account — the password travels as a separate argument. */
export interface NewAccount {
  name: string;
  email: string;
  imapHost: string;
  imapPort: number;
  smtpHost: string;
  smtpPort: number;
  username: string;
}

/** Payload for send_message — the From address comes from the account row. */
export interface OutgoingMessage {
  accountId: number;
  /** Send-as alias picked in compose; absent/null = the account's address. */
  aliasId?: number | null;
  /** One or more recipients, comma-separated. */
  to: string;
  /** Cc recipients, comma-separated; empty/absent = none. */
  cc?: string;
  /** Bcc recipients — SMTP envelope only, never a visible header. */
  bcc?: string;
  subject: string;
  /** Plain-text body; doubles as the fallback part of an HTML message. */
  body: string;
  /** HTML rendering of the body; absent/empty = plain text only. */
  bodyHtml?: string;
  /** Attached files; absent/empty = none. Bytes are read at send time. */
  attachments?: AttachmentRef[];
  /** Message-ID of this message's autosaved server draft version, if any —
   * the backend deletes it from the Drafts folder after a successful send. */
  draftMessageId?: string;
  /** Message-ID (no angle brackets) of the mail this one answers — becomes
   * the In-Reply-To header. Absent for fresh mail. */
  inReplyTo?: string;
  /** Space-separated ancestor Message-IDs for the References header,
   * oldest first, ending with `inReplyTo`. */
  references?: string;
  /** The reply-quote riding below the editor, still un-merged into
   * body/bodyHtml. Never read by the backend — it travels so a reopened
   * draft (undo, failed send) can restore its quote block. */
  quote?: DraftQuote | null;
}

/** get_message_quote result — the original rendered for quoting in a
 * reply. Mirrors MessageQuote in models.rs. */
export interface MessageQuote {
  /** Sanitized HTML fragment (cid images inlined, remote refs unresolved,
   * text mail upconverted to blockquotes). Empty = nothing quotable. */
  html: string;
  /** Cleaned plain text — source of the outgoing "> " fallback lines. */
  text: string;
}

/** A compose draft's quote of the original: quote material plus the
 * attribution line built for it. Mirrors DraftQuote in models.rs. */
export interface DraftQuote {
  attribution: string;
  html: string;
  text: string;
}

/** One file attached to an outgoing message, referenced by path. */
export interface AttachmentRef {
  path: string;
  /** Filename shown to recipients — the path's final component. */
  name: string;
}

/** inspect_attachments result — chip metadata for one dropped file. */
export interface AttachmentInfo {
  path: string;
  name: string;
  /** File size in bytes, for display only. */
  size: number;
}

/** One "Send Later" message parked in the backend until its delivery time.
 * Mirrors ScheduledMessage in models.rs. */
export interface ScheduledMessage {
  id: number;
  accountId: number;
  /** Send-as alias the message was composed with; null = account address. */
  aliasId: number | null;
  to: string;
  cc: string;
  bcc: string;
  subject: string;
  body: string;
  bodyHtml: string | null;
  attachments: AttachmentRef[];
  /** Unix seconds (UTC) when the message should leave. */
  scheduledAt: number;
  /** Missed rows never send on their own — the user resolves them. */
  status: "pending" | "missed";
  /** Threading identity of a scheduled reply; null for fresh mail. */
  inReplyTo: string | null;
  references: string | null;
}

/** Payload of send-queued / send-finished / send-undone events. */
export interface SendEvent {
  id: number;
  subject: string;
  /** Set only on send-finished when the delivery failed. */
  error: string | null;
  /** Undo window length in ms; only meaningful on send-queued, 0 elsewhere. */
  undoMs: number;
}

/**
 * How remote (http/https) images in mail bodies are treated. Inline cid:
 * images always render — they are part of the message itself.
 */
export type RemoteImagePolicy = "block" | "ask" | "always";

/** Mirrors NotificationSettings in models.rs. */
export interface NotificationSettings {
  /** Master switch for new-mail notifications. */
  enabled: boolean;
  /** "default" = system sound, "none" = silent, else a macOS sound name. */
  sound: string;
  /**
   * Minutes between background new-mail checks; 0 = manual sync only. With
   * pushEnabled this is the cadence for everything IDLE does not cover:
   * the other folders, and flag changes made on another device.
   */
  syncIntervalMinutes: number;
  /** Hold an IMAP IDLE connection open per account for instant inbox mail. */
  pushEnabled: boolean;
}

/** Mirrors LlmStatus in models.rs. */
export interface LlmStatus {
  /** The Experimental switch for summaries. */
  enabled: boolean;
  /** The Experimental switch for translation. */
  translateEnabled: boolean;
  /** Id of the picked model, if any. */
  activeModel: string | null;
  /** Summaries on and the picked model's file is complete on disk. */
  ready: boolean;
  /** The same for translation. */
  translateReady: boolean;
  models: LlmModel[];
}

/** Mirrors LlmModel in models.rs: a catalog entry plus its local state. */
export interface LlmModel {
  id: string;
  name: string;
  description: string;
  /** Download size in bytes. */
  size: number;
  recommended: boolean;
  state: LlmModelState;
}

export type LlmModelState =
  | { kind: "missing" }
  | { kind: "downloading"; received: number; total: number }
  | { kind: "failed"; error: string }
  | { kind: "ready" };

/** Mirrors SummaryToken in models.rs: one piece of a summary being written. */
export interface SummaryToken {
  requestId: string;
  text: string;
}

/** Mirrors TranslationProgress in models.rs: pieces of a message
 *  translated so far, out of `total`. */
export interface TranslationProgress {
  requestId: string;
  done: number;
  total: number;
}

/** Mirrors TranslatedBody in models.rs: a message's HTML and/or own text
 *  with the translation in place — shown over the original body, whose
 *  quoted history, attachments and warnings stay. */
export interface TranslatedBody {
  html: string | null;
  text: string | null;
}

/** Mirrors TranslationPartial in models.rs: the body translated so far. */
export interface TranslationPartial {
  requestId: string;
  body: TranslatedBody;
}

/** Mirrors LlmDownloadProgress in models.rs. */
export interface LlmDownloadProgress {
  id: string;
  received: number;
  total: number;
}

/** What one direction of the message-list swipe gesture does;
 * "none" disables that direction. */
export type SwipeAction = "none" | "toggleRead" | "archive" | "trash" | "reply";

/** The app's colour scheme; "system" follows macOS. */
export type Appearance = "system" | "light" | "dark";

/** Order of messages in the conversation view. */
export type ThreadOrder = "newestLast" | "newestFirst";

/** The configured action for each swipe direction on message-list rows. */
export interface SwipeActions {
  left: SwipeAction;
  right: SwipeAction;
}

/** An app action that can be triggered from the keyboard. */
export type ShortcutAction =
  | "new-message"
  | "reply"
  | "reply-all"
  | "forward"
  | "archive"
  | "trash"
  | "toggle-read"
  | "check-mail"
  | "toggle-sidebar"
  | "focus-search"
  | "send";

/** One action's keyboard binding. Combos are canonical strings,
 * `Ctrl+Alt+Shift+Meta+<Key>` with modifiers in that order (e.g. "Meta+N"). */
export interface ShortcutBinding {
  action: ShortcutAction;
  /** null = unbound. */
  combo: string | null;
  defaultCombo: string;
}

/** How dates are written. Every value but "system" (the OS locale's own
 * format) is the pattern it produces, so it doubles as the menu label. */
export type DateFormat =
  | "system"
  | "dd.mm.yyyy"
  | "dd.mm.yy"
  | "dd/mm/yyyy"
  | "mm/dd/yyyy"
  | "yyyy-mm-dd"
  | "yyyy/mm/dd";

/** Whether clock times read as 14:30 or 2:30 PM. */
export type TimeFormat = "system" | "24h" | "12h";

/** How the UI writes dates and clock times; the halves are independent. */
export interface DateTimeFormat {
  date: DateFormat;
  time: TimeFormat;
}

/** What the app falls back to before the stored preference has loaded. */
export const SYSTEM_DATE_TIME_FORMAT: DateTimeFormat = {
  date: "system",
  time: "system",
};

/** `html`, when present, is a full sanitized srcdoc document from the backend. */
export interface MessageBody {
  html: string | null;
  text: string | null;
  /** Quoted history split off a plain-text body; the card folds it behind
   * a ••• toggle. Null when the body has none (or is all quote). */
  quotedText: string | null;
  /** Remote images left blocked in `html`; 0 when none or all loaded. */
  blockedImages: number;
  /** Offer "Load Images" (policy is "ask" and this render blocked some). */
  canLoadRemote: boolean;
  /** Attachments — metadata only; saving re-fetches bytes from the server. */
  attachments: MessageAttachment[];
  /** SPF/DKIM/DMARC verdicts of the message; null = unknown (no
   * Authentication-Results header). Warn only on explicit "fail". */
  auth: AuthResults | null;
  /** Set when a familiar name arrived from an address it doesn't usually
   * use — the Canary-style warning, computed from local history only. */
  senderAnomaly: SenderAnomaly | null;
}

/** Mirrors AuthResults in models.rs. Each field is the lowercase verdict
 * token ("pass", "fail", "softfail", …); null = not mentioned. */
export interface AuthResults {
  spf: string | null;
  dkim: string | null;
  dmarc: string | null;
}

/** Mirrors SenderAnomaly in models.rs. */
export interface SenderAnomaly {
  /** The familiar display name the message arrived under. */
  name: string;
  /** The address this name usually writes from. */
  usualEmail: string;
}

/** One attachment of a received message, as cached metadata. */
export interface MessageAttachment {
  id: number;
  messageId: number;
  /** MIME part index used by the backend to re-extract the bytes. */
  partIndex: number;
  filename: string;
  /** Full MIME type ("application/pdf"). */
  contentType: string;
  /** Decoded size in bytes, for display. */
  size: number;
}

/** One autocomplete suggestion for a compose recipient field. */
export interface Contact {
  email: string;
  /** Latest display name seen for the address; empty when none was. */
  name: string;
}

/** One folder of one account, as shown in the sidebar. */
export interface Mailbox {
  id: number;
  accountId: number;
  /** Full IMAP name — also the value list_messages expects as `mailbox`. */
  name: string;
  /** "inbox" | "drafts" | "sent" | "archive" | "junk" | "trash" | null. */
  role: string | null;
  /** Decoded, prefix-stripped name for the UI — never send to the backend. */
  displayName: string;
  /** Unread messages in this folder, from the local cache. */
  unreadCount: number;
}

/** What a search found, plus any typed word it searched as another because
 * nothing in the cache matched it. */
export interface SearchResults {
  messages: MessageHeader[];
  corrections: SearchCorrection[];
}

/** A mistyped search word and the cached word searched in its place
 * ("bnny" → "bunny"). */
export interface SearchCorrection {
  typed: string;
  corrected: string;
}

/** Counts for one list view — the list header totals plus the backfill
 * progress pair (cached vs. what the server holds). */
export interface ViewStatus {
  /** Rows the unlimited list query would return: conversations in threaded
   * views, messages in flat ones. */
  listRows: number;
  /** Unread messages in the view, from the local cache. */
  unread: number;
  /** Messages cached across every folder of the view's account (all
   * accounts in the unified view) — progress is account-wide so the
   * backfill is visible from any view. */
  cached: number;
  /** Messages the server holds in those same folders; null until the first
   * sync reports it. cached < serverTotal = backfill still running. */
  serverTotal: number | null;
}

export interface MessageHeader {
  id: number;
  accountId: number;
  /** Full IMAP name of the folder the message lives in. */
  mailbox: string;
  from: string;
  /** To recipients as displayed ("Name <addr>", comma-separated). */
  to: string;
  /** Cc recipients, same shape; empty when there were none. */
  cc: string;
  /** Reply-To recipients, same shape; empty when the header was absent. */
  replyTo: string;
  /** Bcc recipients, same shape; almost always empty — Sent copies from
   * other clients (or journaled deliveries) can carry the header. */
  bcc: string;
  subject: string;
  snippet: string;
  /** In threaded lists: the newest message of the conversation (my own
   * reply, filed in Sent, counts) — not necessarily this row's own date. */
  date: string;
  read: boolean;
  /** Whether the message (in threaded lists: any message of the
   * conversation) carries attachments — drives the list's paperclip. */
  hasAttachments: boolean;
  /** Message-ID without angle brackets; empty when the sender set none. */
  messageId: string;
  /** Space-joined ancestor Message-IDs — a reply appends messageId to this
   * chain to form its own References header. */
  references: string;
  /** Whether the message sits in the account's Drafts folder. Only the
   * conversation query sets it (Draft badge + click-to-edit); list views
   * derive draft-ness from the mailbox role instead. */
  isDraft: boolean;
  /** Messages in this row's conversation (all folders except trash, junk
   * and drafts — a draft shows in the conversation but is not counted).
   * Always ≥ 1 in threaded lists; 0 in flat views. */
  threadCount: number;
  /** Whether any message of the conversation is unread. */
  threadUnread: boolean;
  /** Whether an unsent draft is saved for this conversation — the list
   * row's "Draft" pill. Computed by threaded lists only. */
  threadHasDraft: boolean;
}

/** What a `messages-changed` event carries. Mirrors `MessagesChanged` in
 * models.rs. */
export interface MessagesChanged {
  accountId: number;
  /** Set only when nothing but read flags changed, so the list can be
   * patched in place. `null` means rows may have appeared or gone and the
   * view has to ask again. */
  read: { ids: number[]; read: boolean } | null;
}
