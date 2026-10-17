import { expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import type { MessageBody, MessageHeader } from "./types";

function body(partial: Partial<MessageBody>): MessageBody {
  return {
    html: null,
    text: null,
    quotedText: null,
    blockedImages: 0,
    canLoadRemote: false,
    attachments: [],
    auth: null,
    senderAnomaly: null,
    language: null,
    ...partial,
  };
}

vi.mock("./api", () => ({
  // Per-message re-render used only by the "Load Images" click.
  getMessageBody: vi.fn(async () => ({
    html: null,
    text: null,
    quotedText: null,
    blockedImages: 0,
    canLoadRemote: false,
    attachments: [],
    auth: null,
    senderAnomaly: null,
    language: null,
  })),
  saveAttachment: vi.fn(async () => {}),
  saveAllAttachments: vi.fn(async () => {}),
  // Default: the conversation is just the selected message (the view falls
  // back to it when the thread comes back empty).
  listThread: vi.fn(async () => []),
  // Bodies arrive as one bulk map, keyed by message id.
  threadBodies: vi.fn(async () => ({})),
  setMessageRead: vi.fn(async () => {}),
  summarizeThread: vi.fn(
    async () => "- Alice proposed Saturday\n- Bob agreed, bring snacks",
  ),
  summarizeMessage: vi.fn(async () => "- one line"),
  newRequestId: vi.fn(() => "req-1"),
  onSummaryToken: vi.fn(async () => () => {}),
  cancelSummary: vi.fn(async () => undefined),
  cachedSummary: vi.fn(async () => null),
}));

import * as api from "./api";
import MessageView from "./MessageView.svelte";

const message: MessageHeader = {
  id: 1,
  accountId: 1,
  mailbox: "INBOX",
  from: "Alice <alice@example.com>",
  to: "me@example.com",
  cc: "",
  replyTo: "",
  bcc: "",
  messageId: "",
  references: "",
  threadCount: 1,
  threadUnread: false,
  isDraft: false,
  threadHasDraft: false,
  subject: "Weekend plans",
  snippet: "Hey",
  date: "2026-07-07T09:15:00Z",
  read: false,
  hasAttachments: false,
};

function renderView(props: Record<string, unknown> = {}) {
  return render(MessageView, {
    props: {
      message,
      accountEmails: { 1: "me@example.com" },
      accountColors: {},
      onDraft: vi.fn(),
      ...props,
    },
  });
}

it("renders html bodies in a sandboxed iframe without script rights", async () => {
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({
      html: "<!doctype html><html><body><p>hi there</p></body></html>",
      text: "hi there",
    }),
  });

  const { container } = renderView();

  const iframe = await waitFor(() => {
    const frame = container.querySelector("iframe");
    expect(frame).not.toBeNull();
    return frame as HTMLIFrameElement;
  });
  // SECURITY tripwire (hard rule): the sandbox must contain EXACTLY these
  // two tokens — allow-same-origin so the parent can measure the document's
  // height, and allow-popups so a link click becomes a new-window request
  // that Rust denies and forwards to the default browser. allow-scripts (or
  // any other token) must NEVER appear here: scripts stay blocked by the
  // sandbox, the sanitizer and the srcdoc CSP, and the app frame must stay
  // unnavigable.
  expect(iframe.getAttribute("sandbox")).toBe("allow-same-origin allow-popups");
  expect(iframe.getAttribute("referrerpolicy")).toBe("no-referrer");
  expect(iframe.getAttribute("srcdoc")).toContain("<p>hi there</p>");
});

it("renders text-only bodies as escaped text without an iframe", async () => {
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({ html: null, text: "plain <b>not html</b>" }),
  });

  const { container } = renderView();

  expect(await screen.findByText("plain <b>not html</b>")).toBeInTheDocument();
  expect(container.querySelector("iframe")).toBeNull();
});

