import { beforeEach, expect, it, vi } from "vitest";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/svelte";
import type {
  Alias,
  Account,
  DateTimeFormat,
  MessageHeader,
  MessagesChanged,
  ScheduledMessage,
  SendEvent,
  ShortcutBinding,
  SwipeActions,
} from "./lib/types";

const accounts: Account[] = [
  {
    id: 1,
    name: "Personal",
    email: "domco@example.com",
    imapHost: "imap.example.com",
    imapPort: 993,
    smtpHost: "smtp.example.com",
    smtpPort: 587,
    username: "domco@example.com",
    lastError: null,
    checkedAt: 1751900000,
    color: null,
    signatureId: null,
    notifyEnabled: null,
    notifySound: null,
    defaultAliasId: null,
    auth: "password",
  },
  {
    id: 2,
    name: "Work",
    email: "hello@vocalio.sk",
    imapHost: "imap.vocalio.sk",
    imapPort: 993,
    smtpHost: "smtp.vocalio.sk",
    smtpPort: 587,
    username: "hello@vocalio.sk",
    lastError: null,
    checkedAt: 1751900000,
    color: null,
    signatureId: null,
    notifyEnabled: null,
    notifySound: null,
    defaultAliasId: null,
    auth: "password",
  },
];

const allMessages: MessageHeader[] = [
  {
    id: 1,
    accountId: 1,
    mailbox: "INBOX",
    from: "Alice <alice@example.com>",
    to: "domco@example.com, Bob <bob@example.com>",
    cc: "carol@example.com",
    replyTo: "",
    bcc: "",
    messageId: "",
    references: "",
    threadCount: 1,
    threadUnread: false,
    isDraft: false,
    threadHasDraft: false,
    subject: "Weekend plans",
    snippet: "Are we still on for Saturday?",
    date: "2026-07-07T09:15:00Z",
    read: false,
    hasAttachments: false,
  },
  {
    id: 2,
    accountId: 2,
    mailbox: "INBOX",
    from: "Peter",
    to: "hello@vocalio.sk",
    cc: "",
    replyTo: "",
    bcc: "",
    messageId: "",
    references: "",
    threadCount: 1,
    threadUnread: false,
    isDraft: false,
    threadHasDraft: false,
    subject: "Re: Invoice",
    snippet: "Payment went out this morning.",
    date: "2026-07-08T07:45:00Z",
    read: true,
    hasAttachments: false,
  },
];

// Lives in Personal's Archive folder — only listed when that folder is open.
const archivedMessages: MessageHeader[] = [
  {
    id: 9,
    accountId: 1,
    mailbox: "Archive",
    from: "Old Friend <old@example.com>",
    to: "hello@vocalio.sk",
    cc: "",
    replyTo: "",
    bcc: "",
    messageId: "",
    references: "",
    threadCount: 1,
    threadUnread: false,
    isDraft: false,
    threadHasDraft: false,
    subject: "Archived note",
    snippet: "Filed away long ago.",
    date: "2026-06-01T00:00:00Z",
    read: true,
    hasAttachments: false,
  },
];

/** "Something other than a read flag changed" — the view has to re-query. */
const RELOAD: MessagesChanged = { accountId: 1, read: null };

// why: mutable + captured by the mock factory, so tests can simulate the
// backend changing state and firing change events.
let currentAccounts: Account[] = [];
let currentAliases: Alias[] = [];
let currentMessages: MessageHeader[] = [];
let currentScheduled: ScheduledMessage[] = [];
let accountsChanged: (() => void) | undefined;
let messagesChanged: ((change: MessagesChanged) => void) | undefined;
let sendQueued: ((e: SendEvent) => void) | undefined;
let sendFinished: ((e: SendEvent) => void) | undefined;
let sendUndone: ((e: SendEvent) => void) | undefined;
let scheduledMissed: (() => void) | undefined;
let scheduledChanged: (() => void) | undefined;
let settingsChanged: (() => void) | undefined;
let currentSwipeActions: SwipeActions;
let currentShortcuts: ShortcutBinding[];

/** The shipped bindings, as the backend reports them before any rebinding. */
function defaultShortcuts(): ShortcutBinding[] {
  const defaults: [ShortcutBinding["action"], string][] = [
    ["new-message", "Meta+N"],
    ["reply", "Meta+R"],
    ["reply-all", "Shift+Meta+R"],
    ["forward", "Shift+Meta+F"],
    ["archive", "Ctrl+Meta+A"],
    ["trash", "Backspace"],
    ["toggle-read", "Shift+Meta+U"],
    ["check-mail", "Shift+Meta+N"],
    ["toggle-sidebar", "Ctrl+Meta+S"],
    ["focus-search", "Meta+F"],
    ["send", "Meta+Enter"],
  ];
  return defaults.map(([action, combo]) => ({
    action,
    combo,
    defaultCombo: combo,
  }));
}
let currentDateTimeFormat: DateTimeFormat;

// why a named default: tests override listMailboxes with
// mockImplementation, which clearAllMocks does NOT undo — beforeEach
// reinstalls this so an override can never leak into later tests.
async function defaultListMailboxes(accountId: number) {
  return accountId === 1
    ? [
        {
          id: 1,
          accountId: 1,
          name: "INBOX",
          role: "inbox",
          displayName: "INBOX",
          unreadCount: 0,
        },
        {
          id: 2,
          accountId: 1,
          name: "Archive",
          role: "archive",
          displayName: "Archive",
          unreadCount: 0,
        },
        // A Gmail-style folder: the wire name stays modified UTF-7, the
        // backend ships the decoded label alongside it.
        {
          id: 3,
          accountId: 1,
          name: "[Gmail]/Odoslan&AOk-",
          role: "sent",
          displayName: "Odoslané",
          unreadCount: 0,
        },
      ]
    : [];
}

