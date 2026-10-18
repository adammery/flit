import { afterEach, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/svelte";
import MessageCard from "./MessageCard.svelte";
import type { MessageBody, MessageHeader } from "./types";

vi.mock("./api", () => ({
  getMessageBody: vi.fn(async () => ({})),
  saveAttachment: vi.fn(async () => true),
  saveAllAttachments: vi.fn(async () => true),
  openAttachment: vi.fn(async () => {}),
  revealAttachment: vi.fn(async () => {}),
  summarizeMessage: vi.fn(
    async () => "- Alice asks about the weekend\n- Reply by Friday",
  ),
  newRequestId: vi.fn(() => "req-1"),
  onSummaryToken: vi.fn(async () => () => {}),
  cancelSummary: vi.fn(async () => undefined),
  cachedSummary: vi.fn(async () => null),
  translateMessage: vi.fn(async () => ({
    html: "<!doctype html><html><body><p>ahoj</p></body></html>",
    text: "ahoj",
    subject: "Víkendové plány",
  })),
  onTranslationProgress: vi.fn(async () => () => {}),
  onTranslationPartial: vi.fn(async () => () => {}),
}));

import * as api from "./api";

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

const body: MessageBody = {
  html: "<!doctype html><html><body><p>hi there</p></body></html>",
  text: "hi there",
  quotedText: null,
  blockedImages: 0,
  canLoadRemote: false,
  attachments: [],
  auth: null,
  senderAnomaly: null,
  language: null,
};

function renderCard(extra: Record<string, unknown> = {}) {
  return render(MessageCard, {
    props: {
      message,
      body,
      loading: false,
      error: null,
      expanded: true,
      last: true,
      ownEmail: "me@example.com",
      accountColor: null,
      onToggle: vi.fn(),
      onDraft: vi.fn(),
      ...extra,
    },
  });
}

/** Net number of `load` listeners a spied target currently holds. */
function loadListeners(add: ReturnType<typeof vi.spyOn>, remove: ReturnType<typeof vi.spyOn>) {
  const count = (spy: ReturnType<typeof vi.spyOn>) =>
    spy.mock.calls.filter(([type]) => type === "load").length;
  return count(add) - count(remove);
}

afterEach(() => {
  vi.restoreAllMocks();
});

it("hooks each body image once, however often the frame reloads", async () => {
  const { container, unmount } = renderCard();
  const frame = container.querySelector("iframe") as HTMLIFrameElement;
  // why: jsdom never renders srcdoc, so the frame keeps its about:blank
  // document — plant the images the sanitized mail would carry.
  const doc = frame.contentDocument as Document;
  const images = [doc.createElement("img"), doc.createElement("img")];
  const spies = images.map((image) => ({
    add: vi.spyOn(image, "addEventListener"),
    remove: vi.spyOn(image, "removeEventListener"),
  }));
  for (const image of images) doc.body.appendChild(image);

  // srcdoc load, then a "Load Images" swap: the hook runs on every load.
  await fireEvent.load(frame);
  await fireEvent.load(frame);
  for (const { add, remove } of spies) {
    expect(loadListeners(add, remove)).toBe(1);
  }

  unmount();
  for (const { add, remove } of spies) {
    expect(loadListeners(add, remove)).toBe(0);
  }
});

it("removes the frame's own load listener when the card is destroyed", () => {
  const add = vi.spyOn(HTMLIFrameElement.prototype, "addEventListener");
  const remove = vi.spyOn(HTMLIFrameElement.prototype, "removeEventListener");
  const { unmount } = renderCard();
  // Both frame actions (autoSize, interceptBodyLinks) hook the load event.
  expect(loadListeners(add, remove)).toBeGreaterThan(0);

  unmount();
  expect(loadListeners(add, remove)).toBe(0);
});

it("offers no summarize button unless summaries are ready", () => {
  const { queryByLabelText } = renderCard();

  expect(queryByLabelText("Summarize this message")).toBeNull();
});

it("summarizes the message into bullet lines", async () => {
  const { getByLabelText, findByText, getByRole } = renderCard({
    canSummarize: true,
  });

  await fireEvent.click(getByLabelText("Summarize this message"));

  expect(api.summarizeMessage).toHaveBeenCalledWith(1, false, "req-1");
  expect(await findByText("Alice asks about the weekend")).toBeInTheDocument();
  expect(await findByText("Reply by Friday")).toBeInTheDocument();
  expect(getByRole("region", { name: "AI summary" })).toBeInTheDocument();
});

it("shows why a summary failed", async () => {
  vi.mocked(api.summarizeMessage).mockRejectedValueOnce(
    new Error("Summaries are switched off"),
  );
  const { getByLabelText, findByRole } = renderCard({ canSummarize: true });

  await fireEvent.click(getByLabelText("Summarize this message"));

  expect(await findByRole("alert")).toHaveTextContent(
    "Summaries are switched off",
  );
});

it("folds the summary and unfolds it again", async () => {
  const { getByLabelText, findByText, queryByText, getByRole } = renderCard({
    canSummarize: true,
  });
  await fireEvent.click(getByLabelText("Summarize this message"));
  await findByText("Reply by Friday");

  await fireEvent.click(getByLabelText("Hide summary"));

  expect(queryByText("Reply by Friday")).toBeNull();
  expect(getByRole("region", { name: "AI summary" })).toBeInTheDocument();

  await fireEvent.click(getByLabelText("Show summary"));

  expect(await findByText("Reply by Friday")).toBeInTheDocument();
});

it("shows the summary section with its button before any summary exists", () => {
  const { getByRole, getByLabelText } = renderCard({ canSummarize: true });

  expect(getByRole("region", { name: "AI summary" })).toBeInTheDocument();
  expect(getByLabelText("Summarize this message")).toBeInTheDocument();
});

it("shows an already-written summary when the card opens", async () => {
  vi.mocked(api.cachedSummary).mockResolvedValueOnce("- Written earlier");
  const { findByText, queryByLabelText } = renderCard({ canSummarize: true });

  expect(await findByText("Written earlier")).toBeInTheDocument();
  expect(api.cachedSummary).toHaveBeenCalledWith(1);
  expect(api.summarizeMessage).not.toHaveBeenCalled();
  expect(queryByLabelText("Summarize this message")).toBeNull();
});

it("asks for no cached summary while summaries are not ready", () => {
  renderCard();

  expect(api.cachedSummary).not.toHaveBeenCalled();
});

it("asks for a fresh summary from the panel's retry", async () => {
  const { getByLabelText, findByText } = renderCard({ canSummarize: true });
  await fireEvent.click(getByLabelText("Summarize this message"));
  await findByText("Reply by Friday");

  await fireEvent.click(getByLabelText("Summarize again"));

  expect(api.summarizeMessage).toHaveBeenLastCalledWith(1, true, "req-1");
});

it("cancels a summary still being written and drops the panel", async () => {
  vi.mocked(api.summarizeMessage).mockReturnValueOnce(new Promise(() => {}));
  const { getByLabelText, findByLabelText, queryByRole } = renderCard({
    canSummarize: true,
  });
  await fireEvent.click(getByLabelText("Summarize this message"));

  await fireEvent.click(await findByLabelText("Cancel summary"));

  expect(api.cancelSummary).toHaveBeenCalledWith("req-1");
  // Back to the empty section with its button.
  expect(await findByLabelText("Summarize this message")).toBeInTheDocument();
  expect(queryByRole("alert")).toBeNull();
});

it("shows summary lines as plain text without stray markdown bold", async () => {
  vi.mocked(api.summarizeMessage).mockResolvedValueOnce("- **Goal:** simplify");
  const { getByLabelText, findByText } = renderCard({ canSummarize: true });

  await fireEvent.click(getByLabelText("Summarize this message"));

  expect(await findByText("Goal: simplify")).toBeInTheDocument();
});

it("offers no translate button unless translation is ready", () => {
  const { queryByLabelText } = renderCard({ canSummarize: true });

  expect(queryByLabelText("Translate this message")).toBeNull();
});

it("offers translation without summaries", async () => {
  const { getByLabelText, queryByLabelText } = renderCard({
    translateInto: "Slovak",
  });

  expect(queryByLabelText("Summarize this message")).toBeNull();
  await fireEvent.click(getByLabelText("Translate this message"));

  expect(api.translateMessage).toHaveBeenCalledWith(1, false, "req-1");
});

it("translates the body in place and flips back to the original", async () => {
  const { container, getByLabelText, findByText, getByText, queryByText } = renderCard({
    canSummarize: true,
    translateInto: "Slovak",
  });
  const srcdoc = () => container.querySelector("iframe")?.getAttribute("srcdoc");

  await fireEvent.click(getByLabelText("Translate this message"));

  expect(api.translateMessage).toHaveBeenCalledWith(1, false, "req-1");
  expect(await findByText("Translated")).toBeInTheDocument();
  expect(srcdoc()).toContain("ahoj");

  expect(getByText("Víkendové plány")).toBeInTheDocument();

  await fireEvent.click(getByText("Show original"));
  expect(srcdoc()).toContain("hi there");
  expect(queryByText("Víkendové plány")).toBeNull();

  await fireEvent.click(getByText("Show translation"));
  expect(srcdoc()).toContain("ahoj");

  await fireEvent.click(getByLabelText("Close translation"));
  expect(srcdoc()).toContain("hi there");
  expect(getByLabelText("Translate this message")).toBeInTheDocument();
});

it("shows why a translation failed and offers it again", async () => {
  vi.mocked(api.translateMessage).mockRejectedValueOnce(
    new Error("This message has no text to translate."),
  );
  const { getByLabelText, findByRole } = renderCard({ translateInto: "Slovak" });

  await fireEvent.click(getByLabelText("Translate this message"));

  expect((await findByRole("alert")).textContent).toContain("no text to translate");
  expect(getByLabelText("Translate this message")).toBeInTheDocument();
});

it("keeps the original's attachments under a translated body", async () => {
  const { getByLabelText, findByText, getByText } = renderCard({
    translateInto: "Slovak",
    body: {
      ...body,
      attachments: [
        {
          id: 5,
          messageId: 1,
          partIndex: 2,
          filename: "rechnung.pdf",
          contentType: "application/pdf",
          size: 2048,
        },
      ],
    },
  });

  await fireEvent.click(getByLabelText("Translate this message"));

  expect(await findByText("Translated")).toBeInTheDocument();
  expect(getByText("rechnung.pdf")).toBeInTheDocument();
});

it("offers no translation for a message already in the reader's language", () => {
  const { queryByLabelText } = renderCard({
    translateInto: "Slovak",
    body: { ...body, language: "Slovak" },
  });

  expect(queryByLabelText("Translate this message")).toBeNull();
});

it("offers translation for a message in another language", () => {
  const { getByLabelText } = renderCard({
    translateInto: "Slovak",
    body: { ...body, language: "German" },
  });

  expect(getByLabelText("Translate this message")).toBeInTheDocument();
});