it("shows an error when the bulk body fetch fails", async () => {
  vi.mocked(api.threadBodies).mockRejectedValueOnce("imap error: gone");

  renderView();

  expect(await screen.findByText("imap error: gone")).toBeInTheDocument();
});

it("shows the empty state and fetches nothing without a message", () => {
  vi.mocked(api.listThread).mockClear();
  vi.mocked(api.threadBodies).mockClear();

  renderView({ message: null });

  expect(screen.getByText("Select a message")).toBeInTheDocument();
  expect(api.listThread).not.toHaveBeenCalled();
  expect(api.threadBodies).not.toHaveBeenCalled();
});

it("offers to load remote images and re-renders with them", async () => {
  vi.mocked(api.getMessageBody).mockClear();
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({
      html: "<!doctype html><html><body>no pics</body></html>",
      blockedImages: 2,
      canLoadRemote: true,
    }),
  });
  vi.mocked(api.getMessageBody).mockResolvedValueOnce(
    body({ html: "<!doctype html><html><body>with pics</body></html>" }),
  );

  renderView();

  await fireEvent.click(
    await screen.findByRole("button", { name: "Load Images" }),
  );

  expect(api.getMessageBody).toHaveBeenLastCalledWith(1, true);
  await waitFor(() => {
    expect(
      screen.queryByRole("button", { name: "Load Images" }),
    ).not.toBeInTheDocument();
  });
});

it("shows recipients like the prototype: To always, Cc and Bcc when present", async () => {
  const { rerender } = renderView();

  expect(await screen.findByText("To: me@example.com")).toBeInTheDocument();
  expect(screen.queryByText(/Cc:/)).not.toBeInTheDocument();
  expect(screen.queryByText(/Bcc:/)).not.toBeInTheDocument();

  await rerender({
    message: {
      ...message,
      cc: "carol@example.com",
      bcc: "archiv@example.com",
      // Reply-To is a sending concern — the prototype never displays it.
      replyTo: "Support <support@example.com>",
    },
  });

  expect(
    screen.getByText("Cc: carol@example.com · Bcc: archiv@example.com"),
  ).toBeInTheDocument();
  expect(screen.queryByText(/Reply-To:/)).not.toBeInTheDocument();
});

it("opens a collapsed message when its sender name is clicked", async () => {
  vi.mocked(api.listThread).mockResolvedValueOnce(conversationForName());
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({ text: "the original in full" }),
    2: body({ text: "the answer in full" }),
  });

  renderView({ message: { ...message, id: 2 } });
  await screen.findByText("the answer in full");

  // The prototype's name click on a collapsed row opens the card AND
  // reveals the address.
  await fireEvent.click(screen.getByRole("button", { name: "Old Sender" }));

  expect(await screen.findByText("the original in full")).toBeInTheDocument();
  expect(
    screen.getByText("From: old@example.com · To: me@example.com"),
  ).toBeInTheDocument();
});

function conversationForName(): MessageHeader[] {
  return [
    {
      ...message,
      id: 1,
      from: "Old Sender <old@example.com>",
      snippet: "the original",
      read: true,
    hasAttachments: false,
    },
    { ...message, id: 2, snippet: "the answer", read: true },
  ];
}

it("reveals the sender address on a name click without collapsing", async () => {
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({ text: "the full body" }),
  });
  renderView();
  await screen.findByText("the full body");

  await fireEvent.click(screen.getByRole("button", { name: "Alice" }));

  expect(
    screen.getByText("From: alice@example.com · To: me@example.com"),
  ).toBeInTheDocument();
  // The card stayed open — the name click must not toggle the card.
  expect(screen.getByText("the full body")).toBeInTheDocument();
});

const attachments = [
  {
    id: 11,
    messageId: 1,
    partIndex: 0,
    filename: "report.pdf",
    contentType: "application/pdf",
    size: 1200,
  },
  {
    id: 12,
    messageId: 1,
    partIndex: 2,
    filename: "photo.jpg",
    contentType: "image/jpeg",
    size: 45_600,
  },
];