vi.mock("./lib/api", () => ({
  listAccounts: vi.fn(async () => currentAccounts),
  listAliases: vi.fn(async () => currentAliases),
  listMailboxes: vi.fn(defaultListMailboxes),
  listMessages: vi.fn(async (accountId: number | null, mailbox = "INBOX") => {
    const pool = mailbox === "Archive" ? archivedMessages : currentMessages;
    return accountId === null
      ? pool
      : pool.filter((m) => m.accountId === accountId);
  }),
  // Whole-view totals derived from the same canned pool the list serves;
  // cached == serverTotal, so no backfill progress strip in these tests.
  viewStatus: vi.fn(async (accountId: number | null, mailbox = "INBOX") => {
    const pool = mailbox === "Archive" ? archivedMessages : currentMessages;
    const rows =
      accountId === null
        ? pool
        : pool.filter((m) => m.accountId === accountId);
    return {
      listRows: rows.length,
      unread: rows.filter((m) => !m.read).length,
      cached: rows.length,
      serverTotal: rows.length,
    };
  }),
  // why: a canned single-hit result — App tests only assert the wiring
  // (what was called with what); real matching is covered by Rust tests.
  searchMessages: vi.fn(async () => ({
    messages: [currentMessages[1]],
    corrections: [],
  })),
  // The view's conversation fetch: empty = fall back to the selected row.
  listThread: vi.fn(async () => []),
  // Bulk bodies for the conversation view, keyed by message id — serve the
  // canned body under the anchor so the view pane shows it.
  threadBodies: vi.fn(async (messageId: number) => ({
    [messageId]: {
      html: null,
      text: "body text",
      blockedImages: 0,
      canLoadRemote: false,
      attachments: [],
    },
  })),
  getMessageBody: vi.fn(async () => ({
    html: null,
    text: "body text",
    quotedText: null,
    blockedImages: 0,
    canLoadRemote: false,
    attachments: [],
  })),
  getMessageQuote: vi.fn(async () => ({
    html: "<p>body text</p>",
    text: "body text",
  })),
  syncAccount: vi.fn(async () => undefined),
  refreshAccount: vi.fn(async () => undefined),
  setMessageRead: vi.fn(async () => undefined),
  moveToTrash: vi.fn(async () => undefined),
  archiveMessage: vi.fn(async () => undefined),
  moveMessage: vi.fn(async () => undefined),
  trashThread: vi.fn(async () => undefined),
  archiveThread: vi.fn(async () => undefined),
  moveThread: vi.fn(async () => undefined),
  trashMessages: vi.fn(async () => undefined),
  archiveMessages: vi.fn(async () => undefined),
  moveMessages: vi.fn(async () => undefined),
  setMessagesRead: vi.fn(async () => undefined),
  openCompose: vi.fn(async () => undefined),
  openDraft: vi.fn(async () => undefined),
  openSettings: vi.fn(async () => undefined),
  onAccountsChanged: vi.fn(async (callback: () => void) => {
    accountsChanged = callback;
    return () => {};
  }),
  onMessagesChanged: vi.fn(async (callback: (change: MessagesChanged) => void) => {
    messagesChanged = callback;
    return () => {};
  }),
  undoSend: vi.fn(async () => undefined),
  onSendQueued: vi.fn(async (callback: (e: SendEvent) => void) => {
    sendQueued = callback;
    return () => {};
  }),
  onSendFinished: vi.fn(async (callback: (e: SendEvent) => void) => {
    sendFinished = callback;
    return () => {};
  }),
  onSendUndone: vi.fn(async (callback: (e: SendEvent) => void) => {
    sendUndone = callback;
    return () => {};
  }),
  getThreadOrder: vi.fn(async () => "newestLast"),
  // Avatar lookups are off by default, so the command resolves to nothing.
  loadDomainAvatars: vi.fn(async () => ({})),
  getSwipeActions: vi.fn(async () => currentSwipeActions),
  getShortcuts: vi.fn(async () => currentShortcuts),
  getDateTimeFormat: vi.fn(async () => currentDateTimeFormat),
  onLlmModelsChanged: vi.fn(async () => () => {}),
  llmStatus: vi.fn(async () => ({
    enabled: false,
    activeModel: null,
    ready: false,
    models: [],
  })),
  onSettingsChanged: vi.fn(async (callback: () => void) => {
    settingsChanged = callback;
    return () => {};
  }),
  listScheduled: vi.fn(async () => currentScheduled),
  sendScheduledNow: vi.fn(async () => undefined),
  cancelScheduled: vi.fn(async () => undefined),
  onScheduledMissed: vi.fn(async (callback: () => void) => {
    scheduledMissed = callback;
    return () => {};
  }),
  onScheduledChanged: vi.fn(async (callback: () => void) => {
    scheduledChanged = callback;
    return () => {};
  }),
}));

import * as api from "./lib/api";
import { PANE_WIDTHS_KEY } from "./lib/paneSizes";
import App from "./App.svelte";

beforeEach(() => {
  currentAccounts = [...accounts];
  currentAliases = [];
  currentMessages = [...allMessages];
  currentScheduled = [];
  accountsChanged = undefined;
  messagesChanged = undefined;
  sendQueued = undefined;
  sendFinished = undefined;
  sendUndone = undefined;
  scheduledMissed = undefined;
  scheduledChanged = undefined;
  settingsChanged = undefined;
  currentSwipeActions = { left: "archive", right: "toggleRead" };
  currentShortcuts = defaultShortcuts();
  currentDateTimeFormat = { date: "system", time: "system" };
  localStorage.clear();
  vi.clearAllMocks();
  vi.mocked(api.listMailboxes).mockImplementation(defaultListMailboxes);
  // Same leak-guard: tests override listThread with mockResolvedValue.
  vi.mocked(api.listThread).mockResolvedValue([]);
});

it("asks for the first page of rows, not the whole mailbox", async () => {
  render(App);

  await screen.findByText("Weekend plans");
  expect(api.listMessages).toHaveBeenCalledWith(null, "INBOX", 500);
});

it("loads accounts and the unified inbox on start", async () => {
  render(App);

  expect(
    await screen.findByRole("button", { name: "All Inboxes" }),
  ).toBeInTheDocument();
  expect(screen.getByText("Personal")).toBeInTheDocument();
  expect(screen.getByText("Work")).toBeInTheDocument();
  expect(await screen.findByText("Weekend plans")).toBeInTheDocument();
  expect(screen.getByText("Re: Invoice")).toBeInTheDocument();
  // the list pane header names the current mailbox and counts its messages,
  // calling out how many are still unread
  expect(
    screen.getByRole("heading", { name: "All Inboxes" }),
  ).toBeInTheDocument();
  expect(screen.getByText("2 messages, 1 unread")).toBeInTheDocument();
});

it("shows the selected message in the view pane", async () => {
  render(App);

  expect(screen.getByText("Select a message")).toBeInTheDocument();

  await fireEvent.click(await screen.findByText("Weekend plans"));

  expect(
    await screen.findByRole("heading", { name: "Weekend plans" }),
  ).toBeInTheDocument();
  expect(screen.queryByText("Select a message")).not.toBeInTheDocument();
});

it("filters the list when an account is selected", async () => {
  render(App);
  await screen.findByText("Weekend plans");

  await fireEvent.click(screen.getByText("Work"));

  expect(await screen.findByText("Re: Invoice")).toBeInTheDocument();
  expect(screen.queryByText("Weekend plans")).not.toBeInTheDocument();
});

it("expands an account and opens one of its folders", async () => {
  render(App);
  await screen.findByText("Weekend plans");

  await fireEvent.click(
    screen.getByRole("button", { name: "Toggle folders for Personal" }),
  );
  // why within(nav): the toolbar has an Archive action button too — the
  // sidebar folder is the one this test opens.
  const sidebar = within(screen.getByRole("navigation"));
  await fireEvent.click(await sidebar.findByRole("button", { name: "Archive" }));

  expect(await screen.findByText("Archived note")).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "Archive" })).toBeInTheDocument();
  expect(screen.queryByText("Weekend plans")).not.toBeInTheDocument();
});

it("shows decoded folder names but talks to the backend in wire names", async () => {
  const { listMessages } = await import("./lib/api");
  render(App);
  await screen.findByText("Weekend plans");

  await fireEvent.click(
    screen.getByRole("button", { name: "Toggle folders for Personal" }),
  );
  // The sidebar and list header show the decoded label…
  await fireEvent.click(await screen.findByRole("button", { name: "Odoslané" }));
  expect(
    await screen.findByRole("heading", { name: "Odoslané" }),
  ).toBeInTheDocument();
  expect(screen.queryByText("[Gmail]/Odoslan&AOk-")).not.toBeInTheDocument();

  // …while the IMAP wire name is what the backend receives.
  expect(listMessages).toHaveBeenLastCalledWith(1, "[Gmail]/Odoslan&AOk-", 500);
});

it("hides the folder toggle for accounts with no extra folders", async () => {
  render(App);
  await screen.findByText("Weekend plans");

  expect(
    screen.getByRole("button", { name: "Toggle folders for Personal" }),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: "Toggle folders for Work" }),
  ).not.toBeInTheDocument();
});

