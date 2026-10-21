<script lang="ts">
  import { onMount } from "svelte";
  import {
    archiveMessage,
    archiveMessages,
    archiveThread,
    cancelScheduled,
    discardDraft,
    getDateTimeFormat,
    getMessageBody,
    getMessageQuote,
    getShortcuts,
    getSwipeActions,
    getThreadOrder,
    llmStatus,
    listAccounts,
    listAliases,
    listMailboxes,
    listMessages,
    listThread,
    listScheduled,
    loadDomainAvatars,
    moveMessage,
    moveMessages,
    moveThread,
    moveToTrash,
    trashMessages,
    trashThread,
    onAccountsChanged,
    onLlmModelsChanged,
    onMessagesChanged,
    onSettingsChanged,
    onScheduledChanged,
    onScheduledMissed,
    onSendFinished,
    onSendQueued,
    onSendUndone,
    openCompose,
    openDraft as openServerDraft,
    openSettings,
    searchMessages,
    sendScheduledNow,
    showContextMenu,
    setMessageRead,
    setMessagesRead,
    refreshAccount,
    syncAccount,
    undoSend,
    viewStatus,
  } from "./lib/api";
  import { senderDomain } from "./lib/avatar";
  import { applyDateTimeFormat } from "./lib/datetime.svelte";
  import { debounce } from "./lib/debounce";
  import {
    forwardDraft,
    replyAllDraft,
    replyDraft,
    type DraftKind,
  } from "./lib/draft";
  import type {
    Account,
    Alias,
    Mailbox,
    MessageHeader,
    MessageQuote,
    ScheduledMessage,
    SearchCorrection,
    ShortcutAction,
    ShortcutBinding,
    SwipeActions,
    ThreadOrder,
    ViewStatus,
  } from "./lib/types";
  import { actionFor, comboFromEvent, firesIn } from "./lib/shortcuts";
  import { rowMenu, type MenuPick } from "./lib/rowMenu";
  import { DEFAULT_SWIPE_ACTIONS } from "./lib/swipe";
  import { markPaint, timed } from "./lib/timing";
  import { neighborId, nextMessageId, type NavDelta } from "./lib/messageNav";
  import {
    EMPTY_SELECTION,
    extendRange,
    pruneSelection,
    type SelectMode,
    type Selection,
    selectOne,
    toggleId,
  } from "./lib/selection";
  import {
    clampPaneWidth,
    loadPaneWidths,
    PANE_LIMITS,
    savePaneWidths,
    type PaneWidths,
  } from "./lib/paneSizes";
  import Sidebar from "./lib/Sidebar.svelte";
  import Toolbar from "./lib/Toolbar.svelte";
  import MessageList from "./lib/MessageList.svelte";
  import MessageView from "./lib/MessageView.svelte";
  import Outbox, { type OutboxEntry } from "./lib/Outbox.svelte";
  import MissedSends from "./lib/MissedSends.svelte";

  let accounts = $state<Account[]>([]);
  let aliases = $state<Alias[]>([]);
  let mailboxesByAccount = $state<Record<number, Mailbox[]>>({});
  let messages = $state<MessageHeader[]>([]);
  let selectedAccountId = $state<number | null>(null);
  let selectedMailbox = $state("INBOX");
  let selectedMessageId = $state<number | null>(null);

  // Rows revealed per "page" of the list — the cache may mirror tens of
  // thousands, but the DOM only grows as the user actually scrolls.
  const LIST_PAGE = 500;
  let visibleLimit = $state(LIST_PAGE);
  // Whole-view totals + backfill progress; null while a search is active.
  let listStatus = $state<ViewStatus | null>(null);
  // Whether `messages` currently holds search hits rather than a folder's
  // rows — a search hit is a row for one message, not for its thread, so
  // the reading pane must open that message rather than the thread's
  // newest (which is what a folder row stands for).
  let showingSearchHits = $state(false);
  // Typed words the current search replaced; empty outside a search.
  let searchCorrections = $state<SearchCorrection[]>([]);

  function handleLoadMore() {
    // Fires repeatedly while the user sits near the bottom — bump once and
    // ignore the rest until the longer list has actually arrived.
    if (messages.length < visibleLimit) return;
    visibleLimit += LIST_PAGE;
    void refreshMessages().catch((err: unknown) =>
      console.error("failed to load more messages:", err),
    );
  }

  // The rows a bulk action would hit. `selection` is the raw record; rows it
  // names can leave the list at any time (an eviction, a folder switch, a
  // search), so everything downstream reads the pruned view instead — no
  // effect has to chase `messages` to keep the two in step.
  let selection = $state<Selection>(EMPTY_SELECTION);
  let messageIds = $derived(messages.map((m) => m.id));
  let visibleSelection = $derived(pruneSelection(selection, messageIds));

  // why gated on the selection size: the reading pane must not follow a
  // cmd-click into a second message — showing it would mark it read, which
  // is the opposite of what gathering rows for a bulk action means. One row
  // selected still reads normally, cmd-clicked or not.
  let selectedMessage = $derived(
    visibleSelection.ids.length > 1
      ? null
      : (messages.find((m) => m.id === selectedMessageId) ?? null),
  );

  // accountId → color, so the list can dot each row with its account's color
  // (most useful in the unified inbox, where accounts are interleaved).
  let accountColors = $derived(
    Object.fromEntries(accounts.map((a) => [a.id, a.color])),
  );

  // accountId → address, so the conversation view can label own messages.
  let accountEmails = $derived(
    Object.fromEntries(accounts.map((a) => [a.id, a.email])),
  );

  // A message living in a Drafts folder is unfinished work — opening it
  // resumes editing in a compose window instead of a reading view. Used by
  // the list (drafts folder rows) and the conversation view (draft cards).
  function editDraft(id: number) {
    void openServerDraft(id).catch((err: unknown) =>
      console.error("failed to open draft:", err),
    );
  }

  // The draft card's Delete action. The backend removes the server version,
  // re-syncs the Drafts folder and emits messages-changed — the card
  // disappears on that refresh, no optimistic bookkeeping needed.
  function deleteDraft(message: MessageHeader) {
    void discardDraft(message.accountId, message.messageId).catch(
      (err: unknown) => console.error("failed to delete draft:", err),
    );
  }

  // A click or arrow key on a row. Only "replace" opens the message — cmd
  // and shift gather rows for a bulk action, which must not resume a draft
  // in a compose window or clear an unread dot the user never read.
  function selectMessage(id: number, mode: SelectMode = "replace") {
    if (mode === "toggle") {
      selection = toggleId(visibleSelection, id);
      selectedMessageId = id;
      return;
    }
    if (mode === "range") {
      selection = extendRange(visibleSelection, messageIds, id);
      selectedMessageId = id;
      return;
    }
    const message = messages.find((m) => m.id === id);
    if (message && isDraft(message)) {
      editDraft(id);
      return;
    }
    selection = selectOne(id);
    selectedMessageId = id;
    // why: opening an unread message marks it read (like Apple Mail) — the
    // backend clears the local dot and pushes \Seen to the server.
    if (message && !message.read) handleSetRead(id, true);
  }

  function isDraft(message: MessageHeader): boolean {
    return (mailboxesByAccount[message.accountId] ?? []).some(
      (m) => m.name === message.mailbox && m.role === "drafts",
    );
  }

  // why: the backend updates the cache and emits messages-changed, so the
  // list refreshes on its own — here we only fire the command and log a
  // failure (a stale flag heals on the next sync).
  function handleSetRead(id: number, read: boolean) {
    const ids = bulkIdsFor(id);
    if (ids) {
      void setMessagesRead(ids, read, selectionIsThreaded).catch(
        (err: unknown) => console.error("failed to set read state:", err),
      );
      return;
    }
    void setMessageRead(id, read).catch((err: unknown) =>
      console.error("failed to set read state:", err),
    );
  }

  // Rows optimistically dropped whose server move is still in flight. The
  // backend deletes a cached row only after the server confirms the move,
  // so a background messages-changed landing mid-move would resurrect the
  // row via refreshMessages — the filter there keeps it out until the
  // action settles. Plain Set, not $state: nothing renders from it, it is
  // only read inside refreshMessages, which runs after every mutation.
  const pendingEvictions = new Set<number>();

  // Optimistically drop a message from the list and run `action` (trash /
  // archive) on the server, so it feels instant. When it was the open
  // message, selection steps to the neighbour it leaves behind; a server
  // failure re-queries to bring the message back rather than leave the list
  // lying.
  function evictMessage(id: number, action: (id: number) => Promise<void>) {
    const next = neighborId(
      messages.map((m) => m.id),
      id,
    );
    pendingEvictions.add(id);
    messages = messages.filter((m) => m.id !== id);
    // why only for the open row: a swipe can take away a row the user is
    // not reading, and stepping would open — and mark read — its neighbour.
    if (selectedMessageId === id) {
      if (next === null) selectedMessageId = null;
      else selectMessage(next);
    }
    void action(id).then(
      // Success needs no refresh of its own: the backend deleted the row
      // before resolving and emits messages-changed, which refreshes.
      () => pendingEvictions.delete(id),
      (err: unknown) => {
        console.error("message action failed:", err);
        // why: un-pend before the refresh — the row is still cached after
        // a failure and must come back into the list.
        pendingEvictions.delete(id);
        void refreshMessages();
      },
    );
  }

  // Thread rows (a grouped conversation) act on every member in the
  // current folder; single rows — including all rows of the flat
  // trash/junk/drafts views, where threadCount is 0 — act on one message.
  function isThreadRow(id: number): boolean {
    return (messages.find((m) => m.id === id)?.threadCount ?? 0) > 1;
  }

  // The selected rows themselves — the toolbar needs their read state, and
  // the bulk commands their ids.
  // why a Set: a shift-range can hold hundreds of ids, and an Array.includes
  // per row would rescan it for every message in the list.
  let selectedRows = $derived.by(() => {
    const ids = new Set(visibleSelection.ids);
    return messages.filter((m) => ids.has(m.id));
  });

  /** An action on row `id` covers the whole selection when the selection
   * holds more than one row and `id` is one of them. A single row keeps the
   * existing per-message commands, and so does a row outside the selection:
   * a swipe aims at the row under the fingers, not at rows picked earlier. */
  function bulkIdsFor(id: number): number[] | null {
    const ids = visibleSelection.ids;
    return ids.length > 1 && ids.includes(id) ? ids : null;
  }

  // What a selected row stands for, which is what the backend must expand:
  // a whole conversation in the threaded views, one message in the flat
  // trash/junk/drafts views (where the backend reports threadCount 0).
  let selectionIsThreaded = $derived(
    selectedRows.length > 0 && selectedRows.every((m) => m.threadCount > 0),
  );

  // Optimistic bulk eviction: every row leaves the list at once, selection
  // clears, and one command covers the lot. A failure re-queries, exactly
  // like the single-row path.
  function evictMessages(ids: number[], action: () => Promise<void>) {
    for (const id of ids) pendingEvictions.add(id);
    const evicted = new Set(ids);
    messages = messages.filter((m) => !evicted.has(m.id));
    selection = EMPTY_SELECTION;
    selectedMessageId = null;
    void action().then(
      () => {
        for (const id of ids) pendingEvictions.delete(id);
      },
      (err: unknown) => {
        console.error("bulk message action failed:", err);
        for (const id of ids) pendingEvictions.delete(id);
        void refreshMessages();
      },
    );
  }

  function handleTrash(id: number) {
    const ids = bulkIdsFor(id);
    if (ids) {
      // why read now: evictMessages clears the selection, and the closure
      // would otherwise see an empty one by the time it runs.
      const threaded = selectionIsThreaded;
      evictMessages(ids, () => trashMessages(ids, threaded));
      return;
    }
    evictMessage(id, isThreadRow(id) ? trashThread : moveToTrash);
  }

  // sender domain → icon, for the message list's avatars. Empty while the
  // lookup setting is off: the backend returns nothing rather than fetching.
  let avatars = $state<Record<string, string>>({});
  // why a plain Set, not $state: it must NOT be a dependency of the effect
  // below. A domain with no icon never lands in `avatars`, so tracking asked
  // domains reactively would re-run the effect and ask again forever.
  let askedDomains = new Set<string>();
  // Bumped by resetAvatars so a settings change re-runs the effect below —
  // without it, nothing would re-ask until the message list itself changed.
  let avatarEpoch = $state(0);

  $effect(() => {
    void avatarEpoch;
    const wanted = [
      ...new Set(
        messages
          .map((message) => senderDomain(message.from))
          .filter((domain): domain is string => domain !== null),
      ),
    ].filter((domain) => !askedDomains.has(domain));
    if (wanted.length === 0) return;

    for (const domain of wanted) askedDomains.add(domain);
    // why fire-and-forget: avatars are decoration — a failed lookup leaves
    // the monogram in place and must never surface as an error.
    void loadDomainAvatars(wanted)
      .then((found) => {
        if (Object.keys(found).length > 0) avatars = { ...avatars, ...found };
      })
      .catch((err: unknown) => console.error("failed to load avatars:", err));
  });

  /** Drop what we know so a settings change is picked up: switching the
   *  lookup on must re-ask, switching it off must clear the icons. */
  function resetAvatars() {
    askedDomains = new Set();
    avatars = {};
    avatarEpoch += 1;
  }

  // What the list's swipe gesture does per direction — user-configurable in
  // the settings window, so it loads on start and re-loads on settings-changed.
  let swipeActions = $state<SwipeActions>(DEFAULT_SWIPE_ACTIONS);

  async function refreshSwipeActions() {
    swipeActions = await getSwipeActions();
  }

  // How the conversation view orders its cards — user-configurable in the
  // settings window, reloaded on settings-changed like the swipe actions.
  let threadOrder = $state<ThreadOrder>("newestLast");

  async function refreshThreadOrder() {
    threadOrder = await getThreadOrder();
  }

  // Whether message cards offer the on-device summary, and the language
  // they offer translation into (null: not offered) — each feature's
  // Experimental switch on and the picked model downloaded.
  let llmReady = $state(false);
  let llmTranslateInto = $state<string | null>(null);
  async function refreshLlmReady() {
    const status = await llmStatus();
    llmReady = status.ready;
    llmTranslateInto = status.translateReady ? status.translateLanguage : null;
  }

  // How dates and clock times are written. Unlike the two above this is no
  // prop: format.ts reads the shared rune, so the list rows, the
  // conversation cards and the reply attribution all follow it at once.
  async function refreshDateTimeFormat() {
    applyDateTimeFormat(await getDateTimeFormat());
  }

  // why fetch first: neither the list rows nor the toolbar hold the body —
  // replies fetch quote material, forwards the plain text. Both are local
  // cache hits, and a failure still opens the compose window, just without
  // the quoted content.
  function openDraftWithBody(kind: DraftKind, message: MessageHeader) {
    const open =
      kind === "forward"
        ? getMessageBody(message.id).then((body) =>
            openDraft(kind, message, null, body.text),
          )
        : getMessageQuote(message.id).then((quote) =>
            openDraft(kind, message, quote, null),
          );
    void open.catch((err: unknown) => {
      console.error("failed to load content for draft:", err);
      openDraft(kind, message, null, null);
    });
  }

  function handleSwipeReply(id: number) {
    const message = messages.find((m) => m.id === id);
    if (message) void openDraftOnLatest("reply", message);
  }

  // why resolve first: a list row only represents the newest message in the
  // open folder — after my own reply (filed in Sent) the conversation's true
  // latest message lives elsewhere, and that's the one a follow-up quotes.
  // listThread is a local cache hit; on failure the row itself still works.
  async function openDraftOnLatest(kind: DraftKind, message: MessageHeader) {
    let target = message;
    try {
      const thread = await listThread(message.id);
      target = thread[thread.length - 1] ?? message;
    } catch (err) {
      console.error("failed to resolve the thread's latest message:", err);
    }
    openDraftWithBody(kind, target);
  }

  // The account's archive folder wire name; Gmail has no \Archive, so its
  // All Mail (\All) counts — mirrors the backend's archive fallback order.
  function archiveNameFor(accountId: number): string | null {
    const folders = mailboxesByAccount[accountId] ?? [];
    const archive =
      folders.find((m) => m.role === "archive") ??
      folders.find((m) => m.role === "all");
    return archive?.name ?? null;
  }

  function isArchived(message: MessageHeader): boolean {
    return message.mailbox === archiveNameFor(message.accountId);
  }

  // Archive flips to unarchive on an already-archived message — re-archiving
  // would be a silent server no-op while the row vanished from the list.
  function handleArchive(id: number) {
    const ids = bulkIdsFor(id);
    if (ids) {
      // why every: a selection sitting entirely in its archive folder flips
      // to unarchive, like one row does; a mixed one archives what is left.
      if (selectedRows.every(isArchived)) {
        handleMove(id, "INBOX");
      } else {
        const threaded = selectionIsThreaded;
        evictMessages(ids, () => archiveMessages(ids, threaded));
      }
      return;
    }
    const message = messages.find((m) => m.id === id);
    const thread = isThreadRow(id);
    if (message && isArchived(message)) {
      const inbox =
        (mailboxesByAccount[message.accountId] ?? []).find(
          (m) => m.role === "inbox",
        )?.name ?? "INBOX";
      handleMove(id, inbox);
    } else {
      evictMessage(id, thread ? archiveThread : archiveMessage);
    }
  }

  function handleMove(id: number, mailbox: string) {
    const ids = bulkIdsFor(id);
    if (ids) {
      const threaded = selectionIsThreaded;
      evictMessages(ids, () => moveMessages(ids, mailbox, threaded));
      return;
    }
    const move = isThreadRow(id) ? moveThread : moveMessage;
    evictMessage(id, (messageId) => move(messageId, mailbox));
  }

  // Right-click on a row, by Finder's rule: inside a multi-selection the menu
  // covers the selection, anywhere else just the clicked row. Neither moves
  // the selection nor opens the row, so nothing gets marked read.
  function handleContextMenu(id: number) {
    const rows = bulkIdsFor(id)
      ? selectedRows
      : messages.filter((m) => m.id === id);
    if (rows.length === 0) return;
    const entries = rowMenu(rows, mailboxesByAccount, isArchived, shortcuts);
    void showContextMenu(entries, (pick) =>
      runMenuPick(id, rows[0], pick),
    ).catch((err: unknown) => console.error("failed to show the menu:", err));
  }

  // Each pick goes through the handler its toolbar button, swipe or shortcut
  // already uses. `id` is the right-clicked row — those handlers widen it to
  // the selection when it belongs to one; `row` is what a reply answers
  // (the menu offers replies for a single row only).
  function runMenuPick(id: number, row: MessageHeader, pick: MenuPick) {
    switch (pick.action) {
      case "reply":
      case "reply-all":
      case "forward":
        void openDraftOnLatest(pick.action, row);
        return;
      case "edit-draft":
        editDraft(id);
        return;
      case "read":
        handleSetRead(id, pick.read);
        return;
      case "archive":
        handleArchive(id);
        return;
      case "trash":
        handleTrash(id);
        return;
      case "move":
        handleMove(id, pick.mailbox);
        return;
    }
  }

  // Keyboard shortcuts — user-configurable in the settings window, reloaded
  // on settings-changed like the swipe actions.
  let shortcuts = $state<ShortcutBinding[]>([]);

  async function refreshShortcuts() {
    shortcuts = await getShortcuts();
  }

  let toolbar = $state<ReturnType<typeof Toolbar>>();

  /** Run a shortcut's action through the same path as its toolbar button,
   * with the same guards — a message action with nothing selected is a
   * no-op, like the disabled button. Returns false for actions this window
   * does not own (Send belongs to compose windows). */
  function runShortcut(action: ShortcutAction): boolean {
    // The rows the toolbar's action buttons operate on (see Toolbar.svelte).
    const rows =
      selectedRows.length > 0
        ? selectedRows
        : selectedMessage
          ? [selectedMessage]
          : [];
    switch (action) {
      case "new-message":
        openNewMessage();
        return true;
      case "reply":
      case "reply-all":
      case "forward":
        openDraftFromSelection(action);
        return true;
      case "archive":
        if (rows.length > 0) handleArchive(rows[0].id);
        return true;
      case "trash":
        if (rows.length > 0) handleTrash(rows[0].id);
        return true;
      case "toggle-read":
        if (rows.length > 0) {
          handleSetRead(rows[0].id, !rows.every((row) => row.read));
        }
        return true;
      case "check-mail":
        if (syncsInFlight === 0) handleRefresh();
        return true;
      case "toggle-sidebar":
        toggleSidebar();
        return true;
      case "focus-search":
        toolbar?.focusSearch();
        return true;
      case "send":
        return false;
    }
  }

  // Configured shortcuts first; then Arrow Up / Down walk the list. The
  // arrows are ignored while typing in a field (search, etc.) or with a
  // modifier held, so system/app shortcuts aren't hijacked.
  function handleKeydown(event: KeyboardEvent) {
    const combo = comboFromEvent(event);
    const action =
      combo !== null && firesIn(event.target, combo)
        ? actionFor(shortcuts, combo)
        : null;
    if (action !== null && runShortcut(action)) {
      event.preventDefault();
      return;
    }
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const target = event.target as HTMLElement | null;
    if (
      target?.tagName === "INPUT" ||
      target?.tagName === "TEXTAREA" ||
      target?.isContentEditable
    ) {
      return;
    }
    event.preventDefault();
    const delta: NavDelta = event.key === "ArrowDown" ? 1 : -1;
    const next = nextMessageId(messageIds, selectedMessageId, delta);
    if (next === null) return;
    // Shift grows the range instead of moving the selection — and, like a
    // shift-click, it must not open the row it lands on.
    if (event.shiftKey) {
      selection = extendRange(visibleSelection, messageIds, next);
      selectedMessageId = next;
      return;
    }
    selectMessage(next);
  }

  let listTitle = $derived(
    selectedAccountId === null
      ? "All Inboxes"
      : selectedMailbox !== "INBOX"
        ? (mailboxesByAccount[selectedAccountId]?.find(
            (m) => m.name === selectedMailbox,
          )?.displayName ?? selectedMailbox)
        : (accounts.find((a) => a.id === selectedAccountId)?.name ?? "Inbox"),
  );

  let paneWidths = $state<PaneWidths>(loadPaneWidths(localStorage));

  const SIDEBAR_COLLAPSED_KEY = "flit.sidebar.collapsed";
  let sidebarCollapsed = $state(
    localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "true",
  );

  function toggleSidebar() {
    sidebarCollapsed = !sidebarCollapsed;
    localStorage.setItem(SIDEBAR_COLLAPSED_KEY, String(sidebarCollapsed));
  }

  // Sends riding out their undo window, mirrored from backend events.
  let outbox = $state<OutboxEntry[]>([]);
  /** How long a resolved badge lingers: long enough to read, short for ✓. */
  const SENT_BADGE_MS = 2500;
  const FAILED_BADGE_MS = 6000;

  function badgeQueued(id: number, subject: string, undoMs: number) {
    outbox = [...outbox, { id, subject, status: "sending", error: null, undoMs }];
  }

  function badgeFinished(id: number, error: string | null) {
    outbox = outbox.map((b) =>
      b.id === id ? { ...b, status: error ? "failed" : "sent", error } : b,
    );
    setTimeout(
      () => (outbox = outbox.filter((b) => b.id !== id)),
      error ? FAILED_BADGE_MS : SENT_BADGE_MS,
    );
  }

  function badgeUndone(id: number) {
    outbox = outbox.filter((b) => b.id !== id);
  }

  // why: the badge disappears on the backend's send-undone event, not here —
  // if undo lost the race the send is in flight and the badge must resolve.
  function handleUndo(id: number) {
    void undoSend(id).catch((err: unknown) =>
      console.error("undo send failed:", err),
    );
  }

  // Send-later messages parked in the backend: pending ones feed the
  // sidebar's Scheduled section; missed ones (due while the app was off or
  // asleep past the grace window) are never auto-sent — the catch-up dialog
  // asks per message.
  let scheduledSends = $state<ScheduledMessage[]>([]);
  let missedSends = $state<ScheduledMessage[]>([]);
  // "Decide later" hides these ids until the next launch; the rows stay
  // missed in the DB, so a restart asks again.
  const dismissedMissed = new Set<number>();

  async function refreshScheduled() {
    const scheduled = await listScheduled();
    scheduledSends = scheduled.filter((m) => m.status === "pending");
    missedSends = scheduled.filter(
      (m) => m.status === "missed" && !dismissedMissed.has(m.id),
    );
  }

  // why remove-first: the row is gone from the DB either way (take() is the
  // claim); keeping it in the dialog would just offer a dead button.
  function missedSendNow(id: number) {
    missedSends = missedSends.filter((m) => m.id !== id);
    void sendScheduledNow(id).catch((err: unknown) =>
      console.error("send scheduled now failed:", err),
    );
  }

  function missedOpenDraft(id: number) {
    missedSends = missedSends.filter((m) => m.id !== id);
    void cancelScheduled(id).catch((err: unknown) =>
      console.error("cancel scheduled failed:", err),
    );
  }

  function missedDismiss() {
    for (const entry of missedSends) dismissedMissed.add(entry.id);
    missedSends = [];
  }

  // why remove-first: like the dialog's actions, the row is already claimed
  // in the backend; the scheduled-changed event re-syncs the list after.
  function handleCancelScheduled(id: number) {
    scheduledSends = scheduledSends.filter((m) => m.id !== id);
    void cancelScheduled(id).catch((err: unknown) =>
      console.error("cancel scheduled failed:", err),
    );
  }

  function openNewMessage() {
    const fallback = accounts[0];
    if (!fallback) return;
    // why: new mail goes from the account being viewed; on the unified inbox
    // the first account acts as the default sender.
    const sender = accounts.find((a) => a.id === selectedAccountId) ?? fallback;
    void openCompose({
      accountId: sender.id,
      // The identity marked Default in the account's alias settings.
      aliasId: sender.defaultAliasId ?? undefined,
      to: "",
      subject: "",
      body: "",
    }).catch((err: unknown) => console.error("failed to open compose:", err));
  }

  function openDraft(
    kind: DraftKind,
    message: MessageHeader,
    quote: MessageQuote | null,
    bodyText: string | null,
  ) {
    // why: reply-all must not echo mail back to the mailbox it landed in —
    // the receiving account's address is the one to drop from recipients.
    const ownEmail =
      accounts.find((a) => a.id === message.accountId)?.email ?? "";
    const draft =
      kind === "reply"
        ? replyDraft(message, quote, aliases, ownEmail)
        : kind === "reply-all"
          ? replyAllDraft(message, quote, ownEmail, aliases)
          : forwardDraft(message, bodyText, aliases);
    void openCompose(draft).catch((err: unknown) =>
      console.error(`failed to open ${kind}:`, err),
    );
  }

  // The toolbar's reply/forward buttons act on the open conversation.
  function openDraftFromSelection(kind: DraftKind) {
    if (selectedMessage) void openDraftOnLatest(kind, selectedMessage);
  }

  // Move to targets for the toolbar menu: the selection's account folders,
  // minus the folder the message already sits in (a self-move is a no-op).
  let moveTargets = $derived(
    selectedMessage === null
      ? []
      : (mailboxesByAccount[selectedMessage.accountId] ?? []).filter(
          (m) => m.name !== selectedMessage?.mailbox,
        ),
  );

  function startPaneResize(pane: keyof PaneWidths, event: PointerEvent) {
    event.preventDefault();
    const divider = event.currentTarget as HTMLElement;
    const startX = event.clientX;
    const startWidth = paneWidths[pane];
    // why pointer capture: move/up events keep hitting the divider even when
    // the cursor leaves it mid-drag, so no window-level listeners are needed.
    // Optional call — jsdom doesn't implement it.
    divider.setPointerCapture?.(event.pointerId);
    const onMove = (e: PointerEvent) => {
      paneWidths[pane] = clampPaneWidth(pane, startWidth + e.clientX - startX);
    };
    const onUp = () => {
      divider.removeEventListener("pointermove", onMove);
      divider.removeEventListener("pointerup", onUp);
      savePaneWidths(localStorage, paneWidths);
    };
    divider.addEventListener("pointermove", onMove);
    divider.addEventListener("pointerup", onUp);
  }

  function nudgePane(pane: keyof PaneWidths, event: KeyboardEvent) {
    const delta =
      event.key === "ArrowLeft" ? -16 : event.key === "ArrowRight" ? 16 : 0;
    if (delta === 0) return;
    event.preventDefault();
    paneWidths[pane] = clampPaneWidth(pane, paneWidths[pane] + delta);
    savePaneWidths(localStorage, paneWidths);
  }

  // why: plain variable, not $state — the input's value lives in Toolbar;
  // this only steers which query refreshMessages runs.
  let searchQuery = "";

  // Manual "check for new mail": re-sync what the user is looking at — all
  // accounts on the unified inbox, otherwise just the selected one. Asking
  // on purpose reconciles folders in full, unlike the routine passes.
  function handleRefresh() {
    startSync(
      selectedAccountId === null ? accounts.map((a) => a.id) : [selectedAccountId],
      refreshAccount,
    );
  }

  // Pull-to-refresh: the same re-sync, but awaited (unlike startSync) so the
  // list's drawer can stay open until every account's sync lands.
  let pullRefreshing = $state(false);

  async function handlePullRefresh() {
    if (pullRefreshing) return;
    pullRefreshing = true;
    const ids =
      selectedAccountId === null
        ? accounts.map((a) => a.id)
        : [selectedAccountId];
    // why allSettled: one account's dead server must neither hide the other
    // syncs' results nor leave the drawer spinning forever.
    const results = await Promise.allSettled(ids.map((id) => refreshAccount(id)));
    results.forEach((result, i) => {
      if (result.status === "rejected")
        console.error(
          `account sync failed for account ${ids[i]}:`,
          result.reason,
        );
    });
    pullRefreshing = false;
  }

  function handleOpenSettings() {
    void openSettings().catch((err: unknown) =>
      console.error("failed to open settings:", err),
    );
  }

  async function selectMailbox(accountId: number | null, mailbox = "INBOX") {
    selectedAccountId = accountId;
    selectedMailbox = mailbox;
    selectedMessageId = null;
    // A fresh view starts at the first page again.
    visibleLimit = LIST_PAGE;
    await refreshMessages();
    startSync(accountId === null ? accounts.map((a) => a.id) : [accountId]);
  }

  // Re-query the current view without touching the selection — the derived
  // selectedMessage keeps pointing at the same id if it still exists. With a
  // search active, "the current view" is the result list (across all folders,
  // like Gmail), so a sync landing mid-search refreshes the hits instead of
  // yanking the full list back.
  // why filter at assignment (after the await): a refresh already in
  // flight when an eviction starts still applies the filter when it lands,
  // so even a stale query cannot bring the row back.
  function withoutPending(list: MessageHeader[]): MessageHeader[] {
    return list.filter((m) => !pendingEvictions.has(m.id));
  }

  async function refreshMessages() {
    const query = searchQuery.trim();
    if (query) {
      const results = await timed("searchMessages", () =>
        searchMessages(selectedAccountId, query),
      );
      messages = withoutPending(results.messages);
      searchCorrections = results.corrections;
      // Search results are their own universe — whole-view totals and the
      // load-more trigger don't apply to them.
      listStatus = null;
      showingSearchHits = true;
      markPaint("list.paint");
      return;
    }
    const [list, status] = await timed("refreshMessages", () =>
      Promise.all([
        listMessages(selectedAccountId, selectedMailbox, visibleLimit),
        viewStatus(selectedAccountId, selectedMailbox),
      ]),
    );
    messages = withoutPending(list);
    listStatus = status;
    showingSearchHits = false;
    searchCorrections = [];
    // why here: the assignment above is what re-renders the rows, so the
    // frame that follows it is the list's render cost.
    markPaint("list.paint");
  }

  // The sidebar's folder lists, mirrored per account. Refreshed alongside
  // messages because folders first appear when an account's sync lands.
  async function refreshMailboxes() {
    const entries = await Promise.all(
      accounts.map(
        async (a) => [a.id, await listMailboxes(a.id)] as const,
      ),
    );
    mailboxesByAccount = Object.fromEntries(entries);
  }

  // Patch a read-flag change straight into the rows already on screen.
  //
  // why not just re-query: the list query is a full scan (~250 ms measured on
  // a real mailbox) and clicking an unread message fired one every time. A
  // dot clearing can neither add nor remove a row, so there is nothing to ask
  // for — only rows to correct.
  function applyReadChange(ids: number[], read: boolean) {
    const wanted = new Set(ids);
    let seen = 0;
    let flipped = 0;
    messages = messages.map((message) => {
      if (!wanted.has(message.id)) return message;
      seen += 1;
      if (message.read === read) return message;
      flipped += 1;
      // why threadUnread only for a lone message: it answers "is anything in
      // this conversation still unread", which this window cannot decide for
      // a conversation it holds one row of. Left alone it errs towards
      // showing the dot, and the next real reload settles it.
      return message.threadCount > 1
        ? { ...message, read }
        : { ...message, read, threadUnread: !read };
    });
    // why the fallback: ids we cannot see still count towards the view's
    // unread tally — a thread member in another folder, a row past the loaded
    // page — so the number can only be adjusted here when every id was on
    // screen. Even then this skips the list query, which is the expensive half.
    if (seen < wanted.size) {
      void refreshStatus().catch((err: unknown) =>
        console.error("failed to refresh view status:", err),
      );
      return;
    }
    if (listStatus && flipped > 0) {
      listStatus = {
        ...listStatus,
        unread: Math.max(0, listStatus.unread + (read ? -flipped : flipped)),
      };
    }
  }

  /** The view's counts alone — no list query. */
  async function refreshStatus() {
    if (searchQuery.trim()) return;
    listStatus = await viewStatus(selectedAccountId, selectedMailbox);
  }

  // why 200ms: long enough to collapse a typing burst into one query, short
  // enough that results still feel live (search is a local SQLite hit).
  const refreshDebounced = debounce(() => void refreshMessages(), 200);

  // why debounce: a backfilling account emits messages-changed after every
  // cached batch, and several accounts sync at once — refreshing per event
  // would rerun the thread-grouping list query and the sidebar fetches
  // nonstop for the whole sync. One trailing refresh per burst is enough;
  // 250ms is imperceptible next to a click.
  const refreshOnMessagesChanged = debounce(() => {
    void refreshMessages();
    void refreshMailboxes();
  }, 250);

  function handleSearch(query: string) {
    searchQuery = query;
    refreshDebounced();
  }

  // How many fire-and-forget account syncs are still running — the toolbar's
  // refresh icon spins while any are, so a click visibly does something.
  let syncsInFlight = $state(0);

  // why: fire-and-forget and parallel — every invoke runs as its own async
  // task in the backend, and the UI reads from the cache as each account's
  // messages-changed event lands. One slow server never delays the others.
  function startSync(
    accountIds: number[],
    run: (accountId: number) => Promise<void> = syncAccount,
  ) {
    for (const id of accountIds) {
      syncsInFlight += 1;
      run(id)
        .catch((err: unknown) => {
          console.error(`account sync failed for account ${id}:`, err);
        })
        .finally(() => {
          syncsInFlight -= 1;
        });
    }
  }

  // why: plain variable, not $state — nothing renders from it; it only stops
  // the very first refresh from double-syncing what onMount already syncs.
  let accountsLoaded = false;

  async function refreshAccounts() {
    const known = new Set(accounts.map((a) => a.id));
    accounts = await listAccounts();
    // why here: alias edits broadcast accounts-changed too, so reply
    // matching always works against the current alias list.
    aliases = await listAliases();
    // why: an account just added in settings syncs right away, so its
    // messages and connection status appear without waiting for a selection.
    if (accountsLoaded) {
      const added = accounts.filter((a) => !known.has(a.id));
      if (added.length > 0) startSync(added.map((a) => a.id));
    }
    accountsLoaded = true;
    await refreshMailboxes();
    // why: if the selected account was deleted in the settings window, fall
    // back to the unified inbox instead of filtering by a dead account.
    if (
      selectedAccountId !== null &&
      !accounts.some((a) => a.id === selectedAccountId)
    ) {
      await selectMailbox(null);
    }
  }

  onMount(() => {
    void (async () => {
      await refreshAccounts();
      await selectMailbox(null);
    })();
    const refreshSwipeLogged = () => {
      void refreshSwipeActions().catch((err: unknown) =>
        console.error("failed to load swipe actions:", err),
      );
      void refreshShortcuts().catch((err: unknown) =>
        console.error("failed to load keyboard shortcuts:", err),
      );
      void refreshThreadOrder().catch((err: unknown) =>
        console.error("failed to load thread order:", err),
      );
      void refreshDateTimeFormat().catch((err: unknown) =>
        console.error("failed to load the date format:", err),
      );
      void refreshLlmReady().catch((err: unknown) =>
        console.error("failed to load the summaries status:", err),
      );
      resetAvatars();
    };
    refreshSwipeLogged();
    // why: the settings window mutates the config in its own JS context —
    // this window finds out through the backend's settings-changed event.
    const unlistenSettings = onSettingsChanged(refreshSwipeLogged);
    // A model download finishing (or a model removed) changes readiness
    // without touching any setting.
    const unlistenLlm = onLlmModelsChanged(
      () =>
        void refreshLlmReady().catch((err: unknown) =>
          console.error("failed to load the summaries status:", err),
        ),
    );
    // why separate: scheduled/missed sends must surface even if account sync
    // fails — they are local rows, not server state.
    const refreshScheduledLogged = () =>
      void refreshScheduled().catch((err: unknown) =>
        console.error("failed to load scheduled sends:", err),
      );
    refreshScheduledLogged();
    const unlistenMissed = onScheduledMissed(refreshScheduledLogged);
    const unlistenScheduled = onScheduledChanged(refreshScheduledLogged);
    // why: account CRUD lives in the settings window (its own JS context) —
    // this window finds out through the backend's accounts-changed event.
    const unlistenAccounts = onAccountsChanged(() => void refreshAccounts());
    const unlistenMessages = onMessagesChanged((change) => {
      // A read flag is the one change the window can settle by itself.
      if (change.read) {
        applyReadChange(change.read.ids, change.read.read);
        return;
      }
      refreshOnMessagesChanged();
    });
    // why: compose windows queue sends in the backend; this window only
    // mirrors the send-* events into badges.
    const unlistenQueued = onSendQueued((e) =>
      badgeQueued(e.id, e.subject, e.undoMs),
    );
    const unlistenFinished = onSendFinished((e) => badgeFinished(e.id, e.error));
    const unlistenUndone = onSendUndone((e) => badgeUndone(e.id));
    return () => {
      void unlistenAccounts.then((stop) => stop());
      void unlistenMessages.then((stop) => stop());
      void unlistenQueued.then((stop) => stop());
      void unlistenFinished.then((stop) => stop());
      void unlistenUndone.then((stop) => stop());
      void unlistenMissed.then((stop) => stop());
      void unlistenScheduled.then((stop) => stop());
      void unlistenSettings.then((stop) => stop());
      void unlistenLlm.then((stop) => stop());
    };
  });