it("lists attachment chips with type tiles and saves one on click", async () => {
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({ text: "see files", attachments }),
  });

  renderView();

  const chip = await screen.findByRole("button", { name: /report\.pdf/ });
  expect(chip).toHaveTextContent("1.2 kB");
  expect(chip).toHaveTextContent("PDF");
  expect(screen.getByText("2 attachments")).toBeInTheDocument();

  await fireEvent.click(chip);
  expect(api.saveAttachment).toHaveBeenCalledWith(attachments[0]);
});

it("offers Save All only for multiple attachments", async () => {
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({ text: "see files", attachments }),
  });
  renderView();

  await fireEvent.click(await screen.findByRole("button", { name: "Save All" }));
  expect(api.saveAllAttachments).toHaveBeenCalledWith(1);
});

it("shows no attachment strip when a message has none", async () => {
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({ text: "plain" }),
  });
  renderView();
  await screen.findByText("plain");

  expect(
    screen.queryByRole("button", { name: "Save All" }),
  ).not.toBeInTheDocument();
});

it("folds quoted text history behind a toggle", async () => {
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({
      text: "Uhradené. Ďakujem.",
      quotedText: "On Monday, Tomáš wrote:\n> pôvodná správa",
    }),
  });
  renderView();
  await screen.findByText("Uhradené. Ďakujem.");

  // The history is hidden until the ••• pill is clicked.
  expect(screen.queryByText(/pôvodná správa/)).not.toBeInTheDocument();
  await fireEvent.click(
    screen.getByRole("button", { name: "Show quoted text" }),
  );
  expect(screen.getByText(/pôvodná správa/)).toBeInTheDocument();
});

// ── Conversation view ──────────────────────────────────────────────────

const conversation: MessageHeader[] = [
  { ...message, id: 1, snippet: "the original", read: true },
  {
    ...message,
    id: 2,
    from: "Me <me@example.com>",
    mailbox: "Sent",
    snippet: "my reply",
    read: true,
    hasAttachments: false,
  },
  {
    ...message,
    id: 3,
    snippet: "their answer",
    read: false,
    hasAttachments: false,
  },
];

const conversationBodies = {
  1: body({ text: "the original in full" }),
  2: body({ text: "my reply in full" }),
  3: body({ text: "their answer in full" }),
};

it("opens the newest message and collapses older ones to preview rows", async () => {
  vi.mocked(api.getMessageBody).mockClear();
  vi.mocked(api.listThread).mockResolvedValueOnce(conversation);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(conversationBodies);

  renderView({ message: { ...message, id: 3 } });

  // The newest body is visible; the older two show snippet rows only.
  expect(await screen.findByText("their answer in full")).toBeInTheDocument();
  expect(screen.getByText("the original")).toBeInTheDocument();
  expect(screen.getByText("my reply")).toBeInTheDocument();
  expect(screen.queryByText("the original in full")).not.toBeInTheDocument();
  // Thread header shows the subject only — no message-count label.
  expect(
    screen.getByRole("heading", { name: "Weekend plans" }),
  ).toBeInTheDocument();
  expect(screen.queryByText("3 messages")).not.toBeInTheDocument();
  // One bulk call fetched everything; no per-card body fetches.
  expect(api.threadBodies).toHaveBeenCalledWith(3);
  expect(api.getMessageBody).not.toHaveBeenCalled();
});

it("labels own messages with me and marks only the opened one read", async () => {
  vi.mocked(api.setMessageRead).mockClear();
  vi.mocked(api.listThread).mockResolvedValueOnce(conversation);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(conversationBodies);

  renderView({ message: { ...message, id: 3 } });
  await screen.findByText("their answer in full");

  expect(screen.getByText("me")).toBeInTheDocument();
  expect(api.setMessageRead).toHaveBeenCalledWith(3, true);
  expect(api.setMessageRead).toHaveBeenCalledTimes(1);
});