it("clears the selected message when switching accounts", async () => {
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });

  await fireEvent.click(screen.getByText("Work"));

  expect(await screen.findByText("Select a message")).toBeInTheDocument();
});

it("opens the settings window from the toolbar gear", async () => {
  render(App);
  await screen.findByText("Weekend plans");

  await fireEvent.click(screen.getByRole("button", { name: "Settings" }));

  await waitFor(() => expect(api.openSettings).toHaveBeenCalled());
});

it("a new message starts from the account's default send-as identity", async () => {
  currentAccounts = [{ ...accounts[0], defaultAliasId: 5 }, accounts[1]];
  render(App);
  await screen.findByText("Weekend plans");

  await fireEvent.click(screen.getByRole("button", { name: "New Message" }));

  await waitFor(() =>
    expect(api.openCompose).toHaveBeenCalledWith(
      expect.objectContaining({ accountId: 1, aliasId: 5 }),
    ),
  );
});

it("opens a compose window for a new message", async () => {
  render(App);
  await screen.findByText("Weekend plans");

  await fireEvent.click(screen.getByRole("button", { name: "New Message" }));

  // the unified inbox has no account selected — From falls back to the first
  await waitFor(() =>
    expect(api.openCompose).toHaveBeenCalledWith({
      accountId: 1,
      to: "",
      subject: "",
      body: "",
    }),
  );
});

it("a reply leaves from the alias the mail was addressed to", async () => {
  // Message 1's To line reads "domco@example.com, Bob <bob@example.com>" —
  // make the second entry an alias of account 1.
  currentAliases = [
    { id: 10, accountId: 1, name: "Bob", email: "bob@example.com" },
  ];
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });
  await screen.findByText("body text");

  await fireEvent.click(screen.getByRole("button", { name: "Reply" }));

  await waitFor(() =>
    expect(api.openCompose).toHaveBeenCalledWith(
      expect.objectContaining({ accountId: 1, aliasId: 10 }),
    ),
  );
});

it("opens a reply draft from the account the message arrived on", async () => {
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });
  await screen.findByText("body text");

  await fireEvent.click(screen.getByRole("button", { name: "Reply" }));

  await waitFor(() =>
    expect(api.openCompose).toHaveBeenCalledWith({
      accountId: 1,
      to: "alice@example.com",
      subject: "Re: Weekend plans",
      body: "",
      quote: {
        attribution: expect.stringContaining("wrote:"),
        html: "<p>body text</p>",
        text: "body text",
      },
    }),
  );
});

it("toolbar reply acts on the thread's latest message, not the list row", async () => {
  // The row header only represents the newest message in the open folder —
  // after my own reply (filed in Sent) the conversation's true latest
  // message is the sent one, and that's what a follow-up must quote.
  const ownReply: MessageHeader = {
    ...allMessages[0],
    id: 50,
    mailbox: "[Gmail]/Odoslan&AOk-",
    from: "domco@example.com",
    to: "Alice <alice@example.com>",
    subject: "Re: Weekend plans",
    messageId: "own-reply@x",
    date: "2026-07-09T10:00:00Z",
  };
  vi.mocked(api.listThread).mockResolvedValue([allMessages[0], ownReply]);
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });
  // Wait for the whole thread to load — the older message's preview row
  // appears only once both messages are in, so Reply targets the newest.
  await screen.findByText("Are we still on for Saturday?", {
    selector: ".preview",
  });

  await fireEvent.click(screen.getByRole("button", { name: "Reply" }));

  await waitFor(() =>
    expect(api.openCompose).toHaveBeenCalledWith(
      expect.objectContaining({
        to: "Alice <alice@example.com>",
        inReplyTo: "own-reply@x",
      }),
    ),
  );
  expect(api.getMessageQuote).toHaveBeenCalledWith(50);
});

it("opens a reply-all draft without the receiving account's address", async () => {
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });
  await screen.findByText("body text");

  await fireEvent.click(screen.getByRole("button", { name: "Reply All" }));

  // Message 1 arrived on account 1 (domco@example.com) — that address
  // must vanish while Bob and the Cc line survive.
  await waitFor(() =>
    expect(api.openCompose).toHaveBeenCalledWith({
      accountId: 1,
      to: "Alice <alice@example.com>, Bob <bob@example.com>",
      cc: "carol@example.com",
      subject: "Re: Weekend plans",
      body: "",
      quote: {
        attribution: expect.stringContaining("wrote:"),
        html: "<p>body text</p>",
        text: "body text",
      },
    }),
  );
});

it("opens a forward draft with the original below a header block", async () => {
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });
  await screen.findByText("body text");

  await fireEvent.click(screen.getByRole("button", { name: "Forward" }));

  await waitFor(() =>
    expect(api.openCompose).toHaveBeenCalledWith({
      accountId: 1,
      to: "",
      subject: "Fwd: Weekend plans",
      body: expect.stringContaining("---------- Forwarded message ----------"),
    }),
  );
});

it("starts a sync for every account on launch", async () => {
  render(App);
  await screen.findByText("Weekend plans");

  await waitFor(() => {
    expect(api.syncAccount).toHaveBeenCalledWith(1);
    expect(api.syncAccount).toHaveBeenCalledWith(2);
  });
});

it("syncs accounts in parallel, not one after another", async () => {
  const started: number[] = [];
  let releaseSyncs!: () => void;
  const gate = new Promise<void>((resolve) => (releaseSyncs = resolve));
  // why mockImplementationOnce: consumed by this test's two calls, so later
  // tests fall back to the factory's instantly-resolving sync.
  const gatedSync = async (id: number) => {
    started.push(id);
    await gate;
  };
  vi.mocked(api.syncAccount)
    .mockImplementationOnce(gatedSync)
    .mockImplementationOnce(gatedSync);

  render(App);

  // A sequential loop would await account 1's sync (blocked on the gate)
  // before ever starting account 2's.
  await waitFor(() => expect(started).toEqual([1, 2]));
  releaseSyncs();
});

it("pull-to-refresh re-syncs the view and pins the drawer until syncs land", async () => {
  render(App);
  await screen.findByText("Weekend plans");
  vi.mocked(api.refreshAccount).mockClear();
  // Cleared too: the launch sync already called it, and the point below is
  // that the pull itself does not.
  vi.mocked(api.syncAccount).mockClear();

  let releaseSyncs!: () => void;
  const gate = new Promise<void>((resolve) => (releaseSyncs = resolve));
  // why mockImplementationOnce: consumed by this pull's two calls, so later
  // tests fall back to the factory's instantly-resolving sync.
  const gatedSync = async () => {
    await gate;
  };
  vi.mocked(api.refreshAccount)
    .mockImplementationOnce(gatedSync)
    .mockImplementationOnce(gatedSync);

  // Two upward pulses past the trigger; the quiet gap after is the release.
  const list = screen.getByRole("listbox", { name: "Messages" });
  await fireEvent.wheel(list, { deltaY: -80 });
  await fireEvent.wheel(list, { deltaY: -80 });

  // The unified inbox is visible — the pull re-syncs every account. It goes
  // through refreshAccount, not syncAccount: a pull is the user asking on
  // purpose, so folders reconcile in full.
  await screen.findByText(/Checking for new mail/);
  expect(api.refreshAccount).toHaveBeenCalledWith(1);
  expect(api.refreshAccount).toHaveBeenCalledWith(2);
  expect(api.syncAccount).not.toHaveBeenCalled();

  releaseSyncs();
  await waitFor(() =>
    expect(screen.queryByText(/Checking for new mail/)).toBeNull(),
  );
});

