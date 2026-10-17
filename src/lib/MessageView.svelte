<script lang="ts">
  import {
    cachedSummary,
    listThread,
    setMessageRead,
    threadBodies,
  } from "./api";
  import type { DraftKind } from "./draft";
  import MessageCard from "./MessageCard.svelte";
  import { runSummary, type SummaryRun, type SummaryState } from "./summaries";
  import SummaryPanel from "./SummaryPanel.svelte";
  import { timed } from "./timing";
  import type { MessageBody, MessageHeader, ThreadOrder } from "./types";

  let {
    message,
    selectedCount = 0,
    focusSelected = false,
    accountEmails = {},
    accountColors = {},
    threadOrder = "newestLast",
    onDraft,
    onEditDraft,
    onDeleteDraft,
    canSummarize = false,
    translateInto = null,
  }: {
    message: MessageHeader | null;
    /** How many rows the list has selected. Past one there is no single
     * message to read, so the pane says how many are staged instead. */
    selectedCount?: number;
    /** Open the clicked message itself instead of the thread's newest.
     * Search results are rows for one message, not for a thread — a folder
     * row stays the thread's face and keeps opening the newest as before. */
    focusSelected?: boolean;
    /** accountId → address; marks a card's sender as "me". */
    accountEmails?: Record<number, string>;
    /** accountId → accent color for the "me" avatar. */
    accountColors?: Record<number, string | null>;
    /** Which end of the conversation the newest message renders at. */
    threadOrder?: ThreadOrder;
    /** Per-message reply actions in card footers. */
    onDraft?: (kind: DraftKind, message: MessageHeader) => void;
    /** A clicked draft card resumes editing in a compose window. */
    onEditDraft?: (id: number) => void;
    /** The draft card's Delete action — removes the server draft. */
    onDeleteDraft?: (message: MessageHeader) => void;
    /** Whether cards offer the on-device summary button. */
    canSummarize?: boolean;
    /** The language cards offer translation into; null when they don't. */
    translateInto?: string | null;
  } = $props();

  // The whole conversation of the selected row, oldest first. Falls back to
  // just the selected message so the viewer never comes up empty.
  let thread = $state<MessageHeader[]>([]);
  // Like the design: the newest message starts open, older ones collapse to
  // a preview row; any number can be open at once.
  let expandedIds = $state<Set<number>>(new Set());
  // Bodies arrive in one bulk call (single server connection for whatever
  // isn't cached), keyed by message id.
  let bodies = $state<Record<number, MessageBody>>({});
  let bodiesLoading = $state(false);
  let bodiesError = $state<string | null>(null);
  // why plain: only steers whether a reload resets the open state — nothing
  // renders from it.
  let anchorId: number | null = null;
  // The pane's scroll container, and the card (if any) to scroll into view
  // once it has grown to its final size.
  let threadEl = $state<HTMLDivElement | null>(null);
  let scrollTargetId = $state<number | null>(null);
  // The on-device summary of the whole conversation, once asked for.
  let threadSummary = $state<SummaryState | null>(null);
  let threadSummaryRun: SummaryRun | null = null;

  function summarizeConversation(fresh = false) {
    if (message === null) return;
    const id = message.id;
    threadSummaryRun?.cancel();
    threadSummaryRun = runSummary("thread", id, fresh, (state) => {
      if (message?.id === id) threadSummary = state;
    });
  }

  function cancelConversationSummary() {
    threadSummaryRun?.cancel();
    threadSummaryRun = null;
    threadSummary = null;
  }

  $effect(() => {
    if (message === null) {
      thread = [];
      bodies = {};
      expandedIds = new Set();
      anchorId = null;
      scrollTargetId = null;
      cancelConversationSummary();
      return;
    }
    const id = message.id;
    const fallback = message;
    timed("listThread", () => listThread(id))
      .then((loaded) => {
        if (message?.id !== id) return;
        thread = loaded.length > 0 ? loaded : [fallback];
        if (anchorId !== id) {
          // A fresh selection: open the message the click landed on (or the
          // newest, for an ordinary folder row), load all bodies.
          anchorId = id;
          bodies = {};
          cancelConversationSummary();
          const anchor = focusSelected
            ? anchorMessage(thread, fallback)
            : newestMessage(thread);
          openMessage(anchor);
          // Only a focused selection needs to scroll anywhere — an ordinary
          // folder row's newest card is unconditionally the one just opened.
          scrollTargetId = focusSelected ? anchor.id : null;
          fetchBodies(id);
        } else if (thread.some((entry) => !(entry.id in bodies))) {
          // Same conversation refreshed and grew (a reply just synced in) —
          // top up the missing bodies without resetting the open state.
          fetchBodies(id);
        }
      })
      .catch((err: unknown) => {
        console.error("failed to load thread:", err);
        if (message?.id === id) {
          thread = [fallback];
          if (anchorId !== id) {
            anchorId = id;
            bodies = {};
            openMessage(fallback);
            scrollTargetId = null;
            fetchBodies(id);
          }
        }
      });
  });

  // Scroll the focused card into view once its body has landed — it starts
  // out empty and grows to full height only then, so scrolling any earlier
  // would leave the pane short of where the card ends up.
  $effect(() => {
    if (scrollTargetId === null || bodiesLoading) return;
    const id = scrollTargetId;
    threadEl
      ?.querySelector<HTMLElement>(`[data-message-id="${id}"]`)
      ?.scrollIntoView({ block: "nearest" });
    scrollTargetId = null;
  });

  function fetchBodies(anchor: number) {
    bodiesLoading = true;
    bodiesError = null;
    // why timed here: this is the wait between clicking a message and seeing
    // any body at all — every member of the thread has to land first.
    timed("threadBodies", () => threadBodies(anchor))
      .then((loaded) => {
        if (anchorId === anchor) {
          bodies = loaded;
          showCachedConversationSummary(anchor);
        }
      })
      .catch((err: unknown) => {
        if (anchorId === anchor) bodiesError = String(err);
      })
      .finally(() => {
        if (anchorId === anchor) bodiesLoading = false;
      });
  }

  // A conversation opens with the summary it already has. Asked only once
  // its bodies are here, so the lookup is a cache hit and never a second
  // server fetch racing the bulk one.
  function showCachedConversationSummary(anchor: number) {
    if (!canSummarize || thread.length < 2 || threadSummary !== null) return;
    cachedSummary(anchor, true)
      .then((text) => {
        if (text !== null && anchorId === anchor && threadSummary === null) {
          threadSummary = { loading: false, text, error: null };
        }
      })
      .catch(() => undefined);
  }

  /** Open a card (fresh selections replace the set), marking it read —
   * same as opening it from the list. */
  function openMessage(entry: MessageHeader) {
    expandedIds = new Set([entry.id]);
    markRead(entry);
  }

  function markRead(entry: MessageHeader) {
    if (entry.read) return;
    void setMessageRead(entry.id, true).catch((err: unknown) =>
      console.error("failed to mark thread message read:", err),
    );
  }

  /** The newest real message — a trailing draft never auto-opens (it is
   * edited in a compose window, never read inline). Falls back to the last
   * entry so a draft-only thread still highlights something. */
  function newestMessage(entries: MessageHeader[]): MessageHeader {
    return (
      [...entries].reverse().find((entry) => !entry.isDraft) ??
      entries[entries.length - 1]
    );
  }

  /** The thread member the click actually landed on. Matched by RFC
   * Message-ID first: the backend dedupes server-side copies of one message
   * by Message-ID, so the clicked row's own id can be a different folder's
   * copy and simply absent from the thread. The row id is a fallback for
   * senders that set no Message-ID (never matched via a shared blank
   * string), and "newest" is the last resort — same as an unfocused open,
   * and also what a draft anchor falls back to, since a draft never opens
   * as the read view. */
  function anchorMessage(
    entries: MessageHeader[],
    target: MessageHeader,
  ): MessageHeader {
    const found =
      entries.find(
        (entry) => target.messageId !== "" && entry.messageId === target.messageId,
      ) ?? entries.find((entry) => entry.id === target.id);
    return found && !found.isDraft ? found : newestMessage(entries);
  }

  // Display order only — `thread` stays oldest-first everywhere else
  // (newest lookup, default-open logic).
  let displayThread = $derived(
    threadOrder === "newestFirst" ? [...thread].reverse() : thread,
  );
  let newestId = $derived(
    thread.length > 0 ? newestMessage(thread).id : null,
  );

  function toggle(entry: MessageHeader) {
    // Drafts have no reading view — the click resumes editing instead.
    if (entry.isDraft) {
      onEditDraft?.(entry.id);
      return;
    }
    const next = new Set(expandedIds);
    if (next.has(entry.id)) {
      next.delete(entry.id);
    } else {
      next.add(entry.id);
      markRead(entry);
    }
    expandedIds = next;
  }