it("expands an older message alongside the newest and collapses it again", async () => {
  vi.mocked(api.listThread).mockResolvedValueOnce(conversation);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(conversationBodies);

  renderView({ message: { ...message, id: 3 } });
  await screen.findByText("their answer in full");

  // Click the collapsed row (its preview) — both messages are now open.
  await fireEvent.click(screen.getByText("the original"));
  expect(await screen.findByText("the original in full")).toBeInTheDocument();
  expect(screen.getByText("their answer in full")).toBeInTheDocument();

  // Click the open card's header (meta line) to fold it back to a preview.
  const metas = screen.getAllByText("To: me@example.com");
  await fireEvent.click(metas[0]);
  expect(screen.queryByText("the original in full")).not.toBeInTheDocument();
  expect(screen.getByText("the original")).toBeInTheDocument();
});

it("renders the newest message on top when the setting says so", async () => {
  vi.mocked(api.listThread).mockResolvedValueOnce(conversation);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(conversationBodies);

  renderView({ message: { ...message, id: 3 }, threadOrder: "newestFirst" });

  // The open newest card comes before the collapsed original in the DOM.
  const newest = await screen.findByText("their answer in full");
  const original = screen.getByText("the original");
  expect(
    newest.compareDocumentPosition(original) &
      Node.DOCUMENT_POSITION_FOLLOWING,
  ).toBeTruthy();
});

it("opens a reply draft for the message whose card action was clicked", async () => {
  const onDraft = vi.fn();
  vi.mocked(api.listThread).mockResolvedValueOnce(conversation);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(conversationBodies);

  renderView({ message: { ...message, id: 3 }, onDraft });
  await screen.findByText("their answer in full");

  await fireEvent.click(screen.getByRole("button", { name: "Reply to this message" }));
  expect(onDraft).toHaveBeenCalledWith(
    "reply",
    expect.objectContaining({ id: 3 }),
  );

  await fireEvent.click(screen.getByRole("button", { name: "Reply all to this message" }));
  expect(onDraft).toHaveBeenCalledWith(
    "reply-all",
    expect.objectContaining({ id: 3 }),
  );
});

// ── Focusing a specific message (search hits) ──────────────────────────
//
// A search result is a row for one message, not for its thread — clicking
// it should open that message, not whichever is newest. The clicked row's
// own id is often not even present in the thread (server-side copies of one
// RFC message are deduped by Message-ID on the backend, and the copy that
// survives can be a different folder's row), so the match has to go through
// `messageId`, with the row `id` as a fallback and "newest" as the last
// resort — never `focusSelected` on by default, so ordinary folder browsing
// is untouched.

const searchConversation: MessageHeader[] = [
  { ...message, id: 101, messageId: "m1@x", snippet: "the original", read: false },
  {
    ...message,
    id: 102,
    messageId: "m2@x",
    from: "Me <me@example.com>",
    mailbox: "Sent",
    snippet: "my reply",
    read: true,
  },
  { ...message, id: 103, messageId: "m3@x", snippet: "their answer", read: false },
];

const searchConversationBodies = {
  101: body({ text: "the original in full" }),
  102: body({ text: "my reply in full" }),
  103: body({ text: "their answer in full" }),
};

it("opens the clicked message, not the newest, when focusSelected is set", async () => {
  vi.mocked(api.setMessageRead).mockClear();
  vi.mocked(api.listThread).mockResolvedValueOnce(searchConversation);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(searchConversationBodies);

  // The clicked row's id (999) is deliberately absent from the thread — the
  // dedup situation — so only messageId can find it.
  renderView({
    message: { ...message, id: 999, messageId: "m1@x" },
    focusSelected: true,
  });

  expect(await screen.findByText("the original in full")).toBeInTheDocument();
  expect(screen.queryByText("their answer in full")).not.toBeInTheDocument();
  expect(api.setMessageRead).toHaveBeenCalledWith(101, true);
  expect(api.setMessageRead).not.toHaveBeenCalledWith(103, true);
});

