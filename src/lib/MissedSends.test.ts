import { expect, it, vi } from "vitest";
import { tick } from "svelte";
import { fireEvent, render, screen } from "@testing-library/svelte";
import MissedSends from "./MissedSends.svelte";
import type { ScheduledMessage } from "./types";

function entry(over: Partial<ScheduledMessage> = {}): ScheduledMessage {
  return {
    id: 1,
    accountId: 1,
    aliasId: null,
    to: "alice@example.com",
    cc: "",
    bcc: "",
    subject: "Quarterly report",
    body: "hello",
    bodyHtml: null,
    attachments: [],
    // 2026-07-15 18:00 local time
    scheduledAt: Math.floor(new Date(2026, 6, 15, 18, 0).getTime() / 1000),
    status: "missed",
    inReplyTo: null,
    references: null,
    ...over,
  };
}

const noop = () => {};

it("renders nothing when there are no missed messages", () => {
  render(MissedSends, {
    entries: [],
    onSendNow: noop,
    onOpenDraft: noop,
    onDismiss: noop,
  });

  expect(screen.queryByRole("alertdialog")).toBeNull();
});

it("lists each missed message with recipient and planned time", () => {
  render(MissedSends, {
    entries: [
      entry(),
      entry({ id: 2, subject: "", to: "bob@example.com" }),
    ],
    onSendNow: noop,
    onOpenDraft: noop,
    onDismiss: noop,
  });

  expect(screen.getByRole("alertdialog")).toBeInTheDocument();
  expect(screen.getByText("Quarterly report")).toBeInTheDocument();
  expect(screen.getByText("(No subject)")).toBeInTheDocument();
  expect(screen.getByText(/alice@example\.com/)).toBeInTheDocument();
  // The missed moment is shown so the user can judge how stale the send is
  // (formatted in the runner's locale, so compute the expectation the same way).
  const when = new Date(2026, 6, 15, 18, 0).toLocaleString(undefined, {
    weekday: "short",
    day: "numeric",
    month: "short",
    hour: "2-digit",
    minute: "2-digit",
  });
  expect(screen.getAllByText(new RegExp(`was due .*${when}`)).length).toBe(2);
});

it("sends or reopens exactly the clicked entry", async () => {
  const onSendNow = vi.fn();
  const onOpenDraft = vi.fn();
  render(MissedSends, {
    entries: [entry(), entry({ id: 2, subject: "Second" })],
    onSendNow,
    onOpenDraft,
    onDismiss: noop,
  });

  await fireEvent.click(
    screen.getByRole("button", { name: "Send Quarterly report now" }),
  );
  await fireEvent.click(
    screen.getByRole("button", { name: "Open Second as draft" }),
  );

  expect(onSendNow).toHaveBeenCalledWith(1);
  expect(onOpenDraft).toHaveBeenCalledWith(2);
});

it("lets the user defer the decision", async () => {
  const onDismiss = vi.fn();
  render(MissedSends, {
    entries: [entry()],
    onSendNow: noop,
    onOpenDraft: noop,
    onDismiss,
  });

  await fireEvent.click(screen.getByRole("button", { name: "Decide later" }));

  expect(onDismiss).toHaveBeenCalled();
});

it("behaves as a modal: announced as one and focused on open", async () => {
  render(MissedSends, {
    entries: [entry(), entry({ id: 2, subject: "Second" })],
    onSendNow: noop,
    onOpenDraft: noop,
    onDismiss: noop,
  });
  await tick();

  expect(screen.getByRole("alertdialog")).toHaveAttribute("aria-modal", "true");
  // The first control, which only opens a draft: nothing leaves on a stray Return.
  expect(document.activeElement).toBe(
    screen.getByRole("button", { name: "Open Quarterly report as draft" }),
  );
});

it("defers the decision on Escape", async () => {
  const onDismiss = vi.fn();
  render(MissedSends, {
    entries: [entry()],
    onSendNow: noop,
    onOpenDraft: noop,
    onDismiss,
  });

  await fireEvent.keyDown(window, { key: "Escape" });

  expect(onDismiss).toHaveBeenCalledTimes(1);
});

it("ignores Escape while there is nothing to decide", async () => {
  const onDismiss = vi.fn();
  render(MissedSends, {
    entries: [],
    onSendNow: noop,
    onOpenDraft: noop,
    onDismiss,
  });

  await fireEvent.keyDown(window, { key: "Escape" });

  expect(onDismiss).not.toHaveBeenCalled();
});
