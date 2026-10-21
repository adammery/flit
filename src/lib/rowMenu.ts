// What the message list's right-click menu offers for a set of rows. Kept pure
// and apart from App.svelte, which pops the menu up and runs the picks, so the
// per-folder rules are unit-tested without a DOM or a native menu — the same
// split as selection.ts.

import { acceleratorFor } from "./shortcuts";
import type {
  Mailbox,
  MessageHeader,
  ShortcutAction,
  ShortcutBinding,
} from "./types";

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
  /** `pick: null` greys the item out — it is shown, but has nothing to do.
   * `accelerator` is the action's keyboard shortcut, as native menus write
   * it ("Shift+Cmd+R"); absent when the action has none. */
  | {
      kind: "item";
      label: string;
      pick: MenuPick | null;
      accelerator?: string;
    }
  /** Greyed out when it has no items. */
  | { kind: "submenu"; label: string; items: MenuEntry[] }
  | { kind: "separator" };

const SEPARATOR: MenuEntry = { kind: "separator" };

function item(
  label: string,
  pick: MenuPick | null,
  accelerator?: string,
): MenuEntry {
  return accelerator === undefined
    ? { kind: "item", label, pick }
    : { kind: "item", label, pick, accelerator };
}

/**
 * The menu for `rows`: the right-clicked row alone, or the whole selection
 * when the click landed inside it. `isArchived` is App.svelte's own test, so
 * the menu says "Move to Inbox" exactly when the archive action would.
 * `shortcuts` are the user's bindings, shown beside the actions they fire.
 */
export function rowMenu(
  rows: MessageHeader[],
  folders: Record<number, Mailbox[]>,
  isArchived: (message: MessageHeader) => boolean,
  shortcuts: ShortcutBinding[] = [],
): MenuEntry[] {
  const keys = (action: ShortcutAction): string | undefined => {
    const combo = shortcuts.find((b) => b.action === action)?.combo;
    return combo ? acceleratorFor(combo) : undefined;
  };
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
  const trash = item("Move to Trash", { action: "trash" }, keys("trash"));

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
    item("Reply", single ? { action: "reply" } : null, keys("reply")),
    item(
      "Reply All",
      single ? { action: "reply-all" } : null,
      keys("reply-all"),
    ),
    item("Forward", single ? { action: "forward" } : null, keys("forward")),
    SEPARATOR,
    item(
      allRead ? "Mark as Unread" : "Mark as Read",
      { action: "read", read: !allRead },
      keys("toggle-read"),
    ),
    SEPARATOR,
    // App.svelte's archive handler flips an archived row back to the inbox.
    item(
      rows.every(isArchived) ? "Move to Inbox" : "Archive",
      { action: "archive" },
      keys("archive"),
    ),
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