it("refreshes the list on messages-changed and keeps the selection", async () => {
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });

  currentMessages = [
    ...allMessages,
    {
      id: 3,
      accountId: 1,
      mailbox: "INBOX",
      from: "New Sender",
      to: "hello@vocalio.sk",
      cc: "",
      replyTo: "",
    bcc: "",
      messageId: "",
      references: "",
      threadCount: 1,
      threadUnread: false,
      isDraft: false,
      threadHasDraft: false,
      subject: "Brand new",
      snippet: "",
      date: "2026-07-09T00:00:00Z",
      read: false,
    hasAttachments: false,
    },
  ];
  messagesChanged?.(RELOAD);

  expect(await screen.findByText("Brand new")).toBeInTheDocument();
  expect(
    screen.getByRole("heading", { name: "Weekend plans" }),
  ).toBeInTheDocument();
});

it("collapses a burst of messages-changed events into one refresh", async () => {
  render(App);
  await screen.findByText("Weekend plans");

  currentMessages = [
    ...allMessages,
    { ...allMessages[0], id: 3, subject: "Brand new" },
  ];
  vi.mocked(api.listMessages).mockClear();
  vi.mocked(api.listMailboxes).mockClear();
  // A backfilling account fires messages-changed once per cached batch —
  // three quick events must cost one list query, not three.
  // why fake timers mid-test: the initial load must run on real timers
  // (findBy* polls with them), only the debounce window itself is faked.
  vi.useFakeTimers();
  messagesChanged?.(RELOAD);
  messagesChanged?.(RELOAD);
  messagesChanged?.(RELOAD);
  await vi.advanceTimersByTimeAsync(250);
  vi.useRealTimers();

  expect(api.listMessages).toHaveBeenCalledTimes(1);
  expect(api.listMailboxes).toHaveBeenCalledTimes(currentAccounts.length);
  expect(await screen.findByText("Brand new")).toBeInTheDocument();
});

// why fake timers mid-test: the initial load must run on real timers
// (findBy* polls with them), only the debounce window itself is faked.
async function typeIntoSearch(value: string) {
  const input = screen.getByRole("searchbox", { name: "Search messages" });
  vi.useFakeTimers();
  await fireEvent.input(input, { target: { value } });
  await vi.advanceTimersByTimeAsync(250);
  vi.useRealTimers();
}

it("runs one debounced search and shows its results", async () => {
  render(App);
  await screen.findByText("Weekend plans");

  const input = screen.getByRole("searchbox", { name: "Search messages" });
  vi.useFakeTimers();
  await fireEvent.input(input, { target: { value: "from:pet" } });
  await fireEvent.input(input, { target: { value: "from:peter" } });
  await vi.advanceTimersByTimeAsync(250);
  vi.useRealTimers();

  expect(api.searchMessages).toHaveBeenCalledTimes(1);
  expect(api.searchMessages).toHaveBeenCalledWith(null, "from:peter");
  expect(await screen.findByText("Re: Invoice")).toBeInTheDocument();
  expect(screen.queryByText("Weekend plans")).not.toBeInTheDocument();
});

it("clearing the search restores the plain list", async () => {
  render(App);
  await screen.findByText("Weekend plans");
  await typeIntoSearch("invoice");
  await screen.findByText("Re: Invoice");

  await typeIntoSearch("");

  expect(await screen.findByText("Weekend plans")).toBeInTheDocument();
  // an empty query never hits the search backend
  expect(api.searchMessages).toHaveBeenCalledTimes(1);
});

it("opens the search hit that was clicked, not the newest of its thread", async () => {
  // The clicked hit's own id (999) is deliberately absent from the thread —
  // the server-side-copy dedup situation — so only its Message-ID finds it.
  vi.mocked(api.searchMessages).mockResolvedValueOnce({
    messages: [
      { ...allMessages[1], id: 999, messageId: "old@x", snippet: "older hit" },
    ],
    corrections: [],
  });
  vi.mocked(api.listThread).mockResolvedValueOnce([
    { ...allMessages[1], id: 50, messageId: "old@x" },
    { ...allMessages[1], id: 51, messageId: "new@x", date: "2026-07-09T00:00:00Z" },
  ]);
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    50: {
      html: null,
      text: "older body",
      quotedText: null,
      blockedImages: 0,
      canLoadRemote: false,
      attachments: [],
      auth: null,
      senderAnomaly: null,
      language: null,
    },
    51: {
      html: null,
      text: "newer body",
      quotedText: null,
      blockedImages: 0,
      canLoadRemote: false,
      attachments: [],
      auth: null,
      senderAnomaly: null,
      language: null,
    },
  });

  render(App);
  await screen.findByText("Weekend plans");
  await typeIntoSearch("invoice");
  await fireEvent.click(await screen.findByText("Re: Invoice"));

  expect(await screen.findByText("older body")).toBeInTheDocument();
  expect(screen.queryByText("newer body")).not.toBeInTheDocument();
});

it("says when a search swapped a typo for the word it meant", async () => {
  vi.mocked(api.searchMessages).mockResolvedValueOnce({
    messages: [allMessages[1]],
    corrections: [{ typed: "bnny", corrected: "bunny" }],
  });

  render(App);
  await screen.findByText("Weekend plans");
  await typeIntoSearch("bnny");

  expect(await screen.findByText(/Showing results for/)).toHaveTextContent(
    "bunny instead of bnny",
  );

  // The line belongs to the results — leaving the search drops it.
  await typeIntoSearch("");
  await screen.findByText("Weekend plans");
  expect(screen.queryByText(/Showing results for/)).not.toBeInTheDocument();
});

it("scopes the search to the selected account", async () => {
  render(App);
  await screen.findByText("Weekend plans");
  await fireEvent.click(screen.getByText("Work"));
  await screen.findByText("Re: Invoice");

  await typeIntoSearch("faktura");

  expect(api.searchMessages).toHaveBeenCalledWith(2, "faktura");
});

it("refreshes accounts when another window changes them", async () => {
  render(App);
  await screen.findByText("Work");

  currentAccounts = [
    ...accounts,
    { ...accounts[0], id: 3, name: "Third", email: "third@example.com" },
  ];
  accountsChanged?.();

  expect(await screen.findByText("Third")).toBeInTheDocument();
});

it("syncs a newly added account immediately", async () => {
  render(App);
  await screen.findByText("Work");

  currentAccounts = [
    ...accounts,
    { ...accounts[0], id: 3, name: "Third", email: "third@example.com" },
  ];
  accountsChanged?.();
  await screen.findByText("Third");

  await waitFor(() => expect(api.syncAccount).toHaveBeenCalledWith(3));
});

// why MouseEvent: jsdom has no PointerEvent constructor; a MouseEvent with a
// pointer event type still reaches the pointerdown/... listeners with clientX.
// bubbles is required — Svelte 5 delegates onpointerdown to the app root.
async function dragSeparator(
  separator: HTMLElement,
  fromX: number,
  toX: number,
) {
  await fireEvent(
    separator,
    new MouseEvent("pointerdown", { clientX: fromX, bubbles: true }),
  );
  await fireEvent(
    separator,
    new MouseEvent("pointermove", { clientX: toX, bubbles: true }),
  );
  await fireEvent(separator, new MouseEvent("pointerup", { bubbles: true }));
}