it("still opens the newest message when focusSelected is not set", async () => {
  vi.mocked(api.listThread).mockResolvedValueOnce(searchConversation);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(searchConversationBodies);

  renderView({ message: { ...message, id: 101, messageId: "m1@x" } });

  expect(await screen.findByText("their answer in full")).toBeInTheDocument();
  expect(screen.queryByText("the original in full")).not.toBeInTheDocument();
});

it("does not let a blank messageId match another blank messageId", async () => {
  const blankIdConversation: MessageHeader[] = [
    { ...message, id: 201, messageId: "", snippet: "the original", read: true },
    { ...message, id: 202, messageId: "", snippet: "their answer", read: false },
  ];
  vi.mocked(api.listThread).mockResolvedValueOnce(blankIdConversation);
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    201: body({ text: "the original in full" }),
    202: body({ text: "their answer in full" }),
  });

  renderView({
    message: { ...message, id: 201, messageId: "" },
    focusSelected: true,
  });

  // Falls through to the id match (201), not a spurious messageId match.
  expect(await screen.findByText("the original in full")).toBeInTheDocument();
  expect(screen.queryByText("their answer in full")).not.toBeInTheDocument();
});

it("falls back to the newest message when the clicked one is not in the thread", async () => {
  vi.mocked(api.listThread).mockResolvedValueOnce(searchConversation);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(searchConversationBodies);

  renderView({
    message: { ...message, id: 999, messageId: "gone@x" },
    focusSelected: true,
  });

  expect(await screen.findByText("their answer in full")).toBeInTheDocument();
});

it("scrolls the focused message's card into view", async () => {
  const scrolled: Element[] = [];
  const spy = vi
    .spyOn(Element.prototype, "scrollIntoView")
    .mockImplementation(function (this: Element) {
      scrolled.push(this);
    });

  vi.mocked(api.listThread).mockResolvedValueOnce(searchConversation);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(searchConversationBodies);

  renderView({
    message: { ...message, id: 999, messageId: "m1@x" },
    focusSelected: true,
  });

  await screen.findByText("the original in full");
  // The card is empty on the first pass and only grows once its body lands,
  // so the scroll that counts is the one after bodies resolve.
  await waitFor(() => expect(scrolled.length).toBeGreaterThan(0));
  expect((scrolled.at(-1) as HTMLElement).dataset.messageId).toBe("101");

  spy.mockRestore();
});

it("does not scroll again when the same conversation just refreshes", async () => {
  const scrolled: Element[] = [];
  const spy = vi
    .spyOn(Element.prototype, "scrollIntoView")
    .mockImplementation(function (this: Element) {
      scrolled.push(this);
    });

  vi.mocked(api.listThread).mockResolvedValueOnce(searchConversation);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(searchConversationBodies);

  const { rerender } = renderView({
    message: { ...message, id: 999, messageId: "m1@x" },
    focusSelected: true,
  });
  await screen.findByText("the original in full");
  await waitFor(() => expect(scrolled.length).toBeGreaterThan(0));
  scrolled.length = 0;

  const grown = [
    ...searchConversation,
    { ...message, id: 104, messageId: "m4@x", snippet: "a later reply" },
  ];
  vi.mocked(api.listThread).mockResolvedValueOnce(grown);
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    ...searchConversationBodies,
    104: body({ text: "a later reply in full" }),
  });

  // Same message id — a refresh of the same conversation, not a fresh
  // selection — must not reset the open card or scroll again.
  await rerender({ message: { ...message, id: 999, messageId: "m1@x" } });

  // The new reply lands as a collapsed preview — the open card stays the
  // one the user was reading, not whatever just arrived.
  expect(await screen.findByText("a later reply")).toBeInTheDocument();
  expect(screen.getByText("the original in full")).toBeInTheDocument();
  expect(scrolled).toHaveLength(0);

  spy.mockRestore();
});

