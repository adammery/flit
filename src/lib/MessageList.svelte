<script lang="ts">
  import { avatarColor, senderDomain } from "./avatar";
  import {
    formatListDate,
    sectionFor,
    senderInitials,
    senderName,
  } from "./format";
  import { accumulatePull, shouldRefresh } from "./pullRefresh";
  import {
    accumulateOffset,
    actionFor,
    clampOffset,
    DEFAULT_SWIPE_ACTIONS,
    DRAG_SLOP,
    isHorizontal,
  } from "./swipe";
  import { modeFor, type SelectMode } from "./selection";
  import { offsetsFor, windowFor } from "./virtualList";
  import type {
    MessageHeader,
    SearchCorrection,
    SwipeAction,
    SwipeActions,
    ViewStatus,
  } from "./types";

  let {
    title,
    messages,
    accountColors = {},
    avatars = {},
    selectedId,
    selectedIds,
    onSelect,
    swipeActions = DEFAULT_SWIPE_ACTIONS,
    onArchive,
    onSetRead,
    isArchived,
    onTrash,
    onReply,
    status = null,
    corrections = [],
    onLoadMore,
    onRefresh,
    refreshing = false,
  }: {
    title: string;
    messages: MessageHeader[];
    /** accountId → accent color (or null); drives the per-row color dot. */
    accountColors?: Record<number, string | null>;
    /** sender domain → icon data: URI. Empty unless the user switched the
     * lookup on; a domain that is absent keeps its monogram. */
    avatars?: Record<string, string>;
    /** The lead row — what the reading pane shows and the keyboard walks.
     * Only scrolling follows it; the highlight follows `selectedIds`. */
    selectedId: number | null;
    /** Every selected row. Omitted (the common single-selection case) it
     * stands in as just the lead row. */
    selectedIds?: number[];
    onSelect: (id: number, mode: SelectMode) => void;
    /** Which action a full swipe in each direction fires. */
    swipeActions?: SwipeActions;
    onArchive?: (id: number) => void;
    onSetRead?: (id: number, read: boolean) => void;
    /** Row already sits in its archive folder — the swipe backdrop
     * reads "Move to Inbox" (onArchive still fires; the parent routes). */
    isArchived?: (message: MessageHeader) => boolean;
    onTrash?: (id: number) => void;
    onReply?: (id: number) => void;
    /** Whole-view totals + backfill progress; null (e.g. search results)
     * falls back to counting the rows at hand. */
    status?: ViewStatus | null;
    /** Typed words a search replaced with the cached word they most likely
     * meant — said under the count, so a hit for "bunny" when the user
     * typed "bnny" is never a mystery. */
    corrections?: SearchCorrection[];
    /** Ask the parent to reveal more rows — fired near the list's bottom
     * while more rows exist than are loaded. */
    onLoadMore?: () => void;
    /** A pull past the top released deep enough — check for new mail. */
    onRefresh?: () => void;
    /** The pull's refresh is in flight — pins the indicator open. */
    refreshing?: boolean;
  } = $props();

  // why a Set: a shift-range can hold hundreds of ids and every row asks.
  let selection = $derived(
    new Set(selectedIds ?? (selectedId === null ? [] : [selectedId])),
  );

  /** Strip color + label for each swipe action ("none" never renders). */
  const SWIPE_STRIPS: Record<
    Exclude<SwipeAction, "none">,
    { color: string; label: (message: MessageHeader) => string }
  > = {
    archive: {
      color: "#4f7cf7",
      label: (m) => (isArchived?.(m) ? "Move to Inbox" : "Archive"),
    },
    toggleRead: {
      color: "#f0a132",
      label: (m) => (m.read ? "Mark Unread" : "Mark Read"),
    },
    trash: { color: "#e5484d", label: () => "Trash" },
    reply: { color: "#7a5af8", label: () => "Reply" },
  };

  // The row currently under a two-finger swipe and how far it has traveled.
  // One gesture at a time — trackpads can't swipe two rows at once.
  let swipeId = $state<number | null>(null);
  let swipeOffset = $state(0);
  let settleTimer: ReturnType<typeof setTimeout> | undefined;

  function handleWheel(message: MessageHeader, event: WheelEvent) {
    if (!isHorizontal(event.deltaX, event.deltaY)) return;
    event.preventDefault();
    if (swipeId !== message.id) {
      swipeId = message.id;
      swipeOffset = 0;
    }
    swipeOffset = accumulateOffset(swipeOffset, event.deltaX, swipeActions);
    // why a timer: DOM wheel streams have no "gesture ended" event — a quiet
    // gap longer than the ~10-20ms between trackpad pulses means release.
    clearTimeout(settleTimer);
    settleTimer = setTimeout(() => settleSwipe(message), 120);
  }

  // Mouse/touch drag: pointerdown only arms a candidate — the swipe starts
  // once the pointer travels past DRAG_SLOP with horizontal dominating, so a
  // plain click still selects and a vertical move never grabs the row.
  let dragId: number | null = null;
  let dragStartX = 0;
  let dragStartY = 0;
  let dragActive = false;
  let dragConsumedClick = false;

  function handlePointerDown(message: MessageHeader, event: PointerEvent) {
    if (event.button !== 0) return;
    dragId = message.id;
    dragStartX = event.clientX;
    dragStartY = event.clientY;
    dragActive = false;
  }

  function handlePointerMove(message: MessageHeader, event: PointerEvent) {
    if (dragId !== message.id) return;
    const dx = event.clientX - dragStartX;
    const dy = event.clientY - dragStartY;
    if (!dragActive) {
      if (Math.abs(dx) <= DRAG_SLOP || !isHorizontal(dx, dy)) return;
      dragActive = true;
      // why capture: move/up events keep hitting this row even when the
      // pointer leaves it mid-drag. Optional call — jsdom lacks it.
      (event.currentTarget as HTMLElement).setPointerCapture?.(
        event.pointerId,
      );
      // why: a pending wheel settle would reset the row mid-drag.
      clearTimeout(settleTimer);
      swipeId = message.id;
    }
    swipeOffset = clampOffset(dx, swipeActions);
  }

  function handlePointerUp(message: MessageHeader) {
    if (dragId !== message.id) return;
    dragId = null;
    if (!dragActive) return;
    dragActive = false;
    // why: the browser fires a click right after this pointerup — swallow
    // it so releasing a swipe never also selects the row.
    dragConsumedClick = true;
    // pointerup IS the release — settle now, no quiet-gap timer needed.
    settleSwipe(message);
  }

  function handlePointerCancel(message: MessageHeader) {
    if (dragId !== message.id) return;
    // The system took the pointer (e.g. a native gesture) — snap back.
    dragId = null;
    if (!dragActive) return;
    dragActive = false;
    swipeId = null;
    swipeOffset = 0;
  }

  function handleClick(message: MessageHeader, event: MouseEvent) {
    if (dragConsumedClick) {
      dragConsumedClick = false;
      return;
    }
    onSelect(message.id, modeFor(event));
  }

  function settleSwipe(message: MessageHeader) {
    if (swipeId !== message.id) return;
    const action = actionFor(swipeOffset, swipeActions);
    // Reset first so the row snaps back even when no action fired.
    swipeId = null;
    swipeOffset = 0;
    if (action === "archive") onArchive?.(message.id);
    else if (action === "toggleRead") onSetRead?.(message.id, !message.read);
    else if (action === "trash") onTrash?.(message.id);
    else if (action === "reply") onReply?.(message.id);
  }

  // Pull-to-refresh: vertical wheel pulses past the top open an indicator
  // drawer above the list; releasing deep enough asks the parent to sync.
  let pullDepth = $state(0);
  let pullTimer: ReturnType<typeof setTimeout> | undefined;
  /** Drawer height (px) while the refresh runs — one comfortable row. */
  const REFRESH_HEIGHT = 34;

  function handlePullWheel(event: WheelEvent) {
    if (!onRefresh || refreshing) return;
    // Dominantly horizontal pulses belong to the row swipes.
    if (isHorizontal(event.deltaX, event.deltaY)) return;
    // Engage only when already at the very top and moving further up; once
    // engaged, keep the gesture until it settles so it can also ease back.
    if (pullDepth === 0 && ((listEl?.scrollTop ?? 0) > 0 || event.deltaY >= 0))
      return;
    event.preventDefault();
    pullDepth = accumulatePull(pullDepth, event.deltaY);
    // Same trick as the row swipes: wheel streams have no end event — a
    // quiet gap between trackpad pulses means the fingers lifted.
    clearTimeout(pullTimer);
    pullTimer = setTimeout(settlePull, 120);
  }

  function settlePull() {
    if (shouldRefresh(pullDepth)) onRefresh?.();
    // Collapse either way — a triggered refresh reopens via `refreshing`.
    pullDepth = 0;
  }

  // why: a folder switch can replace the list while a swipe or pull is still
  // in flight; the pending settle would then fire its action (archive,
  // refresh…) from a torn-down component. The effect reads no state, so it
  // runs once and its teardown runs exactly at destroy.
  $effect(() => () => {
    clearTimeout(settleTimer);
    clearTimeout(pullTimer);
  });

  let unreadCount = $derived(
    status?.unread ?? messages.filter((m) => !m.read).length,
  );
  let totalRows = $derived(status?.listRows ?? messages.length);
  let hasMore = $derived(status !== null && messages.length < status.listRows);
  // Backfill progress, shown only while the server holds more than the
  // cache; disappears on its own once the mailbox is fully mirrored.
  let syncing = $derived(
    status !== null &&
      status.serverTotal !== null &&
      status.cached < status.serverTotal
      ? { cached: status.cached, total: status.serverTotal }
      : null,
  );

  // Rows annotated with their date section; a header renders wherever the
  // section changes (the list is already sorted date-desc). One `now` for
  // the whole pass so a render can't straddle midnight.
  let rows = $derived.by(() => {
    const now = new Date();
    let prev: string | null = null;
    return messages.map((message) => {
      const section = sectionFor(message.date, now);
      const opens = section !== prev;
      prev = section;
      return { message, section, opens };
    });
  });

  // Only the rows near the viewport go into the DOM. A big folder hands this
  // component 500 rows and drawing them all measured 85-878 ms — in one
  // folder more than the query that produced them.
  //
  // why a row and its date header count as one item: the header always
  // renders directly above its row, so the two scroll as a unit and the
  // existing markup needs no restructuring — only a taller item.
  const OVERSCAN = 6;
  /** Fallbacks until the first row has been measured. */
  const ROW_FALLBACK = 62;
  const HEADER_FALLBACK = 30;

  let rowHeight = $state(ROW_FALLBACK);
  let headerHeight = $state(HEADER_FALLBACK);
  let scrollTop = $state(0);
  let viewportHeight = $state(0);

  let offsets = $derived(
    offsetsFor(rows.map((r) => rowHeight + (r.opens ? headerHeight : 0))),
  );
  let visible = $derived(
    windowFor(offsets, scrollTop, viewportHeight, OVERSCAN),
  );
  let windowRows = $derived(rows.slice(visible.start, visible.end));

  // why measured rather than hard-coded: the row's height comes out of type
  // and padding, and a stylesheet edit that silently disagreed with a
  // constant here would misplace every row below the fold.
  function measure() {
    if (listEl === null) return;
    viewportHeight = listEl.clientHeight;
    // why the > 0 guard: an element that has not laid out yet — and every
    // element under jsdom — reports zero. Believing that would make every
    // row zero-tall, and the window would collapse onto the last row.
    const row = listEl.querySelector<HTMLElement>(".swipe-row");
    if (row && row.offsetHeight > 0) rowHeight = row.offsetHeight;
    const header = listEl.querySelector<HTMLElement>(".section");
    if (header && header.offsetHeight > 0) headerHeight = header.offsetHeight;
  }

  /** How close to the bottom (px) the scroll gets before asking for more. */
  const LOAD_MORE_THRESHOLD = 300;

  function handleScroll() {
    if (listEl === null) return;
    scrollTop = listEl.scrollTop;
    if (!hasMore || !onLoadMore) return;
    const remaining =
      listEl.scrollHeight - listEl.scrollTop - listEl.clientHeight;
    // Duplicate fires are fine — the parent's reveal logic is idempotent
    // until the longer list actually arrives.
    if (remaining < LOAD_MORE_THRESHOLD) onLoadMore();
  }

  // why: keyboard navigation moves the selection without scrolling — keep the
  // selected row in view so arrowing through a long list stays usable.
  // why the lead and not ".selected": a shift-range marks many rows, and the
  // one worth keeping in view is the end the user just moved to.
  let listEl = $state<HTMLElement | null>(null);
  $effect(() => {
    if (listEl === null) return;
    measure();
    if (selectedId === null) return;
    const lead = listEl.querySelector<HTMLElement>("[data-lead]");
    if (lead) {
      lead.scrollIntoView({ block: "nearest" });
      return;
    }
    // why by offset: the lead row is outside the drawn window, so there is
    // no element to scroll to. Jumping to where it will be brings it into
    // the window, and the row renders there.
    const index = rows.findIndex((r) => r.message.id === selectedId);
    if (index >= 0) {
      listEl.scrollTop = Math.max(0, offsets[index] - listEl.clientHeight / 2);
      scrollTop = listEl.scrollTop;
    }
  });