it("resizes the sidebar by dragging its divider and persists the width", async () => {
  render(App);
  const separator = await screen.findByRole("separator", {
    name: "Resize sidebar",
  });

  await dragSeparator(separator, 208, 258);

  expect(separator).toHaveAttribute("aria-valuenow", "258");
  expect(JSON.parse(localStorage.getItem(PANE_WIDTHS_KEY)!)).toEqual({
    sidebar: 258,
    list: 352,
  });
});

it("clamps a drag past the pane's minimum width", async () => {
  render(App);
  const separator = await screen.findByRole("separator", {
    name: "Resize message list",
  });

  await dragSeparator(separator, 560, 0);

  expect(separator).toHaveAttribute("aria-valuenow", "240");
});

it("restores saved pane widths on start", async () => {
  localStorage.setItem(
    PANE_WIDTHS_KEY,
    JSON.stringify({ sidebar: 300, list: 400 }),
  );
  render(App);

  const separator = await screen.findByRole("separator", {
    name: "Resize sidebar",
  });
  expect(separator).toHaveAttribute("aria-valuenow", "300");
});

it("resizes a pane with arrow keys on the focused divider", async () => {
  render(App);
  const separator = await screen.findByRole("separator", {
    name: "Resize sidebar",
  });

  await fireEvent.keyDown(separator, { key: "ArrowRight" });
  await fireEvent.keyDown(separator, { key: "ArrowRight" });
  await fireEvent.keyDown(separator, { key: "ArrowLeft" });

  expect(separator).toHaveAttribute("aria-valuenow", "224");
  expect(
    JSON.parse(localStorage.getItem(PANE_WIDTHS_KEY)!).sidebar,
  ).toBe(224);
});

it("falls back to the unified inbox when the selected account disappears", async () => {
  render(App);
  await fireEvent.click(await screen.findByText("Work"));
  await screen.findByText("Re: Invoice");

  currentAccounts = [accounts[0]];
  accountsChanged?.();

  expect(await screen.findByText("Weekend plans")).toBeInTheDocument();
  expect(screen.queryByText("Work")).not.toBeInTheDocument();
});

it("shows a sending badge whose undo hands the message back", async () => {
  render(App);
  await screen.findByText("Weekend plans");

  sendQueued?.({ id: 4, subject: "Ahoj", error: null, undoMs: 8000 });
  expect(await screen.findByText("Sending: Ahoj")).toBeInTheDocument();

  await fireEvent.click(screen.getByRole("button", { name: "Undo" }));
  expect(api.undoSend).toHaveBeenCalledWith(4);

  // the badge leaves on the backend's confirmation, not on the click
  sendUndone?.({ id: 4, subject: "Ahoj", error: null, undoMs: 0 });
  await waitFor(() =>
    expect(screen.queryByText("Sending: Ahoj")).not.toBeInTheDocument(),
  );
});

it("confirms a delivered send and hides the badge on its own", async () => {
  render(App);
  await screen.findByText("Weekend plans");
  sendQueued?.({ id: 5, subject: "Ahoj", error: null, undoMs: 8000 });
  await screen.findByText("Sending: Ahoj");

  vi.useFakeTimers();
  sendFinished?.({ id: 5, subject: "Ahoj", error: null, undoMs: 0 });
  await vi.advanceTimersByTimeAsync(0);
  expect(screen.getByText("Sent: Ahoj")).toBeInTheDocument();

  await vi.advanceTimersByTimeAsync(3000);
  vi.useRealTimers();

  expect(screen.queryByText("Sent: Ahoj")).not.toBeInTheDocument();
});

it("shows a failure badge with the delivery error", async () => {
  render(App);
  await screen.findByText("Weekend plans");
  sendQueued?.({ id: 6, subject: "Ahoj", error: null, undoMs: 8000 });
  await screen.findByText("Sending: Ahoj");

  vi.useFakeTimers();
  sendFinished?.({ id: 6, subject: "Ahoj", error: "smtp error: relay refused", undoMs: 0 });
  await vi.advanceTimersByTimeAsync(0);
  vi.useRealTimers();

  expect(screen.getByText("Couldn't send: Ahoj")).toBeInTheDocument();
  expect(screen.getByText("smtp error: relay refused")).toBeInTheDocument();
});

it("navigates the message list with Arrow Down / Up", async () => {
  render(App);
  const first = (await screen.findByText("Weekend plans")).closest(
    '[role="option"]',
  );
  const second = screen.getByText("Re: Invoice").closest('[role="option"]');
  expect(first).toHaveAttribute("aria-selected", "false");

  await fireEvent.keyDown(document.body, { key: "ArrowDown" });
  expect(first).toHaveAttribute("aria-selected", "true");

  await fireEvent.keyDown(document.body, { key: "ArrowDown" });
  expect(second).toHaveAttribute("aria-selected", "true");
  expect(first).toHaveAttribute("aria-selected", "false");

  await fireEvent.keyDown(document.body, { key: "ArrowUp" });
  expect(first).toHaveAttribute("aria-selected", "true");
});

/** Render the app and wait until its shortcut bindings have loaded. */
async function renderWithShortcuts() {
  render(App);
  await screen.findByText("Weekend plans");
  await waitFor(() => expect(api.getShortcuts).toHaveBeenCalled());
}

it("opens a new message with ⌘N", async () => {
  await renderWithShortcuts();

  await fireEvent.keyDown(document.body, { key: "n", metaKey: true });

  await waitFor(() =>
    expect(api.openCompose).toHaveBeenCalledWith({
      accountId: 1,
      to: "",
      subject: "",
      body: "",
    }),
  );
});

it("collapses the sidebar with ⌃⌘S", async () => {
  await renderWithShortcuts();

  await fireEvent.keyDown(document.body, {
    key: "s",
    ctrlKey: true,
    metaKey: true,
  });

  expect(
    screen.queryByRole("button", { name: "All Inboxes" }),
  ).not.toBeInTheDocument();
});

it("trashes the open message with ⌫, but not while typing in search", async () => {
  await renderWithShortcuts();
  await fireEvent.click(screen.getByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });

  const search = screen.getByRole("searchbox", { name: "Search messages" });
  await fireEvent.keyDown(search, { key: "Backspace" });
  expect(api.moveToTrash).not.toHaveBeenCalled();

  await fireEvent.keyDown(document.body, { key: "Backspace" });
  expect(api.moveToTrash).toHaveBeenCalledWith(1);
});

it("ignores message shortcuts while nothing is selected", async () => {
  await renderWithShortcuts();

  await fireEvent.keyDown(document.body, { key: "Backspace" });
  await fireEvent.keyDown(document.body, { key: "r", metaKey: true });
  await fireEvent.keyDown(document.body, {
    key: "a",
    ctrlKey: true,
    metaKey: true,
  });

  expect(api.moveToTrash).not.toHaveBeenCalled();
  expect(api.archiveMessage).not.toHaveBeenCalled();
  expect(api.openCompose).not.toHaveBeenCalled();
});

it("puts focus in the search field with ⌘F", async () => {
  await renderWithShortcuts();

  await fireEvent.keyDown(document.body, { key: "f", metaKey: true });

  expect(
    screen.getByRole("searchbox", { name: "Search messages" }),
  ).toHaveFocus();
});