</script>

<article>
  {#if message === null}
    <p class="empty">
      {selectedCount > 1
        ? `${selectedCount} messages selected`
        : "Select a message"}
    </p>
  {:else}
    <div class="thread" bind:this={threadEl}>
      <div class="stack">
        <div class="thread-head">
          <h2 class="subject">{message.subject}</h2>
        </div>
        {#if canSummarize && thread.length > 1}
          <div class="thread-summary">
            <SummaryPanel
              summary={threadSummary}
              label="Conversation summary"
              startLabel="Summarize this conversation"
              collapseKey="t{message.id}"
              onStart={() => summarizeConversation()}
              onRetry={() => summarizeConversation(true)}
              onCancel={cancelConversationSummary}
            />
          </div>
        {/if}
        {#each displayThread as entry (entry.id)}
          <MessageCard
            message={entry}
            body={bodies[entry.id] ?? null}
            canSummarize={canSummarize && !entry.isDraft}
            translateInto={entry.isDraft ? null : translateInto}
            loading={bodiesLoading}
            error={bodiesError}
            expanded={entry.isDraft || expandedIds.has(entry.id)}
            last={entry.id === newestId}
            ownEmail={accountEmails[entry.accountId] ?? null}
            accountColor={accountColors[entry.accountId] ?? null}
            onToggle={() => toggle(entry)}
            onDraft={(kind) => onDraft?.(kind, entry)}
            onDelete={entry.isDraft ? () => onDeleteDraft?.(entry) : undefined}
          />
        {/each}
      </div>
    </div>
  {/if}
</article>

<style>
  article {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    background: var(--bg-thread);
  }

  .empty {
    margin: auto;
    color: var(--text-tertiary);
  }

  /* One scroll container for the whole stack — bodies are shown whole and
     auto-sized, the conversation scrolls as a single column. */
  .thread {
    flex: 1;
    min-height: 0;
    padding: 18px 22px;
    overflow-y: auto;
    /* why: overflow-y auto alone computes overflow-x to auto — a squeezed
       pane must clip sideways, never grow a horizontal scrollbar. */
    overflow-x: hidden;
  }

  .stack {
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 860px;
    margin: 0 auto;
  }

  .thread-head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin-bottom: 4px;
  }

  .subject {
    overflow: hidden;
    margin: 0;
    font-size: 17px;
    font-weight: 700;
    color: var(--text-primary);
    text-overflow: ellipsis;
    white-space: nowrap;
    /* why: the body opts chrome out of selection; the subject is content. */
    user-select: text;
  }

  .thread-summary {
    margin-bottom: 12px;
  }
</style>