</script>

<div class="pane">
  <header>
    <h1>{title}</h1>
    <p class="count">
      {totalRows}
      {totalRows === 1 ? "message" : "messages"}{unreadCount > 0
        ? `, ${unreadCount} unread`
        : ""}
    </p>
    {#if corrections.length > 0}
      <p class="correction">
        Showing results for
        <!-- why the separator leads, as an expression: Svelte trims the
             whitespace at an each block's edges, so a trailing ", " in the
             markup would lose its space. -->
        {#each corrections as { typed, corrected }, i (typed)}
          {i > 0 ? ", " : ""}<strong>{corrected}</strong> instead of {typed}
        {/each}
      </p>
    {/if}
  </header>

  <!-- Always mounted so the drawer's height can animate open and closed;
       .pulling turns the transition off to track the fingers with no lag. -->
  <div
    class="pull"
    class:pulling={pullDepth > 0 && !refreshing}
    style:height={`${refreshing ? REFRESH_HEIGHT : pullDepth}px`}
    aria-live="polite"
  >
    {#if refreshing}
      <span class="pull-spinner" aria-hidden="true"></span>
      <span>Checking for new mail…</span>
    {:else if pullDepth > 0}
      <svg
        class="pull-arrow"
        class:armed={shouldRefresh(pullDepth)}
        viewBox="0 0 20 20"
        width="12"
        height="12"
        fill="none"
        stroke="currentColor"
        stroke-width="1.7"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d="M10 4v12m0 0 5-5m-5 5-5-5" />
      </svg>
      <span>{shouldRefresh(pullDepth) ? "Release to refresh" : "Pull to refresh"}</span>
    {/if}
  </div>

  <div
    class="list"
    role="listbox"
    aria-label="Messages"
    aria-multiselectable="true"
    bind:this={listEl}
    onscroll={handleScroll}
    onwheel={handlePullWheel}
  >
    {#if messages.length === 0}
      <p class="empty">No Messages</p>
    {:else}
      <!-- Stand-ins for the rows above and below the drawn window, so the
           scrollbar and the scroll position match the whole list.
           role="presentation" here and on .swipe-row: a listbox may only
           hold options, so every wrapper drops out of the a11y tree. -->
      <div
        class="spacer"
        role="presentation"
        style:height={`${visible.padTop}px`}
      ></div>
      {#each windowRows as { message, section, opens } (message.id)}
        {@const color = accountColors[message.accountId] ?? null}
        {@const domain = senderDomain(message.from)}
        {@const icon = domain ? avatars[domain] : undefined}
        {@const offset = swipeId === message.id ? swipeOffset : 0}
        {@const stripAction =
          offset < 0
            ? swipeActions.left
            : offset > 0
              ? swipeActions.right
              : "none"}
        {#if opens}
          <!-- aria-hidden: every row already carries its date, and a
               listbox's children must be options — the header is a purely
               visual landmark. -->
          <div class="section" aria-hidden="true">{section}</div>
        {/if}
        <!-- The pointer handlers implement the swipe gesture; keyboard
             users act on rows through the buttons and shortcuts instead. -->
        <div
          class="swipe-row"
          role="presentation"
          onwheel={(e) => handleWheel(message, e)}
          onpointerdown={(e) => handlePointerDown(message, e)}
          onpointermove={(e) => handlePointerMove(message, e)}
          onpointerup={() => handlePointerUp(message)}
          onpointercancel={() => handlePointerCancel(message)}
        >
          <!-- why the width style: the backdrop spans exactly the revealed
               strip, so it can never show through the row content above it
               (hover tints the row translucent). -->
          {#if stripAction !== "none"}
            <span
              class="swipe-bg"
              class:trailing={offset < 0}
              class:leading={offset > 0}
              aria-hidden="true"
              style:width={`${Math.abs(offset)}px`}
              style:background={SWIPE_STRIPS[stripAction].color}
            >
              {SWIPE_STRIPS[stripAction].label(message)}
            </span>
          {/if}
          <button
            role="option"
            aria-selected={selection.has(message.id)}
            data-lead={selectedId === message.id ? "true" : undefined}
            class:selected={selection.has(message.id)}
            class:unread={!message.read}
            class:swiping={offset !== 0}
            style:transform={offset === 0
              ? undefined
              : `translateX(${offset}px)`}
            onclick={(e) => handleClick(message, e)}
          >
            <!-- aria-hidden: the monogram is a visual shortcut to the sender
                 name, which the row already spells out right beside it. -->
            <span
              class="avatar"
              class:has-icon={icon !== undefined}
              aria-hidden="true"
              style:background={icon || !domain
                ? undefined
                : avatarColor(domain)}
            >
              {#if icon}
                <!-- alt="": the sender name sits right beside it, so the icon
                     is decoration and a screen reader should skip it. -->
                <img src={icon} alt="" />
              {:else}
                {senderInitials(message.from)}
              {/if}
            </span>
            <span class="content">
              <span class="row">
                {#if !message.read || message.threadUnread}
                  <span class="dot" aria-hidden="true"></span>
                {/if}
                <span class="from">{senderName(message.from)}</span>
                <span class="end">
                  {#if message.threadHasDraft}
                    <span class="draft-pill">Draft</span>
                  {/if}
                  {#if message.hasAttachments}
                    <svg
                      class="clip"
                      viewBox="0 0 20 20"
                      width="12"
                      height="12"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.7"
                      aria-hidden="true"
                    >
                      <title>Has attachments</title>
                      <path
                        d="M15.5 9.5 9.9 15a3.5 3.5 0 0 1-5-5l6.4-6.3a2.3 2.3 0 0 1 3.3 3.3L8.3 13.2a1.2 1.2 0 0 1-1.7-1.7l5.2-5.1"
                      />
                    </svg>
                  {/if}
                  {#if message.threadCount > 1}
                    <span
                      class="thread-count"
                      title="{message.threadCount} messages in conversation"
                    >
                      {message.threadCount}
                    </span>
                  {/if}
                  {#if color}
                    <span
                      class="account-dot"
                      aria-hidden="true"
                      style:background={color}
                    ></span>
                  {/if}
                  <span class="date">{formatListDate(message.date)}</span>
                </span>
              </span>
              <span class="subject">{message.subject}</span>
            </span>
          </button>
        </div>
      {/each}
      <div
        class="spacer"
        role="presentation"
        style:height={`${visible.padBottom}px`}
      ></div>
    {/if}
  </div>

  {#if syncing}
    <footer class="syncing" aria-live="polite">
      <span class="syncing-row">
        <span>Syncing older messages…</span>
        <span>{syncing.cached.toLocaleString()} of {syncing.total.toLocaleString()}</span>
      </span>
      <span class="syncing-bar" aria-hidden="true">
        <span
          class="syncing-fill"
          style:width={`${Math.min(100, (syncing.cached / syncing.total) * 100)}%`}
        ></span>
      </span>
    </footer>
  {/if}
</div>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg-window);
  }

  header {
    flex-shrink: 0;
    padding: 12px 16px 8px;
    border-bottom: 1px solid var(--hairline);
  }

  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 700;
  }

  .count {
    margin: 1px 0 0;
    font-size: 11px;
    color: var(--text-secondary);
  }

  .correction {
    margin: 4px 0 0;
    font-size: 11px;
    color: var(--text-secondary);
  }

  .correction strong {
    font-weight: 600;
    color: var(--text-primary);
  }

  /* why flex-shrink 0: the list is a flex column, and without it the
     spacers would be squeezed and the scroll height would be wrong. */
  .spacer {
    flex-shrink: 0;
  }

  .list {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    /* why: overflow-y auto alone computes overflow-x to auto — the list
       must clip sideways, never scroll (swipes translate rows, not panes). */
    overflow-x: hidden;
    /* why: without a stacking context here, the sticky section header's
       z-index escapes to the pane and paints over the overlay scrollbar
       thumb (WebKit draws a scroller's overlay scrollbars above its own
       stacking context, not above z-indexed escapees). */
    isolation: isolate;
  }

  .empty {
    margin: auto;
    color: var(--text-tertiary);
  }

  /* Pull-to-refresh drawer between the header and the list. */
  .pull {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    overflow: hidden;
    font-size: 11.5px;
    color: var(--text-secondary);
    transition: height 0.18s ease;
  }

  .pull.pulling {
    transition: none;
  }

  /* The arrow flips once the pull is deep enough to fire on release. */
  .pull-arrow {
    flex-shrink: 0;
    transition: transform 0.15s ease;
  }

  .pull-arrow.armed {
    transform: rotate(180deg);
  }

  .pull-spinner {
    flex-shrink: 0;
    width: 11px;
    height: 11px;
    border: 1.5px solid var(--hairline);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: pull-spin 0.7s linear infinite;
  }

  @keyframes pull-spin {
    to {
      transform: rotate(360deg);
    }
  }

  /* Sticky date-section header: a filled full-width band, Apple Mail
     style. The background must stay opaque (--bg-sidebar, not a
     translucent tint) or rows would show through while scrolling under
     it. z-index lifts it above the rows' relative-positioned buttons,
     which would otherwise paint over it in DOM order. */
  .section {
    position: sticky;
    top: 0;
    z-index: 1;
    flex-shrink: 0;
    padding: 5px 16px 4px;
    border-bottom: 1px solid var(--hairline);
    background: var(--bg-sidebar);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--text-secondary);
  }

  /* Backfill progress: a quiet strip pinned under the list, gone once the
     mailbox is fully mirrored. */
  .syncing {
    flex-shrink: 0;
    padding: 6px 16px 8px;
    border-top: 1px solid var(--hairline);
    font-size: 11px;
    color: var(--text-secondary);
  }

  .syncing-row {
    display: flex;
    justify-content: space-between;
    gap: 8px;
  }

  .syncing-bar {
    display: block;
    margin-top: 5px;
    height: 2px;
    border-radius: 1px;
    background: var(--hairline);
    overflow: hidden;
  }

  .syncing-fill {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.3s ease;
  }

  /* Each row: an absolutely-positioned action backdrop underneath, the row
     button sliding over it. Clip so a mid-swipe row can't poke into its
     neighbours. */
  .swipe-row {
    position: relative;
    overflow: hidden;
    /* why: overflow != visible drops a flex item's automatic minimum size
       to 0, so rows would squash vertically to fit the pane instead of
       scrolling — pin them at their content height. */
    flex-shrink: 0;
    /* Flat full-width separators between rows (the new design). */
    border-bottom: 1px solid var(--hairline);
    /* why pan-y: on touch, vertical drags keep scrolling the list while
       horizontal ones reach the pointer handlers as a swipe. */
    touch-action: pan-y;
  }

  .swipe-bg {
    /* why border-box: width comes in as the revealed offset in px — the
       padding must eat into it, not widen the strip past the reveal. */
    box-sizing: border-box;
    position: absolute;
    top: 0;
    bottom: 0;
    display: flex;
    align-items: center;
    padding: 0 14px;
    font-size: 12px;
    font-weight: 600;
    color: #ffffff;
    /* The label pins to the outer edge and clips while the strip is still
       narrow, instead of overflowing into the row. */
    overflow: hidden;
    white-space: nowrap;
  }

  /* Anchoring by direction; the action's color arrives as an inline style. */
  .swipe-bg.trailing {
    right: 0;
    justify-content: flex-end;
  }

  .swipe-bg.leading {
    left: 0;
    justify-content: flex-start;
  }

  .swipe-row button {
    /* why relative: keeps the button painting above the positioned swipe
       backdrop. */
    position: relative;
    display: flex;
    /* why flex-start: the monogram lines up with the sender row rather than
       floating between it and the subject. Every row is now those same two
       lines — a fixed height the list can later virtualize by arithmetic. */
    align-items: flex-start;
    gap: 10px;
    box-sizing: border-box;
    width: 100%;
    padding: 10px 16px;
    border: none;
    background: var(--bg-window);
    font: inherit;
    color: inherit;
    text-align: left;
    cursor: default;
    /* Snap-back after release; .swiping turns it off so the row follows
       the fingers with no lag. */
    transition: transform 0.18s ease;
    /* why: keyboard navigation scrollIntoViews the selected row — leave
       room for the sticky section header (~23px) it would hide under. */
    scroll-margin-top: 24px;
  }

  .swipe-row button.swiping {
    transition: none;
  }

  /* Hover on any row that isn't the selected one — the selected row keeps
     its accent fill. */
  button:not(.selected):hover {
    background: var(--bg-hover);
  }

  button.selected {
    background: var(--accent);
    color: var(--on-accent);
  }

  /* Unread marker, inline before the sender name. */
  .dot {
    flex-shrink: 0;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
  }

  .selected .dot {
    background: var(--on-accent);
  }

  /* Sender monogram, tinted per domain (see lib/avatar.ts). */
  .avatar {
    display: grid;
    place-items: center;
    flex-shrink: 0;
    /* Nudged down so the circle centers on the sender row rather than on the
       row's ascender. */
    margin-top: 1px;
    width: 30px;
    height: 30px;
    border-radius: 50%;
    background: var(--avatar-muted);
    font-size: 12px;
    font-weight: 600;
    color: #ffffff;
    /* Clips a favicon that isn't square to the circle. */
    overflow: hidden;
  }

  /* A real icon replaces the tint: a colored disc behind a logo fights it.
     The hairline keeps a white favicon from dissolving into the row. */
  .avatar.has-icon {
    background: var(--bg-card);
    box-shadow: inset 0 0 0 1px var(--hairline);
  }

  .avatar img {
    width: 18px;
    height: 18px;
    /* contain, not cover: a logo cropped to a circle stops being the logo. */
    object-fit: contain;
  }

  .content {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .from {
    flex: 1;
    overflow: hidden;
    font-weight: 700;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Right end of the top row: a small account-color dot next to the date. */
  .end {
    display: flex;
    align-items: center;
    gap: 5px;
    flex-shrink: 0;
  }

  .account-dot {
    width: 7px;
    height: 7px;
    flex-shrink: 0;
    border-radius: 50%;
  }

  /* Same amber family as the conversation's Draft badge. */
  .draft-pill {
    flex-shrink: 0;
    padding: 0 6px;
    border-radius: 8px;
    background: rgba(178, 134, 14, 0.12);
    font-size: 10px;
    font-weight: 600;
    color: var(--warning);
  }

  /* On the accent-filled selected row the amber pill would vanish —
     switch to the row's own text color on a translucent chip. */
  .selected .draft-pill {
    background: rgb(255 255 255 / 20%);
    color: var(--on-accent);
  }

  .end .clip {
    flex-shrink: 0;
    color: var(--text-secondary);
  }

  .selected .end .clip {
    color: var(--on-accent);
  }

  /* Conversation size badge, e.g. "3" — a quiet outline, per the design. */
  .thread-count {
    flex-shrink: 0;
    padding: 0 5px;
    border: 1px solid currentColor;
    border-radius: 9px;
    font-size: 10px;
    font-weight: 600;
    line-height: 14px;
    color: var(--text-secondary);
  }

  .selected .thread-count {
    color: var(--on-accent);
  }

  .date {
    flex-shrink: 0;
    font-size: 11px;
    color: var(--text-secondary);
  }

  .selected .date {
    color: var(--on-accent);
    opacity: 0.85;
  }

  .subject {
    overflow: hidden;
    font-size: 12.5px;
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

</style>