it("follows a rebinding made in the settings window", async () => {
  await renderWithShortcuts();

  currentShortcuts = defaultShortcuts().map((binding) =>
    binding.action === "new-message"
      ? { ...binding, combo: "Alt+Meta+M" }
      : binding,
  );
  settingsChanged?.();
  await waitFor(() => expect(api.getShortcuts).toHaveBeenCalledTimes(2));

  await fireEvent.keyDown(document.body, { key: "n", metaKey: true });
  expect(api.openCompose).not.toHaveBeenCalled();

  // ⌥ turns M into µ on a Mac — the physical key still counts.
  await fireEvent.keyDown(document.body, {
    key: "µ",
    code: "KeyM",
    altKey: true,
    metaKey: true,
  });
  await waitFor(() => expect(api.openCompose).toHaveBeenCalledTimes(1));
});

it("marks an unread message read when it is opened", async () => {
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  expect(api.setMessageRead).toHaveBeenCalledWith(1, true);
});

it("does not touch the read state of an already-read message", async () => {
  render(App);
  await fireEvent.click(await screen.findByText("Re: Invoice"));
  expect(api.setMessageRead).not.toHaveBeenCalled();
});

it("adds a row to the selection on cmd-click", async () => {
  render(App);
  const first = (await screen.findByText("Weekend plans")).closest(
    '[role="option"]',
  );
  const second = screen.getByText("Re: Invoice").closest('[role="option"]');

  await fireEvent.click(screen.getByText("Weekend plans"));
  await fireEvent.click(screen.getByText("Re: Invoice"), { metaKey: true });

  expect(first).toHaveAttribute("aria-selected", "true");
  expect(second).toHaveAttribute("aria-selected", "true");
});

it("drops a row from the selection when it is cmd-clicked again", async () => {
  render(App);
  // why scoped to the list: the open message repeats its subject in the
  // reading pane, so a bare getByText would match two nodes.
  const list = within(screen.getByRole("listbox"));
  const first = (await list.findByText("Weekend plans")).closest(
    '[role="option"]',
  );

  await fireEvent.click(list.getByText("Weekend plans"));
  await fireEvent.click(list.getByText("Weekend plans"), { metaKey: true });

  expect(first).toHaveAttribute("aria-selected", "false");
});

// why: cmd-click gathers rows for a bulk action — the user is not reading
// them, so the unread dot has to stay put.
it("does not mark a message read when it is cmd-clicked", async () => {
  render(App);
  const list = within(screen.getByRole("listbox"));
  await fireEvent.click(await list.findByText("Re: Invoice"));
  await fireEvent.click(list.getByText("Weekend plans"), { metaKey: true });

  expect(api.setMessageRead).not.toHaveBeenCalled();
});

it("selects the span between the anchor and a shift-click", async () => {
  render(App);
  const first = (await screen.findByText("Weekend plans")).closest(
    '[role="option"]',
  );
  const second = screen.getByText("Re: Invoice").closest('[role="option"]');

  await fireEvent.click(screen.getByText("Weekend plans"));
  await fireEvent.click(screen.getByText("Re: Invoice"), { shiftKey: true });

  expect(first).toHaveAttribute("aria-selected", "true");
  expect(second).toHaveAttribute("aria-selected", "true");
});

it("collapses a multi-selection back to one row on an arrow key", async () => {
  render(App);
  const first = (await screen.findByText("Weekend plans")).closest(
    '[role="option"]',
  );
  const second = screen.getByText("Re: Invoice").closest('[role="option"]');

  await fireEvent.click(screen.getByText("Weekend plans"));
  await fireEvent.click(screen.getByText("Re: Invoice"), { metaKey: true });
  await fireEvent.keyDown(document.body, { key: "ArrowDown" });

  expect(first).toHaveAttribute("aria-selected", "false");
  expect(second).toHaveAttribute("aria-selected", "true");
});

it("marks message rows of colored accounts with a dot", async () => {
  currentAccounts = [{ ...accounts[0], color: "#ff9f0a" }, accounts[1]];
  const { container } = render(App);
  await screen.findByText("Weekend plans");

  // Only account 1 has a color, so only its message shows a dot.
  const dots = container.querySelectorAll<HTMLElement>(".account-dot");
  expect(dots).toHaveLength(1);
  expect(dots[0].style.background).not.toBe("");
});

it("collapses and restores the sidebar with the toggle button", async () => {
  render(App);
  expect(
    await screen.findByRole("button", { name: "All Inboxes" }),
  ).toBeInTheDocument();

  await fireEvent.click(screen.getByRole("button", { name: "Toggle sidebar" }));
  expect(
    screen.queryByRole("button", { name: "All Inboxes" }),
  ).not.toBeInTheDocument();

  await fireEvent.click(screen.getByRole("button", { name: "Toggle sidebar" }));
  expect(
    screen.getByRole("button", { name: "All Inboxes" }),
  ).toBeInTheDocument();
});

it("trashes the open message and steps to its neighbour", async () => {
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });

  await fireEvent.click(screen.getByRole("button", { name: "Trash" }));

  expect(api.moveToTrash).toHaveBeenCalledWith(1);
  // The row leaves the list at once (optimistic), selection steps to the next.
  expect(screen.queryByText("Weekend plans")).not.toBeInTheDocument();
  await screen.findByRole("heading", { name: "Re: Invoice" });
});

it("routes a thread row's trash and archive to the whole conversation", async () => {
  currentMessages = [
    { ...allMessages[0], threadCount: 2, threadUnread: false },
    allMessages[1],
  ];
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });

  await fireEvent.click(screen.getByRole("button", { name: "Trash" }));

  expect(api.trashThread).toHaveBeenCalledWith(1);
  expect(api.moveToTrash).not.toHaveBeenCalled();
});

it("archives the open message and steps to its neighbour", async () => {
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });

  await fireEvent.click(screen.getByRole("button", { name: "Archive" }));

  expect(api.archiveMessage).toHaveBeenCalledWith(1);
  expect(screen.queryByText("Weekend plans")).not.toBeInTheDocument();
  await screen.findByRole("heading", { name: "Re: Invoice" });
});

it("restores the message when trashing fails on the server", async () => {
  vi.mocked(api.moveToTrash).mockRejectedValueOnce("imap error: no trash");
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));

  await fireEvent.click(screen.getByRole("button", { name: "Trash" }));

  // Optimistically removed, then brought back by the failure re-query.
  await waitFor(() =>
    expect(screen.getByText("Weekend plans")).toBeInTheDocument(),
  );
});

it("keeps an evicted row out of the list while the server move is pending", async () => {
  // The archive hangs server-side (TLS + LOGIN + MOVE can take seconds).
  let finishArchive!: () => void;
  vi.mocked(api.archiveMessage).mockImplementationOnce(
    () =>
      new Promise<void>((resolve) => {
        finishArchive = resolve;
      }),
  );
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });

  await fireEvent.click(screen.getByRole("button", { name: "Archive" }));
  expect(screen.queryByText("Weekend plans")).not.toBeInTheDocument();

  // A background sync lands mid-move while the cache still holds the row.
  // The new arrival proves the refresh completed — the evicted row must
  // not ride back in with it.
  currentMessages = [
    ...allMessages,
    { ...allMessages[0], id: 3, subject: "Brand new" },
  ];
  messagesChanged?.(RELOAD);
  await screen.findByText("Brand new");
  expect(screen.queryByText("Weekend plans")).not.toBeInTheDocument();

  // Server confirms: the backend drops the cached row, the follow-up
  // refresh keeps the list in its final state.
  currentMessages = currentMessages.filter((m) => m.id !== 1);
  finishArchive();
  await screen.findByText("Brand new");
  expect(screen.queryByText("Weekend plans")).not.toBeInTheDocument();
});