// ── Drafts in the conversation ─────────────────────────────────────────

const conversationWithDraft: MessageHeader[] = [
  { ...message, id: 1, snippet: "the original", read: true },
  {
    ...message,
    id: 9,
    from: "Me <me@example.com>",
    mailbox: "Drafts",
    snippet: "half-written answer",
    read: true,
    isDraft: true,
  },
];

const draftBodies = {
  1: body({ text: "the original in full" }),
  9: body({ text: "half-written answer in full" }),
};

it("never anchors a focused selection on a draft", async () => {
  vi.mocked(api.listThread).mockResolvedValueOnce(conversationWithDraft);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(draftBodies);

  // The draft's own id (9) is what a click landed on — a draft card renders
  // expanded regardless (see the badge test below), so the only way to see
  // whether it was chosen as the anchor is whether the OTHER card opens: if
  // the draft had won, id 1 would sit collapsed as a snippet, not its body.
  renderView({
    message: { ...message, id: 9, messageId: "" },
    focusSelected: true,
  });

  expect(await screen.findByText("the original in full")).toBeInTheDocument();
});

it("badges a saved draft and shows its full text without a click", async () => {
  vi.mocked(api.listThread).mockResolvedValueOnce(conversationWithDraft);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(draftBodies);

  renderView({ message: { ...message, id: 1 } });

  expect(await screen.findByText("Draft")).toBeInTheDocument();
  // The draft card starts expanded — unfinished text is what the user came
  // to see — and the newest real message opens alongside it as usual.
  expect(
    await screen.findByText("half-written answer in full"),
  ).toBeInTheDocument();
  expect(screen.getByText("the original in full")).toBeInTheDocument();
  // No reply actions on a draft: replying to your own unfinished reply
  // makes no sense — its only action is resuming the edit.
  expect(
    screen.getAllByRole("button", { name: "Reply to this message" }),
  ).toHaveLength(1);
});

it("opens a clicked draft in the editor", async () => {
  const onEditDraft = vi.fn();
  vi.mocked(api.listThread).mockResolvedValueOnce(conversationWithDraft);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(draftBodies);

  renderView({ message: { ...message, id: 1 }, onEditDraft });
  const badge = await screen.findByText("Draft");

  // Clicking the draft's header row resumes editing in a compose window.
  await fireEvent.click(badge.closest(".head") as HTMLElement);
  expect(onEditDraft).toHaveBeenCalledWith(9);

  // The body area is the same click-to-edit surface — the whole card is.
  await fireEvent.click(screen.getByText("half-written answer in full"));
  expect(onEditDraft).toHaveBeenCalledTimes(2);
});

it("offers Edit and Delete actions on a draft card", async () => {
  const onEditDraft = vi.fn();
  const onDeleteDraft = vi.fn();
  vi.mocked(api.listThread).mockResolvedValueOnce(conversationWithDraft);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(draftBodies);

  renderView({ message: { ...message, id: 1 }, onEditDraft, onDeleteDraft });
  await screen.findByText("Draft");

  await fireEvent.click(screen.getByRole("button", { name: "Edit draft" }));
  expect(onEditDraft).toHaveBeenCalledWith(9);

  await fireEvent.click(screen.getByRole("button", { name: "Delete draft" }));
  expect(onDeleteDraft).toHaveBeenCalledWith(
    expect.objectContaining({ id: 9 }),
  );
  // Delete must not ALSO bubble into the card's click-to-edit surface.
  expect(onEditDraft).toHaveBeenCalledTimes(1);
});