</script>

<svelte:window onkeydown={handleKeydown} />

<Toolbar
  bind:this={toolbar}
  {sidebarCollapsed}
  sidebarWidth={paneWidths.sidebar}
  onToggleSidebar={toggleSidebar}
  onRefresh={handleRefresh}
  refreshing={syncsInFlight > 0}
  onCompose={openNewMessage}
  onSearch={handleSearch}
  onOpenSettings={handleOpenSettings}
  selected={selectedMessage}
  selection={selectedRows}
  archived={selectedMessage ? isArchived(selectedMessage) : false}
  {moveTargets}
  onDraft={openDraftFromSelection}
  onSetRead={handleSetRead}
  onArchive={handleArchive}
  onTrash={handleTrash}
  onMove={handleMove}
  {shortcuts}
/>

<div
  class="layout"
  style:grid-template-columns={sidebarCollapsed
    ? "minmax(0, 1fr)"
    : `${paneWidths.sidebar}px 1px minmax(0, 1fr)`}
>
  {#if !sidebarCollapsed}
    <aside>
      <Sidebar
        {accounts}
        mailboxes={mailboxesByAccount}
        selectedAccountId={selectedAccountId}
        selectedMailbox={selectedMailbox}
        onSelect={selectMailbox}
        scheduled={scheduledSends}
        onCancelScheduled={handleCancelScheduled}
      />
    </aside>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions
         — a focusable separator is the ARIA "window splitter" widget; Svelte's
         checker only knows the static (non-focusable) separator variant. -->
    <div
      class="divider"
      role="separator"
      tabindex="0"
      aria-orientation="vertical"
      aria-label="Resize sidebar"
      aria-valuenow={paneWidths.sidebar}
      aria-valuemin={PANE_LIMITS.sidebar.min}
      aria-valuemax={PANE_LIMITS.sidebar.max}
      onpointerdown={(e) => startPaneResize("sidebar", e)}
      onkeydown={(e) => nudgePane("sidebar", e)}
    ></div>
  {/if}
  <main
    style:grid-template-columns={`${paneWidths.list}px 1px minmax(0, 1fr)`}
  >
    <section class="list">
      <MessageList
        title={listTitle}
        {messages}
        {accountColors}
        {avatars}
        selectedId={selectedMessageId}
        selectedIds={visibleSelection.ids}
        onSelect={selectMessage}
        {swipeActions}
        onArchive={handleArchive}
        onSetRead={handleSetRead}
        {isArchived}
        onTrash={handleTrash}
        onReply={handleSwipeReply}
        onContextMenu={handleContextMenu}
        status={listStatus}
        corrections={searchCorrections}
        onLoadMore={handleLoadMore}
        onRefresh={handlePullRefresh}
        refreshing={pullRefreshing}
      />
      <Outbox entries={outbox} onUndo={handleUndo} />
    </section>
    <MissedSends
      entries={missedSends}
      onSendNow={missedSendNow}
      onOpenDraft={missedOpenDraft}
      onDismiss={missedDismiss}
    />
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions
         — a focusable separator is the ARIA "window splitter" widget; Svelte's
         checker only knows the static (non-focusable) separator variant. -->
    <div
      class="divider"
      role="separator"
      tabindex="0"
      aria-orientation="vertical"
      aria-label="Resize message list"
      aria-valuenow={paneWidths.list}
      aria-valuemin={PANE_LIMITS.list.min}
      aria-valuemax={PANE_LIMITS.list.max}
      onpointerdown={(e) => startPaneResize("list", e)}
      onkeydown={(e) => nudgePane("list", e)}
    ></div>
    <section class="view">
      <MessageView
        message={selectedMessage}
        selectedCount={visibleSelection.ids.length}
        focusSelected={showingSearchHits}
        {accountEmails}
        {accountColors}
        {threadOrder}
        onDraft={openDraftWithBody}
        onEditDraft={editDraft}
        onDeleteDraft={deleteDraft}
        canSummarize={llmReady}
        translateInto={llmTranslateInto}
      />
    </section>
  </main>
</div>

<style>
  .layout {
    display: grid;
    height: calc(100vh - var(--toolbar-height));
    /* why: implicit (auto) grid rows size to their content, so a tall
       conversation stack grew the row past the viewport and the whole app
       scrolled. Pin the single row to the container's height — panes clip
       and scroll themselves instead. */
    grid-template-rows: 100%;
  }

  /* why overflow-x hidden everywhere: overflow-y auto alone computes
     overflow-x to auto, so a squeezed pane (narrow window, panes dragged
     wide) grows a horizontal scrollbar — panes must only ever clip. */
  aside {
    overflow-y: auto;
    overflow-x: hidden;
    background: var(--bg-sidebar);
  }

  main {
    display: grid;
    /* Same row pinning as .layout — main is its own grid. */
    grid-template-rows: 100%;
    min-width: 0;
    min-height: 0;
    background: var(--bg-window);
  }

  .list {
    /* why relative: the Outbox badges anchor to this pane's bottom edge. */
    position: relative;
    overflow-y: auto;
    overflow-x: hidden;
  }

  /* Replaces the old border-right lines: a 1px grid column that doubles as
     a drag handle, with a wider invisible grab area via the ::after overlay. */
  .divider {
    position: relative;
    background: var(--border-chrome);
    cursor: col-resize;
    touch-action: none;
  }

  .divider::after {
    content: "";
    position: absolute;
    top: 0;
    bottom: 0;
    left: -3px;
    right: -3px;
  }

  .divider:focus-visible {
    outline: none;
    background: var(--accent);
  }

  .view {
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    overflow-x: hidden;
  }
</style>