it("trashes a row when the left swipe is configured to trash", async () => {
  currentSwipeActions = { left: "trash", right: "reply" };
  render(App);
  const row = await screen.findByRole("option", { name: /Weekend plans/ });

  // why fake timers mid-test: only the gesture's settle window is faked —
  // the initial load polls with real timers (findBy*).
  vi.useFakeTimers();
  await fireEvent.wheel(row, { deltaX: 60 });
  await fireEvent.wheel(row, { deltaX: 60 });
  await vi.advanceTimersByTimeAsync(200);
  vi.useRealTimers();

  expect(api.moveToTrash).toHaveBeenCalledWith(1);
  expect(screen.queryByText("Weekend plans")).not.toBeInTheDocument();
});

it("opens a quoted reply when the right swipe is configured to reply", async () => {
  currentSwipeActions = { left: "trash", right: "reply" };
  render(App);
  const row = await screen.findByRole("option", { name: /Weekend plans/ });

  vi.useFakeTimers();
  await fireEvent.wheel(row, { deltaX: -120 });
  await vi.advanceTimersByTimeAsync(200);
  vi.useRealTimers();

  // The list has no body loaded — the reply fetches quote material.
  expect(api.getMessageQuote).toHaveBeenCalledWith(1);
  await waitFor(() =>
    expect(api.openCompose).toHaveBeenCalledWith({
      accountId: 1,
      to: "alice@example.com",
      subject: "Re: Weekend plans",
      body: "",
      quote: {
        attribution: expect.stringContaining("wrote:"),
        html: "<p>body text</p>",
        text: "body text",
      },
    }),
  );
});

it("writes list dates in the chosen format and follows a change", async () => {
  currentDateTimeFormat = { date: "dd.mm.yyyy", time: "system" };
  render(App);

  const row = await screen.findByRole("option", { name: /Weekend plans/ });
  await waitFor(() => expect(row).toHaveTextContent("07.07.2026"));

  // The settings window saves in its own JS context; this one only hears
  // the event — and the rows must re-render off the shared format.
  currentDateTimeFormat = { date: "yyyy-mm-dd", time: "system" };
  settingsChanged?.();

  await waitFor(() => expect(row).toHaveTextContent("2026-07-07"));
});

it("re-reads the swipe config when settings change", async () => {
  render(App);
  const row = await screen.findByRole("option", { name: /Weekend plans/ });

  currentSwipeActions = { left: "trash", right: "toggleRead" };
  settingsChanged?.();
  await waitFor(() =>
    expect(api.getSwipeActions).toHaveBeenCalledTimes(2),
  );

  vi.useFakeTimers();
  await fireEvent.wheel(row, { deltaX: 120 });
  await vi.advanceTimersByTimeAsync(200);
  vi.useRealTimers();

  expect(api.moveToTrash).toHaveBeenCalledWith(1);
  expect(api.archiveMessage).not.toHaveBeenCalled();
});

function missedEntry(over: Partial<ScheduledMessage> = {}): ScheduledMessage {
  return {
    id: 5,
    accountId: 1,
    aliasId: null,
    to: "alice@example.com",
    cc: "",
    bcc: "",
    subject: "Overdue hello",
    body: "",
    bodyHtml: null,
    attachments: [],
    scheduledAt: 1_752_600_000,
    status: "missed",
    inReplyTo: null,
    references: null,
    ...over,
  };
}

it("surfaces missed scheduled sends on start and sends on confirm", async () => {
  currentScheduled = [missedEntry()];
  render(App);

  expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
  expect(screen.getByText("Overdue hello")).toBeInTheDocument();

  await fireEvent.click(
    screen.getByRole("button", { name: "Send Overdue hello now" }),
  );

  expect(api.sendScheduledNow).toHaveBeenCalledWith(5);
  expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
});

it("shows newly missed sends when the backend flags them mid-run", async () => {
  render(App);
  await screen.findByRole("button", { name: "All Inboxes" });
  expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();

  currentScheduled = [missedEntry({ subject: "Slept through" })];
  scheduledMissed!();

  expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
  expect(screen.getByText("Slept through")).toBeInTheDocument();

  // Reopening as a draft unschedules the row.
  await fireEvent.click(
    screen.getByRole("button", { name: "Open Slept through as draft" }),
  );
  expect(api.cancelScheduled).toHaveBeenCalledWith(5);
});

it("opens a drafts-folder message in compose instead of the viewer", async () => {
  vi.mocked(api.listMailboxes).mockImplementation(async (accountId: number) =>
    accountId === 1
      ? [
          {
            id: 1,
            accountId: 1,
            name: "INBOX",
            role: "inbox",
            displayName: "INBOX",
            unreadCount: 0,
          },
          {
            id: 4,
            accountId: 1,
            name: "Drafts",
            role: "drafts",
            displayName: "Drafts",
            unreadCount: 0,
          },
        ]
      : [],
  );
  currentMessages = [
    ...allMessages,
    {
      id: 7,
      accountId: 1,
      mailbox: "Drafts",
      from: "",
      to: "jan",
      cc: "",
      replyTo: "",
    bcc: "",
      messageId: "",
      references: "",
      threadCount: 1,
      threadUnread: false,
      isDraft: false,
      threadHasDraft: false,
      subject: "Rozpísaný návrh",
      snippet: "",
      date: "2026-07-10T00:00:00Z",
      read: true,
    hasAttachments: false,
    },
  ];
  render(App);

  await fireEvent.click(await screen.findByText("Rozpísaný návrh"));

  await waitFor(() => expect(api.openDraft).toHaveBeenCalledWith(7));
  // The viewer stayed closed — no body fetch, no reading pane heading.
  expect(api.getMessageBody).not.toHaveBeenCalled();
  expect(
    screen.queryByRole("heading", { name: "Rozpísaný návrh" }),
  ).not.toBeInTheDocument();
});

it("lists pending scheduled sends in the sidebar and cancels one", async () => {
  currentScheduled = [
    missedEntry({ id: 7, subject: "Later today", status: "pending" }),
  ];
  render(App);

  const section = await screen.findByRole("list", {
    name: "Scheduled messages",
  });
  expect(section).toBeInTheDocument();
  expect(screen.getByText("Later today")).toBeInTheDocument();
  // pending rows belong to the sidebar, not the missed dialog
  expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();

  await fireEvent.click(
    screen.getByRole("button", { name: "Cancel scheduled Later today" }),
  );

  expect(api.cancelScheduled).toHaveBeenCalledWith(7);
  expect(screen.queryByText("Later today")).not.toBeInTheDocument();
});

it("refreshes the scheduled section when the backend broadcasts a change", async () => {
  render(App);
  await screen.findByRole("button", { name: "All Inboxes" });
  expect(screen.queryByText("Scheduled")).not.toBeInTheDocument();

  currentScheduled = [
    missedEntry({ id: 8, subject: "Tomorrow 8:00 mail", status: "pending" }),
  ];
  scheduledChanged!();

  expect(await screen.findByText("Tomorrow 8:00 mail")).toBeInTheDocument();
});