it("warns when a familiar sender writes from an unusual address", async () => {
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({
      text: "please pay this invoice",
      senderAnomaly: { name: "Alice", usualEmail: "alice@example.com" },
    }),
  });

  renderView();

  expect(
    await screen.findByText(/Alice does not usually use this email address/),
  ).toBeInTheDocument();
});

it("warns when authentication checks failed", async () => {
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({
      text: "hello",
      auth: { spf: "pass", dkim: "fail", dmarc: "fail" },
    }),
  });

  renderView();

  // Only the failing checks are named, in SPF/DKIM/DMARC order.
  expect(
    await screen.findByText(/failed DKIM, DMARC authentication/),
  ).toBeInTheDocument();
});

it("stays silent on passing or unknown authentication", async () => {
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({
      text: "hello",
      // softfail and none are common on legitimate mail — no warning.
      auth: { spf: "softfail", dkim: "pass", dmarc: null },
    }),
  });

  renderView();

  await screen.findByText("hello");
  expect(screen.queryByText(/authentication/)).not.toBeInTheDocument();
  expect(screen.queryByText(/does not usually use/)).not.toBeInTheDocument();
});

it("dismisses the trust banner via its close button", async () => {
  vi.mocked(api.threadBodies).mockResolvedValueOnce({
    1: body({
      text: "hello",
      senderAnomaly: { name: "Alice", usualEmail: "alice@example.com" },
    }),
  });

  renderView();
  await screen.findByText(/does not usually use this email address/);

  await fireEvent.click(
    screen.getByRole("button", { name: "Dismiss warning" }),
  );

  expect(
    screen.queryByText(/does not usually use this email address/),
  ).not.toBeInTheDocument();
});

it("summarises a multi-row selection instead of showing a body", () => {
  renderView({ message: null, selectedCount: 5 });

  expect(screen.getByText("5 messages selected")).toBeInTheDocument();
});

it("keeps the prompt when nothing is selected at all", () => {
  renderView({ message: null, selectedCount: 0 });

  expect(screen.getByText("Select a message")).toBeInTheDocument();
});

it("offers no conversation summary for a single message or when not ready", async () => {
  renderView({ canSummarize: true });
  await screen.findByText("hi there").catch(() => undefined);

  expect(screen.queryByLabelText("Summarize this conversation")).toBeNull();
});

it("summarizes the whole conversation from its header", async () => {
  vi.mocked(api.listThread).mockResolvedValueOnce(conversation);
  renderView({ canSummarize: true });

  await fireEvent.click(
    await screen.findByLabelText("Summarize this conversation"),
  );

  expect(api.summarizeThread).toHaveBeenCalledWith(1, false, "req-1");
  expect(await screen.findByText("Alice proposed Saturday")).toBeInTheDocument();
  expect(
    screen.getByRole("region", { name: "Conversation summary" }),
  ).toBeInTheDocument();
});

it("hides the conversation summary button while summaries are not ready", async () => {
  vi.mocked(api.listThread).mockResolvedValueOnce(conversation);
  renderView();
  await screen.findByText("Weekend plans");

  expect(screen.queryByLabelText("Summarize this conversation")).toBeNull();
});

it("opens a conversation with the summary it already has", async () => {
  vi.mocked(api.summarizeThread).mockClear();
  vi.mocked(api.listThread).mockResolvedValueOnce(conversation);
  vi.mocked(api.threadBodies).mockResolvedValueOnce(conversationBodies);
  vi.mocked(api.cachedSummary).mockImplementation(async (_id, thread) =>
    thread ? "- Agreed on Saturday" : null,
  );
  renderView({ message: { ...message, id: 3 }, canSummarize: true });

  expect(await screen.findByText("Agreed on Saturday")).toBeInTheDocument();
  expect(api.cachedSummary).toHaveBeenCalledWith(3, true);
  expect(api.summarizeThread).not.toHaveBeenCalled();
  vi.mocked(api.cachedSummary).mockReset();
  vi.mocked(api.cachedSummary).mockResolvedValue(null);
});
