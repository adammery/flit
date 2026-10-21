// What the message list's right-click menu offers for a set of rows. Kept pure
// and apart from App.svelte, which pops the menu up and runs the picks, so the
// per-folder rules are unit-tested without a DOM or a native menu — the same
// split as selection.ts.

import type { Mailbox, MessageHeader } from "./types";

/** What picking an item does. App.svelte runs each through the handler its
 * toolbar button, swipe or shortcut already uses. */
export type MenuPick =
  | {
      action:
        | "reply"
        | "reply-all"
        | "forward"
        | "edit-draft"
        | "archive"
        | "trash";
    }
  | { action: "read"; read: boolean }
  | { action: "move"; mailbox: string };

export type MenuEntry =
  /** `pick: null` greys the item out — it is shown, but has nothing to do. */
  | { kind: "item"; label: string; pick: MenuPick | null }
  /** Greyed out when it has no items. */
  | { kind: "submenu"; label: string; items: MenuEntry[] }
  | { kind: "separator" };

const SEPARATOR: MenuEntry = { kind: "separator" };

function item(label: string, pick: MenuPick | null): MenuEntry {
  return { kind: "item", label, pick };
}

/**
 * The menu for `rows`: the right-clicked row alone, or the whole selection
 * when the click landed inside it. `isArchived` is App.svelte's own test, so
 * the menu says "Move to Inbox" exactly when the archive action would.
 */
export function rowMenu(
  rows: MessageHeader[],
  folders: Record<number, Mailbox[]>,
  isArchived: (message: MessageHeader) => boolean,
): MenuEntry[] {
  const single = rows.length === 1;
  // why one account: folder names belong to an account, and a bulk move
  // sends a single name for every row. null = the rows span accounts.
  const account =
    new Set(rows.map((row) => row.accountId)).size === 1
      ? (folders[rows[0].accountId] ?? [])
      : null;
  const roles = new Set(
    rows.map(
      (row) =>
        (folders[row.accountId] ?? []).find((m) => m.name === row.mailbox)
          ?.role ?? null,
    ),
  );
  // The folder kind every row sits in; null when they differ (search hits).
  const role = roles.size === 1 ? [...roles][0] : null;
  const mailboxes = new Set(rows.map((row) => row.mailbox));
  const current = mailboxes.size === 1 ? rows[0].mailbox : null;

  // Like the toolbar's Move menu: every folder but the one the rows are in.
  const moveTo: MenuEntry = {
    kind: "submenu",
    label: "Move to",
    items: (account ?? [])
      .filter((m) => m.name !== current)
      .map((m) => item(m.displayName, { action: "move", mailbox: m.name })),
  };
  const trash = item("Move to Trash", { action: "trash" });

  // A draft is unfinished work: it is edited, not answered, and read state,
  // archiving and junk mean nothing for it.
  if (role === "drafts") {
    return [
      item("Edit Draft", single ? { action: "edit-draft" } : null),
      SEPARATOR,
      trash,
      SEPARATOR,
      moveTo,
    ];
  }

  // Unread only once everything is read — the toolbar's rule.
  const allRead = rows.every((row) => row.read);
  return [
    item("Reply", single ? { action: "reply" } : null),
    item("Reply All", single ? { action: "reply-all" } : null),
    item("Forward", single ? { action: "forward" } : null),
    SEPARATOR,
    item(allRead ? "Mark as Unread" : "Mark as Read", {
      action: "read",
      read: !allRead,
    }),
    SEPARATOR,
    // App.svelte's archive handler flips an archived row back to the inbox.
    item(rows.every(isArchived) ? "Move to Inbox" : "Archive", {
      action: "archive",
    }),
    ...junkItems(role, account),
    ...(role === "trash" ? [] : [trash]),
    SEPARATOR,
    moveTo,
  ];
}

/**
 * Junk is a move like any other: into the account's junk folder, or back to
 * its inbox from there. Mail the user sent or already binned is not spam, so
 * Sent and Trash leave it out (Drafts never gets here).
 */
function junkItems(
  role: string | null,
  account: Mailbox[] | null,
): MenuEntry[] {
  if (role === "sent" || role === "trash") return [];
  if (role === "junk") {
    const inbox = account?.find((m) => m.role === "inbox")?.name ?? "INBOX";
    return [
      item("Not Junk", account ? { action: "move", mailbox: inbox } : null),
    ];
  }
  if (account === null) return [item("Move to Junk", null)];
  const junk = account.find((m) => m.role === "junk");
  if (!junk) return [];
  return [item("Move to Junk", { action: "move", mailbox: junk.name })];
}