it("unarchives an archived message via Move to Inbox", async () => {
  render(App);
  await screen.findByText("Weekend plans");

  // Open Personal's Archive folder (the sidebar one, not the toolbar
  // action) and select the archived message.
  await fireEvent.click(
    screen.getByRole("button", { name: "Toggle folders for Personal" }),
  );
  const sidebar = within(screen.getByRole("navigation"));
  await fireEvent.click(await sidebar.findByRole("button", { name: "Archive" }));
  await fireEvent.click(await screen.findByText("Archived note"));

  // On an archived message the archive action flips to Move to Inbox…
  const button = await screen.findByRole("button", { name: "Move to Inbox" });

  // …and moves it back to the account's inbox instead of re-archiving.
  await fireEvent.click(button);
  expect(api.moveMessage).toHaveBeenCalledWith(9, "INBOX");
  expect(api.archiveMessage).not.toHaveBeenCalled();
});


it("tells the reading pane how many rows are staged for a bulk action", async () => {
  render(App);
  const list = within(screen.getByRole("listbox"));
  await fireEvent.click(await list.findByText("Weekend plans"));
  await fireEvent.click(list.getByText("Re: Invoice"), { metaKey: true });

  expect(screen.getByText("2 messages selected")).toBeInTheDocument();
});

/** Select both inbox rows: click the first, cmd-click the second. */
async function selectBoth() {
  const list = within(screen.getByRole("listbox"));
  await fireEvent.click(await list.findByText("Weekend plans"));
  await fireEvent.click(list.getByText("Re: Invoice"), { metaKey: true });
}

it("archives every selected row in one call", async () => {
  render(App);
  await selectBoth();
  await fireEvent.click(screen.getByRole("button", { name: "Archive" }));

  expect(api.archiveMessages).toHaveBeenCalledWith([1, 2], true);
  expect(api.archiveMessage).not.toHaveBeenCalled();
});

it("trashes every selected row in one call", async () => {
  render(App);
  await selectBoth();
  await fireEvent.click(screen.getByRole("button", { name: "Trash" }));

  expect(api.trashMessages).toHaveBeenCalledWith([1, 2], true);
});

// One unread among them means the button offers Mark Read for the lot.
it("marks a mixed selection read rather than unread", async () => {
  render(App);
  await selectBoth();
  await fireEvent.click(screen.getByRole("button", { name: "Mark Read" }));

  expect(api.setMessagesRead).toHaveBeenCalledWith([1, 2], true, true);
});

it("drops every selected row from the list at once", async () => {
  render(App);
  await selectBoth();
  await fireEvent.click(screen.getByRole("button", { name: "Trash" }));

  await waitFor(() =>
    expect(screen.queryByText("Weekend plans")).not.toBeInTheDocument(),
  );
  expect(screen.queryByText("Re: Invoice")).not.toBeInTheDocument();
});

/** A third inbox row, older than the other two, so it lists last. */
const lunch: MessageHeader = {
  ...allMessages[0],
  id: 3,
  from: "Martin <martin@example.com>",
  subject: "Lunch?",
  date: "2026-07-06T11:00:00Z",
  read: false,
};

/** A full left swipe on a row — archive, under the default swipe config. */
async function swipeLeft(row: HTMLElement) {
  // why fake timers only here: the initial load polls with real ones.
  vi.useFakeTimers();
  await fireEvent.wheel(row, { deltaX: 60 });
  await fireEvent.wheel(row, { deltaX: 60 });
  await vi.advanceTimersByTimeAsync(200);
  vi.useRealTimers();
}

// why: a swipe is aimed at one row — reaching for a selection gathered
// elsewhere would archive mail the user never touched.
it("archives only the swiped row when it sits outside the selection", async () => {
  currentMessages = [...allMessages, lunch];
  render(App);
  await selectBoth();

  await swipeLeft(screen.getByRole("option", { name: /Lunch\?/ }));

  expect(api.archiveMessage).toHaveBeenCalledWith(3);
  expect(api.archiveMessages).not.toHaveBeenCalled();
});

// why: stepping to the neighbour is for the row being read. Swiping away a
// different row must leave the reading pane alone — opening the neighbour
// would also mark it read without the user ever looking at it.
it("keeps the open message when another row is swiped away", async () => {
  currentMessages = [...allMessages, lunch];
  render(App);
  await fireEvent.click(await screen.findByText("Weekend plans"));
  await screen.findByRole("heading", { name: "Weekend plans" });
  vi.mocked(api.setMessageRead).mockClear();

  // Lunch? is Re: Invoice's neighbour, and unread.
  await swipeLeft(screen.getByRole("option", { name: /Re: Invoice/ }));

  expect(api.archiveMessage).toHaveBeenCalledWith(2);
  expect(
    screen.getByRole("heading", { name: "Weekend plans" }),
  ).toBeInTheDocument();
  expect(api.setMessageRead).not.toHaveBeenCalled();
});

it("still uses the single-message command for one selected row", async () => {
  render(App);
  const list = within(screen.getByRole("listbox"));
  await fireEvent.click(await list.findByText("Re: Invoice"));
  await fireEvent.click(screen.getByRole("button", { name: "Archive" }));

  expect(api.archiveMessage).toHaveBeenCalledWith(2);
  expect(api.archiveMessages).not.toHaveBeenCalled();
});

it("extends the selection with shift and an arrow key", async () => {
  render(App);
  const list = within(screen.getByRole("listbox"));
  const first = (await list.findByText("Weekend plans")).closest(
    '[role="option"]',
  );
  const second = list.getByText("Re: Invoice").closest('[role="option"]');

  await fireEvent.click(list.getByText("Weekend plans"));
  await fireEvent.keyDown(document.body, { key: "ArrowDown", shiftKey: true });

  expect(first).toHaveAttribute("aria-selected", "true");
  expect(second).toHaveAttribute("aria-selected", "true");
});

// Reading is what a plain arrow does; extending a selection is not reading.
it("does not mark a message read when shift-arrowing onto it", async () => {
  render(App);
  const list = within(screen.getByRole("listbox"));
  await fireEvent.click(await list.findByText("Re: Invoice"));
  await fireEvent.keyDown(document.body, { key: "ArrowUp", shiftKey: true });

  expect(api.setMessageRead).not.toHaveBeenCalled();
});

it("patches a read-flag change in place instead of re-querying the list", async () => {
  // why this matters: the list query is a full scan (~250 ms measured), and
  // clicking an unread message fired one every single time. Nothing about a
  // dot clearing can add or remove a row, so there is nothing to re-query.
  render(App);
  await screen.findByText("Weekend plans");
  vi.mocked(api.listMessages).mockClear();
  vi.mocked(api.viewStatus).mockClear();
  vi.mocked(api.listMailboxes).mockClear();

  messagesChanged?.({
    accountId: 1,
    read: { ids: [allMessages[0].id], read: true },
  });

  await waitFor(() =>
    expect(screen.getByRole("option", { name: /Weekend plans/ })).not.toHaveClass(
      "unread",
    ),
  );
  expect(api.listMessages).not.toHaveBeenCalled();
  expect(api.viewStatus).not.toHaveBeenCalled();
  expect(api.listMailboxes).not.toHaveBeenCalled();
});

it("asks for fresh counts when a read change names rows it cannot see", async () => {
  // A whole-thread mark-read names members that are not list rows, so the
  // view's unread tally cannot be adjusted locally — but the expensive list
  // query is still skipped.
  render(App);
  await screen.findByText("Weekend plans");
  vi.mocked(api.listMessages).mockClear();
  vi.mocked(api.viewStatus).mockClear();

  messagesChanged?.({
    accountId: 1,
    read: { ids: [allMessages[0].id, 9999], read: true },
  });

  await waitFor(() => expect(api.viewStatus).toHaveBeenCalledTimes(1));
  expect(api.listMessages).not.toHaveBeenCalled();
});
