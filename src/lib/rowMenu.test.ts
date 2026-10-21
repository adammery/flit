import { describe, expect, it } from "vitest";
import { rowMenu, type MenuEntry } from "./rowMenu";
import type { Mailbox, MessageHeader } from "./types";

function folder(id: number, name: string, role: string | null): Mailbox {
  return { id, accountId: 1, name, role, displayName: name, unreadCount: 0 };
}

// Account 1 has the full set of folders; account 2 only an inbox.
const folders: Record<number, Mailbox[]> = {
  1: [
    folder(1, "INBOX", "inbox"),
    folder(2, "Archive", "archive"),
    folder(3, "Junk", "junk"),
    folder(4, "Trash", "trash"),
    folder(5, "Sent", "sent"),
    folder(6, "Drafts", "drafts"),
    folder(7, "Projects", null),
  ],
  2: [{ ...folder(8, "INBOX", "inbox"), accountId: 2 }],
};

function row(overrides: Partial<MessageHeader> = {}): MessageHeader {
  return {
    id: 1,
    accountId: 1,
    mailbox: "INBOX",
    from: "Alice <alice@example.com>",
    to: "domco@example.com",
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
    snippet: "",
    date: "2026-07-07T09:15:00Z",
    read: false,
    hasAttachments: false,
    ...overrides,
  };
}

const notArchived = () => false;

/** The top-level entry labelled `label`. */
function find(entries: MenuEntry[], label: string): MenuEntry | undefined {
  for (const entry of entries) {
    if (entry.kind === "separator") continue;
    if (entry.label === label) return entry;
  }
  return undefined;
}

describe("rowMenu", () => {
  it("offers replies, read state and every move for one inbox row", () => {
    expect(rowMenu([row()], folders, notArchived)).toEqual([
      { kind: "item", label: "Reply", pick: { action: "reply" } },
      { kind: "item", label: "Reply All", pick: { action: "reply-all" } },
      { kind: "item", label: "Forward", pick: { action: "forward" } },
      { kind: "separator" },
      {
        kind: "item",
        label: "Mark as Read",
        pick: { action: "read", read: true },
      },
      { kind: "separator" },
      { kind: "item", label: "Archive", pick: { action: "archive" } },
      {
        kind: "item",
        label: "Move to Junk",
        pick: { action: "move", mailbox: "Junk" },
      },
      { kind: "item", label: "Move to Trash", pick: { action: "trash" } },
      { kind: "separator" },
      {
        kind: "submenu",
        label: "Move to",
        // Every folder of the account but the one the row sits in.
        items: ["Archive", "Junk", "Trash", "Sent", "Drafts", "Projects"].map(
          (name) => ({
            kind: "item",
            label: name,
            pick: { action: "move", mailbox: name },
          }),
        ),
      },
    ]);
  });

  it("greys the replies out for more than one row", () => {
    const entries = rowMenu([row(), row({ id: 2 })], folders, notArchived);

    for (const label of ["Reply", "Reply All", "Forward"]) {
      expect(find(entries, label)).toMatchObject({ pick: null });
    }
    expect(find(entries, "Archive")).toMatchObject({
      pick: { action: "archive" },
    });
  });

  it("offers Mark as Unread once every row is read", () => {
    const entries = rowMenu(
      [row({ read: true }), row({ id: 2, read: true })],
      folders,
      notArchived,
    );

    expect(find(entries, "Mark as Unread")).toMatchObject({
      pick: { action: "read", read: false },
    });
  });

  it("marks a mixed set read rather than unread", () => {
    const entries = rowMenu(
      [row({ read: true }), row({ id: 2, read: false })],
      folders,
      notArchived,
    );

    expect(find(entries, "Mark as Read")).toBeDefined();
  });

  it("turns Archive into Move to Inbox for archived rows", () => {
    const entries = rowMenu([row({ mailbox: "Archive" })], folders, () => true);

    expect(find(entries, "Archive")).toBeUndefined();
    // App.svelte's archive handler is what flips an archived row back.
    expect(find(entries, "Move to Inbox")).toMatchObject({
      pick: { action: "archive" },
    });
  });

  it("turns Move to Junk into Not Junk inside the junk folder", () => {
    const entries = rowMenu([row({ mailbox: "Junk" })], folders, notArchived);

    expect(find(entries, "Move to Junk")).toBeUndefined();
    expect(find(entries, "Not Junk")).toMatchObject({
      pick: { action: "move", mailbox: "INBOX" },
    });
  });

  it("leaves junk out of Sent, and junk and trash out of Trash", () => {
    const sent = rowMenu([row({ mailbox: "Sent" })], folders, notArchived);
    expect(find(sent, "Move to Junk")).toBeUndefined();
    expect(find(sent, "Move to Trash")).toBeDefined();

    const trash = rowMenu([row({ mailbox: "Trash" })], folders, notArchived);
    expect(find(trash, "Move to Junk")).toBeUndefined();
    expect(find(trash, "Move to Trash")).toBeUndefined();
  });

  it("leaves junk out when the account has no junk folder", () => {
    const entries = rowMenu([row({ accountId: 2 })], folders, notArchived);

    expect(find(entries, "Move to Junk")).toBeUndefined();
  });

  it("offers editing, trash and moves for a draft", () => {
    expect(rowMenu([row({ mailbox: "Drafts" })], folders, notArchived)).toEqual(
      [
        { kind: "item", label: "Edit Draft", pick: { action: "edit-draft" } },
        { kind: "separator" },
        { kind: "item", label: "Move to Trash", pick: { action: "trash" } },
        { kind: "separator" },
        {
          kind: "submenu",
          label: "Move to",
          items: ["INBOX", "Archive", "Junk", "Trash", "Sent", "Projects"].map(
            (name) => ({
              kind: "item",
              label: name,
              pick: { action: "move", mailbox: name },
            }),
          ),
        },
      ],
    );
  });

  it("greys Edit Draft out for more than one draft", () => {
    const entries = rowMenu(
      [row({ mailbox: "Drafts" }), row({ id: 2, mailbox: "Drafts" })],
      folders,
      notArchived,
    );

    expect(find(entries, "Edit Draft")).toMatchObject({ pick: null });
  });

  // why: folder names belong to one account, and a bulk move sends a single
  // name for every row.
  it("greys the folder moves out for rows from two accounts", () => {
    const entries = rowMenu(
      [row(), row({ id: 2, accountId: 2 })],
      folders,
      notArchived,
    );

    expect(find(entries, "Move to")).toMatchObject({ items: [] });
    expect(find(entries, "Move to Junk")).toMatchObject({ pick: null });
    // Archive and trash find each account's own folder in the backend.
    expect(find(entries, "Archive")).toMatchObject({
      pick: { action: "archive" },
    });
    expect(find(entries, "Move to Trash")).toMatchObject({
      pick: { action: "trash" },
    });
  });

  it("lists every folder under Move to for rows from different ones", () => {
    const entries = rowMenu(
      [row(), row({ id: 2, mailbox: "Projects" })],
      folders,
      notArchived,
    );

    const moveTo = find(entries, "Move to");
    expect(moveTo?.kind === "submenu" && moveTo.items).toHaveLength(7);
  });
});
