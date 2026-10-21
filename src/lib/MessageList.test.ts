import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/svelte";
import MessageList from "./MessageList.svelte";
import type { MessageHeader } from "./types";

function header(id: number, from: string, read: boolean): MessageHeader {
  return {
    id,
    accountId: 1,
    mailbox: "INBOX",
    from,
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
    subject: `Subject ${id}`,
    snippet: "…",
    date: "2026-07-07T09:15:00Z",
    read,
    hasAttachments: false,
  };
}

const messages = [
  header(1, "Alice <alice@example.com>", false),
  header(2, "Bob <bob@example.com>", true),
];

function renderList(props: Record<string, unknown> = {}) {
  return render(MessageList, {
    props: {
      title: "Inbox",
      messages,
      selectedId: null,
      onSelect: vi.fn(),
      ...props,
    },
  });
}

/** One trackpad pulse; natural scrolling makes fingers-left = +deltaX. */
function swipe(row: HTMLElement, deltaX: number, deltaY = 0) {
  return fireEvent.wheel(row, { deltaX, deltaY });
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

it("archives a row once a full swipe left settles", async () => {
  const onArchive = vi.fn();
  renderList({ onArchive });
  const row = screen.getByRole("option", { name: /Alice/ });

  await swipe(row, 60);
  await swipe(row, 60);
  expect(onArchive).not.toHaveBeenCalled(); // fingers still down

  vi.advanceTimersByTime(200);
  expect(onArchive).toHaveBeenCalledWith(1);
});

it("fires nothing for a swipe still in flight when the list is torn down", async () => {
  const onArchive = vi.fn();
  const { unmount } = renderList({ onArchive });
  const row = screen.getByRole("option", { name: /Alice/ });

  await swipe(row, 60);
  await swipe(row, 60);
  // A folder switch replaces the list mid-gesture.
  unmount();

  vi.advanceTimersByTime(200);
  expect(onArchive).not.toHaveBeenCalled();
});

it("toggles read state once a full swipe right settles", async () => {
  const onSetRead = vi.fn();
  renderList({ onSetRead });

  // The unread row offers Mark Read…
  await swipe(screen.getByRole("option", { name: /Alice/ }), -120);
  vi.advanceTimersByTime(200);
  expect(onSetRead).toHaveBeenCalledWith(1, true);

  // …and the read row offers Mark Unread.
  await swipe(screen.getByRole("option", { name: /Bob/ }), -120);
  vi.advanceTimersByTime(200);
  expect(onSetRead).toHaveBeenCalledWith(2, false);
});

it("fires nothing when the swipe stops short of the trigger", async () => {
  const onArchive = vi.fn();
  const onSetRead = vi.fn();
  renderList({ onArchive, onSetRead });

  await swipe(screen.getByRole("option", { name: /Alice/ }), 30);
  vi.advanceTimersByTime(200);

  expect(onArchive).not.toHaveBeenCalled();
  expect(onSetRead).not.toHaveBeenCalled();
});

it("routes swipes through the configured actions", async () => {
  const onTrash = vi.fn();
  const onReply = vi.fn();
  renderList({
    swipeActions: { left: "trash", right: "reply" },
    onTrash,
    onReply,
  });
  const row = screen.getByRole("option", { name: /Alice/ });

  // Mid-gesture the strip announces the configured action, not Archive.
  await swipe(row, 60);
  expect(screen.getByText("Trash")).toBeInTheDocument();
  await swipe(row, 60);
  vi.advanceTimersByTime(200);
  expect(onTrash).toHaveBeenCalledWith(1);

  await swipe(row, -120);
  expect(screen.getByText("Reply")).toBeInTheDocument();
  vi.advanceTimersByTime(200);
  expect(onReply).toHaveBeenCalledWith(1);
});

it("neither moves nor fires on a direction configured to none", async () => {
  const onArchive = vi.fn();
  renderList({
    swipeActions: { left: "none", right: "toggleRead" },
    onArchive,
  });
  const row = screen.getByRole("option", { name: /Alice/ });

  await swipe(row, 120);
  // The row stays pinned — no strip is revealed for the dead side.
  expect(row.style.transform).toBe("");
  vi.advanceTimersByTime(200);

  expect(onArchive).not.toHaveBeenCalled();
});

it("leaves vertical scrolling alone", async () => {
  const onArchive = vi.fn();
  renderList({ onArchive });

  // deltaY dominates — this is a scroll, not a swipe, however far it goes.
  await swipe(screen.getByRole("option", { name: /Alice/ }), 100, 300);
  vi.advanceTimersByTime(200);

  expect(onArchive).not.toHaveBeenCalled();
});

// why MouseEvent: jsdom has no PointerEvent, and fireEvent.pointerDown then
// synthesizes a bare Event without clientX/button. A MouseEvent with the
// pointer event type still reaches the handlers with real coordinates;
// bubbles is required — Svelte 5 delegates the handlers to the app root.
function pointer(type: string, init: MouseEventInit = {}) {
  return new MouseEvent(type, { bubbles: true, ...init });
}

/** Press, drag horizontally by `dx`, release — a mouse/touch swipe. */
async function drag(row: HTMLElement, dx: number) {
  await fireEvent(row, pointer("pointerdown", { clientX: 200, clientY: 50 }));
  // Two moves: past the slop first, then the rest — like a real pointer.
  await fireEvent(
    row,
    pointer("pointermove", { clientX: 200 + dx / 2, clientY: 50 }),
  );
  await fireEvent(
    row,
    pointer("pointermove", { clientX: 200 + dx, clientY: 50 }),
  );
  await fireEvent(row, pointer("pointerup", { clientX: 200 + dx, clientY: 50 }));
}

it("fires the configured action when a pointer drag releases past the trigger", async () => {
  const onArchive = vi.fn();
  const onSetRead = vi.fn();
  const onSelect = vi.fn();
  renderList({ onArchive, onSetRead, onSelect });
  const row = screen.getByRole("option", { name: /Alice/ });

  await drag(row, -80);
  expect(onArchive).toHaveBeenCalledWith(1);

  await drag(row, 80);
  expect(onSetRead).toHaveBeenCalledWith(1, true);

  // The click that follows a drag's release must not select the row.
  await fireEvent.click(row);
  expect(onSelect).not.toHaveBeenCalled();
});

it("snaps back without selecting when a drag stops short of the trigger", async () => {
  const onArchive = vi.fn();
  const onSelect = vi.fn();
  renderList({ onArchive, onSelect });
  const row = screen.getByRole("option", { name: /Alice/ });

  await drag(row, -30);
  await fireEvent.click(row);

  expect(onArchive).not.toHaveBeenCalled();
  expect(onSelect).not.toHaveBeenCalled();
  expect(row.style.transform).toBe("");
});

it("still selects on a plain click", async () => {
  const onSelect = vi.fn();
  renderList({ onSelect });
  const row = screen.getByRole("option", { name: /Alice/ });

  await fireEvent(row, pointer("pointerdown", { clientX: 200, clientY: 50 }));
  await fireEvent(row, pointer("pointerup", { clientX: 200, clientY: 50 }));
  await fireEvent.click(row);

  expect(onSelect).toHaveBeenCalledWith(1, "replace");
});

it("does not start a drag from a mostly vertical pointer move", async () => {
  const onArchive = vi.fn();
  const onSelect = vi.fn();
  renderList({ onArchive, onSelect });
  const row = screen.getByRole("option", { name: /Alice/ });

  await fireEvent(row, pointer("pointerdown", { clientX: 200, clientY: 50 }));
  await fireEvent(row, pointer("pointermove", { clientX: 120, clientY: 250 }));
  await fireEvent(row, pointer("pointerup", { clientX: 120, clientY: 250 }));
  await fireEvent.click(row);

  expect(onArchive).not.toHaveBeenCalled();
  // No drag happened, so the click still counts as a selection.
  expect(onSelect).toHaveBeenCalledWith(1, "replace");
});

it("reports a cmd-click as a toggle and a shift-click as a range", async () => {
  const onSelect = vi.fn();
  renderList({ onSelect });
  const row = screen.getByRole("option", { name: /Bob/ });

  await fireEvent.click(row, { metaKey: true });
  expect(onSelect).toHaveBeenCalledWith(2, "toggle");

  await fireEvent.click(row, { shiftKey: true });
  expect(onSelect).toHaveBeenCalledWith(2, "range");
});

it("reports a right-click on a row instead of selecting it", async () => {
  const onContextMenu = vi.fn();
  const onSelect = vi.fn();
  renderList({ onContextMenu, onSelect });
  const row = screen.getByRole("option", { name: /Bob/ });

  // fireEvent returns false once a handler called preventDefault — the
  // webview's own Reload / Inspect menu must not open over ours.
  const shown = await fireEvent.contextMenu(row);

  expect(shown).toBe(false);
  expect(onContextMenu).toHaveBeenCalledWith(2);
  expect(onSelect).not.toHaveBeenCalled();
});

it("leaves the webview's menu alone when nobody listens for right-clicks", async () => {
  renderList();
  const row = screen.getByRole("option", { name: /Bob/ });

  expect(await fireEvent.contextMenu(row)).toBe(true);
});

it("marks every selected row, not just the lead one", () => {
  renderList({ selectedId: 1, selectedIds: [1, 2] });

  expect(screen.getByRole("option", { name: /Alice/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  expect(screen.getByRole("option", { name: /Bob/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

it("leaves a row unmarked once it is toggled out of the selection", () => {
  renderList({ selectedId: 1, selectedIds: [2] });

  expect(screen.getByRole("option", { name: /Alice/ })).toHaveAttribute(
    "aria-selected",
    "false",
  );
});

it("announces the list as multi-selectable", () => {
  renderList();

  expect(screen.getByRole("listbox")).toHaveAttribute(
    "aria-multiselectable",
    "true",
  );
});

it("keeps every option an accessible child of the listbox", () => {
  renderList();
  const listbox = screen.getByRole("listbox");
  // ARIA only lets wrappers between a listbox and its options if they drop
  // out of the tree, so VoiceOver can count and step through the rows.
  const transparent = (el: Element) =>
    el.getAttribute("aria-hidden") === "true" ||
    ["presentation", "none", "group"].includes(el.getAttribute("role") ?? "");

  for (const child of Array.from(listbox.children)) {
    if (child.getAttribute("role") === "option") continue;
    expect(transparent(child), child.outerHTML.slice(0, 80)).toBe(true);
  }
  for (const option of screen.getAllByRole("option")) {
    let el = option.parentElement;
    while (el && el !== listbox) {
      expect(transparent(el), el.outerHTML.slice(0, 80)).toBe(true);
      el = el.parentElement;
    }
  }
});

it("labels the swipe backdrop Move to Inbox for archived rows", async () => {
  renderList({ onArchive: vi.fn(), isArchived: () => true });

  // Mid-swipe (short of the trigger) the backdrop is visible.
  await swipe(screen.getByRole("option", { name: /Alice/ }), 30);
  expect(screen.getByText("Move to Inbox")).toBeInTheDocument();

  vi.advanceTimersByTime(200); // snap back, nothing fired
});

it("shows a paperclip on rows whose conversation has attachments", () => {
  renderList({
    messages: [
      { ...header(1, "Alice <alice@example.com>", true), hasAttachments: true },
      header(2, "Bob <bob@example.com>", true),
    ],
  });

  const withFile = screen.getByRole("option", { name: /Alice/ });
  expect(withFile.querySelector(".clip")).not.toBeNull();
  const without = screen.getByRole("option", { name: /Bob/ });
  expect(without.querySelector(".clip")).toBeNull();
});

it("badges a conversation row with its message count", () => {
  renderList({
    messages: [
      { ...header(1, "Alice <alice@example.com>", true), threadCount: 3 },
      header(2, "Bob <bob@example.com>", true),
    ],
  });

  const thread = screen.getByRole("option", { name: /Alice/ });
  expect(thread.textContent).toContain("3");
  // A single message shows no count badge.
  const single = screen.getByRole("option", { name: /Bob/ });
  expect(single.querySelector(".thread-count")).toBeNull();
});

it("pins a Draft pill on rows whose conversation holds a draft", () => {
  renderList({
    messages: [
      { ...header(1, "Alice <alice@example.com>", true), threadHasDraft: true },
      header(2, "Bob <bob@example.com>", true),
    ],
  });

  const withDraft = screen.getByRole("option", { name: /Alice/ });
  expect(withDraft).toHaveTextContent("Draft");
  const without = screen.getByRole("option", { name: /Bob/ });
  expect(without).not.toHaveTextContent("Draft");
});

it("dots a read representative whose conversation is unread elsewhere", () => {
  renderList({
    messages: [
      {
        ...header(1, "Alice <alice@example.com>", true),
        threadCount: 2,
        threadUnread: true,
      },
    ],
  });

  const row = screen.getByRole("option", { name: /Alice/ });
  expect(row.querySelector(".dot")).not.toBeNull();
});

it("says which words a search used in place of typos", () => {
  renderList({
    corrections: [
      { typed: "bnny", corrected: "bunny" },
      { typed: "invoce", corrected: "invoice" },
    ],
  });

  expect(screen.getByText(/Showing results for/)).toHaveTextContent(
    "Showing results for bunny instead of bnny, invoice instead of invoce",
  );
});

it("shows no correction line for a search taken as typed", () => {
  renderList();

  expect(screen.queryByText(/Showing results for/)).not.toBeInTheDocument();
});

// Pull-to-refresh: upward wheel pulses on the list itself while at the top.

it("starts a refresh once a deep pull at the top settles", async () => {
  const onRefresh = vi.fn();
  renderList({ onRefresh });
  const list = screen.getByRole("listbox");

  await swipe(list, 0, -80);
  await swipe(list, 0, -80);
  expect(onRefresh).not.toHaveBeenCalled(); // fingers still down

  vi.advanceTimersByTime(200);
  expect(onRefresh).toHaveBeenCalledOnce();
});

it("fires nothing for a pull still in flight when the list is torn down", async () => {
  const onRefresh = vi.fn();
  const { unmount } = renderList({ onRefresh });
  const list = screen.getByRole("listbox");

  await swipe(list, 0, -80);
  await swipe(list, 0, -80);
  unmount();

  vi.advanceTimersByTime(200);
  expect(onRefresh).not.toHaveBeenCalled();
});

it("arms the hint only past the trigger line", async () => {
  renderList({ onRefresh: vi.fn() });
  const list = screen.getByRole("listbox");

  expect(screen.queryByText(/refresh/)).toBeNull();
  await swipe(list, 0, -80); // 40px of pull — short of the trigger
  expect(screen.getByText("Pull to refresh")).toBeInTheDocument();
  await swipe(list, 0, -80);
  expect(screen.getByText("Release to refresh")).toBeInTheDocument();
  vi.advanceTimersByTime(200); // release, don't leak the timer
});

it("fires nothing when the pull stops short of the trigger", async () => {
  const onRefresh = vi.fn();
  renderList({ onRefresh });

  await swipe(screen.getByRole("listbox"), 0, -80);
  vi.advanceTimersByTime(200);

  expect(onRefresh).not.toHaveBeenCalled();
});

it("ignores upward wheels while the list is scrolled down", async () => {
  const onRefresh = vi.fn();
  renderList({ onRefresh });
  const list = screen.getByRole("listbox");
  list.scrollTop = 100;

  await swipe(list, 0, -300);
  vi.advanceTimersByTime(200);

  expect(onRefresh).not.toHaveBeenCalled();
});

it("shows a checking indicator and swallows pulls while refreshing", async () => {
  const onRefresh = vi.fn();
  renderList({ onRefresh, refreshing: true });

  expect(screen.getByText(/Checking for new mail/)).toBeInTheDocument();

  await swipe(screen.getByRole("listbox"), 0, -300);
  vi.advanceTimersByTime(200);
  expect(onRefresh).not.toHaveBeenCalled();
});

it("leaves horizontal row swipes out of the pull", async () => {
  const onRefresh = vi.fn();
  renderList({ onRefresh, onArchive: vi.fn() });

  // A row swipe bubbles to the list — dominant deltaX must not pull.
  await swipe(screen.getByRole("option", { name: /Alice/ }), 120, -20);
  vi.advanceTimersByTime(200);

  expect(onRefresh).not.toHaveBeenCalled();
});

/** A view status: 500 of 10000 messages cached, 800 rows listable. */
function status(overrides: Record<string, unknown> = {}) {
  return {
    listRows: 800,
    unread: 56,
    cached: 500,
    serverTotal: 10000,
    ...overrides,
  };
}

/** Pin the scroll geometry jsdom doesn't compute, then fire a scroll. */
function scrollTo(list: HTMLElement, top: number, height = 2000, view = 400) {
  Object.defineProperty(list, "scrollHeight", {
    value: height,
    configurable: true,
  });
  Object.defineProperty(list, "clientHeight", {
    value: view,
    configurable: true,
  });
  list.scrollTop = top;
  return fireEvent.scroll(list);
}

it("asks for more rows when scrolled near the bottom", async () => {
  const onLoadMore = vi.fn();
  renderList({ status: status(), onLoadMore });

  await scrollTo(screen.getByRole("listbox"), 1500);

  expect(onLoadMore).toHaveBeenCalledOnce();
});

it("asks for nothing far from the bottom", async () => {
  const onLoadMore = vi.fn();
  renderList({ status: status(), onLoadMore });

  await scrollTo(screen.getByRole("listbox"), 0);

  expect(onLoadMore).not.toHaveBeenCalled();
});

it("asks for nothing once every row is listed", async () => {
  const onLoadMore = vi.fn();
  renderList({ status: status({ listRows: messages.length }), onLoadMore });

  await scrollTo(screen.getByRole("listbox"), 1500);

  expect(onLoadMore).not.toHaveBeenCalled();
});

it("headers the view's totals, not the revealed slice", () => {
  renderList({ status: status() });

  const count = document.querySelector(".count");
  expect(count?.textContent).toMatch(/800\s+messages/);
  expect(count?.textContent).toContain("56 unread");
});

it("shows backfill progress while the cache trails the server", () => {
  renderList({ status: status() });

  expect(screen.getByText(/Syncing older messages/)).toBeTruthy();
});

it("hides the progress line once the mailbox is fully mirrored", () => {
  renderList({ status: status({ cached: 10000 }) });

  expect(screen.queryByText(/Syncing older messages/)).toBeNull();
});

it("hides the progress line while the server total is unknown", () => {
  renderList({ status: status({ serverTotal: null }) });

  expect(screen.queryByText(/Syncing older messages/)).toBeNull();
});

it("renders one date section header where the section changes", () => {
  const today = new Date().toISOString();
  renderList({
    messages: [
      { ...header(1, "Alice <alice@example.com>", false), date: today },
      { ...header(2, "Bob <bob@example.com>", true), date: today },
      { ...header(3, "Carol <carol@example.com>", true), date: "2025-03-10T10:00:00Z" },
    ],
  });

  const sections = [...document.querySelectorAll(".section")].map((el) =>
    el.textContent?.trim(),
  );
  expect(sections).toEqual(["Today", "2025"]);
});

it("renders no section headers for an empty list", () => {
  renderList({ messages: [] });

  expect(document.querySelector(".section")).toBeNull();
});

it("shows a fetched icon in place of the monogram", () => {
  renderList({
    messages: [
      header(1, "Alice <alice@example.com>", false),
      header(2, "Carol <carol@other.test>", true),
    ],
    avatars: { "example.com": "data:image/png;base64,iVBORw==" },
  });

  const avatars = [...document.querySelectorAll<HTMLElement>(".avatar")];
  const icon = avatars[0].querySelector("img");
  expect(icon).not.toBeNull();
  expect(icon?.getAttribute("src")).toBe("data:image/png;base64,iVBORw==");
  // The tint would fight the logo, so a row with an icon drops it.
  expect(avatars[0].style.background).toBe("");

  // A domain with no icon is untouched — it keeps its tinted monogram.
  expect(avatars[1].querySelector("img")).toBeNull();
  expect(avatars[1].textContent?.trim()).toBe("C");
  expect(avatars[1].style.background).not.toBe("");
});

it("gives every row a monogram tinted by the sender's domain", () => {
  renderList({
    messages: [
      header(1, "Alice <alice@example.com>", false),
      header(2, "Bob <bob@example.com>", true),
      header(3, "Carol <carol@other.test>", true),
    ],
  });

  const avatars = [...document.querySelectorAll<HTMLElement>(".avatar")];
  expect(avatars.map((el) => el.textContent?.trim())).toEqual(["A", "B", "C"]);
  // Alice and Bob share a domain, so they share a tint — that shared color is
  // the whole point of keying on the domain rather than the address.
  expect(avatars[0].style.background).toBe(avatars[1].style.background);
  expect(avatars[2].style.background).not.toBe(avatars[0].style.background);
});

it("shows the sender and subject only — no body preview", () => {
  // why: the list is for scanning, and a two-line preview of the body is the
  // noisiest thing on the row. Dropping it also fixes the row height, which
  // is what lets the list be virtualized by arithmetic alone.
  renderList({
    messages: [
      {
        ...header(1, "Alice <alice@example.com>", false),
        snippet: "Are we still on for Saturday?",
      },
    ],
  });

  expect(screen.getByText("Alice")).toBeTruthy();
  expect(screen.getByText("Subject 1")).toBeTruthy();
  expect(screen.queryByText("Are we still on for Saturday?")).toBeNull();
});

/** A folder's worth of rows — what a big mailbox actually hands the list. */
function manyRows(count: number) {
  return Array.from({ length: count }, (_, i) =>
    header(i + 1, `Sender ${i} <s${i}@example.com>`, true),
  );
}

it("puts only a window of a long list in the DOM", () => {
  // why this matters: drawing all 500 rows of a page measured 85-878 ms on a
  // real mailbox — in one folder more than the query that produced them.
  renderList({ messages: manyRows(500) });

  const drawn = document.querySelectorAll('[role="option"]');

  expect(drawn.length).toBeGreaterThan(0);
  expect(drawn.length).toBeLessThan(60);
});

it("keeps the scroll height of the whole list, not just the window", () => {
  // The spacers stand in for the undrawn rows, so the scrollbar reflects how
  // much there is to scroll rather than how much is currently rendered.
  renderList({ messages: manyRows(500) });

  const spacers = [...document.querySelectorAll<HTMLElement>(".spacer")];
  const padding = spacers.reduce(
    (total, el) => total + parseFloat(el.style.height),
    0,
  );

  expect(spacers).toHaveLength(2);
  expect(padding).toBeGreaterThan(20_000);
});

it("draws later rows once the list is scrolled", async () => {
  renderList({ messages: manyRows(500) });
  const list = document.querySelector<HTMLElement>(".list")!;
  const before = document.querySelectorAll('[role="option"]')[0].textContent;

  list.scrollTop = 12_000;
  await fireEvent.scroll(list);

  const after = document.querySelectorAll('[role="option"]')[0].textContent;
  expect(after).not.toBe(before);
});
