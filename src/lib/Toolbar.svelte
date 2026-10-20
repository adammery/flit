<script lang="ts">
  import type { DraftKind } from "./draft";
  import { formatCombo } from "./shortcuts";
  import type {
    Mailbox,
    MessageHeader,
    ShortcutAction,
    ShortcutBinding,
  } from "./types";

  let {
    sidebarCollapsed = false,
    sidebarWidth = 230,
    onToggleSidebar,
    onRefresh,
    refreshing = false,
    onCompose,
    onSearch,
    onOpenSettings,
    selected = null,
    selection = [],
    archived = false,
    moveTargets = [],
    onDraft,
    onSetRead,
    onArchive,
    onTrash,
    onMove,
    shortcuts = [],
  }: {
    sidebarCollapsed?: boolean;
    /** Current sidebar pane width — the traffic-light zone tracks it. */
    sidebarWidth?: number;
    onToggleSidebar: () => void;
    onRefresh: () => void;
    /** A sync is in flight — the refresh icon spins and the button locks. */
    refreshing?: boolean;
    onCompose: () => void;
    onSearch: (query: string) => void;
    onOpenSettings: () => void;
    /** The open message the action buttons operate on; null disables them. */
    selected?: MessageHeader | null;
    /** Every selected row, when the list holds more than one. Empty falls
     * back to `selected`, so a single selection behaves as it always did. */
    selection?: MessageHeader[];
    /** The selection already sits in its archive folder — the archive
     * action flips to "Move to Inbox" (onArchive still fires). */
    archived?: boolean;
    /** Move to targets — the selection's folders minus its current one. */
    moveTargets?: Mailbox[];
    onDraft?: (kind: DraftKind) => void;
    onSetRead?: (id: number, read: boolean) => void;
    onArchive?: (id: number) => void;
    onTrash?: (id: number) => void;
    onMove?: (id: number, mailbox: string) => void;
    /** Current keyboard bindings — named in the buttons' tooltips. */
    shortcuts?: ShortcutBinding[];
  } = $props();

  /** A button's tooltip: its name plus the shortcut, when one is bound. */
  function tip(name: string, action: ShortcutAction): string {
    const combo = shortcuts.find((b) => b.action === action)?.combo;
    return combo ? `${name} (${formatCombo(combo)})` : name;
  }

  let query = $state("");
  let searchEl = $state<HTMLInputElement | null>(null);

  /** Move keyboard focus into the search field (the ⌘F shortcut). */
  export function focusSearch() {
    searchEl?.focus();
    searchEl?.select();
  }

  // Collapsed, the zone keeps just enough room for the native traffic
  // lights plus the toggle button.
  let zoneWidth = $derived(sidebarCollapsed ? 130 : sidebarWidth);

  // What the action buttons operate on. The id still travels with each
  // callback — it is the row the click started from; the parent decides
  // whether the action covers the rest of the selection.
  let rows = $derived(
    selection.length > 0 ? selection : selected ? [selected] : [],
  );
  let allRead = $derived(rows.every((row) => row.read));

  let readTitle = $derived(allRead ? "Mark Unread" : "Mark Read");
  // why: the label names the action the click takes, like the title — a
  // fixed "Unread" read wrong once the open message was already unread.
  let readLabel = $derived(allRead ? "Unread" : "Read");
  let archiveTitle = $derived(archived ? "Move to Inbox" : "Archive");

  let moveOpen = $state(false);
  let moveEl = $state<HTMLElement | null>(null);

  function closeMoveOnOutsideClick(event: MouseEvent) {
    if (moveOpen && moveEl && !moveEl.contains(event.target as Node)) {
      moveOpen = false;
    }
  }
</script>

<svelte:window
  onmousedown={closeMoveOnOutsideClick}
  onkeydown={(e) => e.key === "Escape" && (moveOpen = false)}
/>

<!-- why: with titleBarStyle Overlay there is no native title bar left to grab,
     so the toolbar itself is the window drag handle. -->
<div class="toolbar" data-tauri-drag-region>
  <!-- Native traffic lights float over this zone's left end
       (tauri.conf.json trafficLightPosition); the toggle sits at its
       right edge, next to the sidebar it controls. -->
  <div
    class="sidebar-zone"
    style:width={`${zoneWidth}px`}
    data-tauri-drag-region
  >
    <button
      class="icon-btn"
      aria-label="Toggle sidebar"
      title={tip("Toggle sidebar", "toggle-sidebar")}
      onclick={onToggleSidebar}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true">
        <rect x="2.5" y="3.5" width="15" height="13" rx="2.5" />
        <line x1="7.5" y1="3.5" x2="7.5" y2="16.5" />
      </svg>
    </button>
  </div>

  <div class="actions" data-tauri-drag-region>
    <button
      class="action"
      aria-label="Check for new mail"
      title={tip("Check for new mail", "check-mail")}
      disabled={refreshing}
      onclick={onRefresh}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true" class:spinning={refreshing}>
        <path d="M16.5 10a6.5 6.5 0 1 1-1.9-4.6" />
        <path d="M16.8 2.8v3.4h-3.4" />
      </svg>
      <span class="label">Refresh</span>
    </button>
    <button
      class="action"
      aria-label="New Message"
      title={tip("New Message", "new-message")}
      onclick={onCompose}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true">
        <path d="M10 3.5H5a2 2 0 0 0-2 2V15a2 2 0 0 0 2 2h9.5a2 2 0 0 0 2-2v-5" />
        <path d="M15.8 2.7a1.6 1.6 0 0 1 2.3 2.3L11 12.1l-3 .7.7-3z" />
      </svg>
      <span class="label">New</span>
    </button>

    <div class="separator" aria-hidden="true"></div>

    <button
      class="action"
      aria-label="Reply"
      title={tip("Reply", "reply")}
      disabled={!selected}
      onclick={() => onDraft?.("reply")}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true">
        <path d="M8 4 3 8.5 8 13" />
        <path d="M3 8.5h8.5a5 5 0 0 1 5 5V16" />
      </svg>
      <span class="label">Reply</span>
    </button>
    <button
      class="action"
      aria-label="Reply All"
      title={tip("Reply All", "reply-all")}
      disabled={!selected}
      onclick={() => onDraft?.("reply-all")}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true">
        <path d="M7 4 2 8.5 7 13" />
        <path d="M11 4 6 8.5l5 4.5" />
        <path d="M6 8.5h7.5a4.5 4.5 0 0 1 4.5 4.5V16" />
      </svg>
      <span class="label">Reply All</span>
    </button>
    <button
      class="action"
      aria-label="Forward"
      title={tip("Forward", "forward")}
      disabled={!selected}
      onclick={() => onDraft?.("forward")}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true">
        <path d="m12 4 5 4.5-5 4.5" />
        <path d="M17 8.5H8.5a5 5 0 0 0-5 5V16" />
      </svg>
      <span class="label">Forward</span>
    </button>

    <div class="separator" aria-hidden="true"></div>

    <button
      class="action"
      aria-label={readTitle}
      title={tip(readTitle, "toggle-read")}
      disabled={rows.length === 0}
      onclick={() => rows.length > 0 && onSetRead?.(rows[0].id, !allRead)}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true">
        <rect x="2.5" y="4.5" width="15" height="11" rx="2" />
        <path d="m3 6 7 5 7-5" />
      </svg>
      <span class="label">{readLabel}</span>
    </button>
    <button
      class="action"
      aria-label={archiveTitle}
      title={tip(archiveTitle, "archive")}
      disabled={rows.length === 0}
      onclick={() => rows.length > 0 && onArchive?.(rows[0].id)}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true">
        <rect x="2.5" y="3.5" width="15" height="4" rx="1" />
        <path d="M4 7.5V15a1.5 1.5 0 0 0 1.5 1.5h9A1.5 1.5 0 0 0 16 15V7.5" />
        <line x1="8" y1="10.5" x2="12" y2="10.5" />
      </svg>
      <span class="label">{archived ? "Inbox" : "Archive"}</span>
    </button>
    <div class="move" bind:this={moveEl}>
      <button
        class="action"
        class:open={moveOpen}
        aria-label="Move to"
        title="Move to"
        aria-haspopup="menu"
        aria-expanded={moveOpen}
        disabled={rows.length === 0 || moveTargets.length === 0}
        onclick={() => (moveOpen = !moveOpen)}
      >
        <svg viewBox="0 0 20 20" aria-hidden="true">
          <path
            d="M2.5 5.5A1.5 1.5 0 0 1 4 4h4l1.5 2H16a1.5 1.5 0 0 1 1.5 1.5V14A1.5 1.5 0 0 1 16 15.5H4A1.5 1.5 0 0 1 2.5 14z"
          />
          <path d="M8 11h4m0 0-1.7-1.7M12 11l-1.7 1.7" />
        </svg>
        <span class="label">Move</span>
      </button>
      {#if moveOpen}
        <div class="menu" role="menu" aria-label="Move to folder">
          {#each moveTargets as folder (folder.id)}
            <button
              role="menuitem"
              onclick={() => {
                moveOpen = false;
                if (rows.length > 0) onMove?.(rows[0].id, folder.name);
              }}
            >
              {folder.displayName}
            </button>
          {/each}
        </div>
      {/if}
    </div>
    <button
      class="action"
      aria-label="Trash"
      title={tip("Trash", "trash")}
      disabled={rows.length === 0}
      onclick={() => rows.length > 0 && onTrash?.(rows[0].id)}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true">
        <path d="M3.5 5.5h13" />
        <path d="M8 5.5V4a1 1 0 0 1 1-1h2a1 1 0 0 1 1 1v1.5" />
        <path
          d="M5 5.5 5.8 15.6A1.5 1.5 0 0 0 7.3 17h5.4a1.5 1.5 0 0 0 1.5-1.4L15 5.5"
        />
      </svg>
      <span class="label">Trash</span>
    </button>
  </div>

  <div class="search-zone">
    <div class="field">
      <svg class="glass" viewBox="0 0 20 20" aria-hidden="true">
        <circle cx="9" cy="9" r="6" />
        <line x1="13.5" y1="13.5" x2="17.5" y2="17.5" />
      </svg>
      <input
        type="search"
        placeholder="Search"
        aria-label="Search messages"
        title="Narrow with from:… to:… subject:… is:unread"
        bind:this={searchEl}
        bind:value={query}
        oninput={() => onSearch(query)}
      />
    </div>
    <button
      class="icon-btn"
      aria-label="Settings"
      title="Settings"
      onclick={onOpenSettings}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true">
        <!-- An eight-tooth cog: flat tips on r 7.4, root arcs on r 5.5. -->
        <path
          stroke-linejoin="round"
          d="M8.58 4.69L8.72 2.71H11.28L11.42 4.69A5.5 5.5 0 0 1 12.75 5.24L14.24 3.94L16.06 5.76L14.76 7.25A5.5 5.5 0 0 1 15.31 8.58L17.29 8.72V11.28L15.31 11.42A5.5 5.5 0 0 1 14.76 12.75L16.06 14.24L14.24 16.06L12.75 14.76A5.5 5.5 0 0 1 11.42 15.31L11.28 17.29H8.72L8.58 15.31A5.5 5.5 0 0 1 7.25 14.76L5.76 16.06L3.94 14.24L5.24 12.75A5.5 5.5 0 0 1 4.69 11.42L2.71 11.28V8.72L4.69 8.58A5.5 5.5 0 0 1 5.24 7.25L3.94 5.76L5.76 3.94L7.25 5.24A5.5 5.5 0 0 1 8.58 4.69Z"
        />
        <circle cx="10" cy="10" r="2.3" />
      </svg>
    </button>
  </div>
</div>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    box-sizing: border-box;
    height: var(--toolbar-height);
    background: linear-gradient(
      var(--bg-toolbar-top),
      var(--bg-toolbar-bottom)
    );
    border-bottom: 1px solid var(--border-chrome);
  }

  .sidebar-zone {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    flex-shrink: 0;
    box-sizing: border-box;
    padding: 0 12px;
    transition: width 0.2s ease;
  }

  svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
  }

  .icon-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 34px;
    height: 28px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-secondary);
    cursor: default;
  }

  .icon-btn:hover {
    background: var(--bg-hover);
  }

  .icon-btn svg {
    width: 18px;
    height: 18px;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: 1;
    min-width: 0;
    padding: 0 10px;
    /* why clip-x only: overflow: hidden also clipped the Move dropdown, which
     * hangs below this row; clip the button row horizontally, keep y visible. */
    overflow-x: clip;
    overflow-y: visible;
  }

  .action {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 1px;
    flex-shrink: 0;
    min-width: 38px;
    height: 44px;
    padding: 0 8px;
    border: none;
    border-radius: 6px;
    background: transparent;
    font: inherit;
    color: var(--text-secondary);
    white-space: nowrap;
    cursor: default;
  }

  .action:hover:not(:disabled),
  .action.open {
    background: var(--bg-hover);
  }

  .action:disabled {
    opacity: 0.4;
  }

  .action svg {
    width: 18px;
    height: 18px;
  }

  .action .label {
    font-size: 10px;
  }

  /* why full opacity: the spinning state means "working", not "unavailable" —
   * the disabled dimming would read as the wrong signal. */
  .action:disabled:has(.spinning) {
    opacity: 1;
  }

  .action svg.spinning {
    animation: spin 0.9s linear infinite;
  }

  @media (prefers-reduced-motion: reduce) {
    .action svg.spinning {
      animation: none;
      opacity: 0.5;
    }
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .separator {
    flex-shrink: 0;
    width: 1px;
    height: 22px;
    margin: 0 8px;
    background: var(--border-chrome);
  }

  .move {
    position: relative;
    flex-shrink: 0;
  }

  .menu {
    position: absolute;
    top: calc(100% + 2px);
    left: 0;
    z-index: 20;
    display: flex;
    flex-direction: column;
    min-width: 170px;
    max-height: 60vh;
    padding: 4px;
    border: 1px solid var(--border-chrome);
    border-radius: 8px;
    background: var(--bg-window);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.15);
    overflow-y: auto;
  }

  .menu button {
    padding: 6px 10px;
    border: none;
    border-radius: 5px;
    background: none;
    font: inherit;
    color: var(--text-primary);
    text-align: left;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    cursor: default;
  }

  .menu button:hover {
    background: var(--accent);
    color: var(--on-accent);
  }

  .search-zone {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: 0 1 322px;
    min-width: 170px;
    box-sizing: border-box;
    padding: 0 14px;
  }

  .field {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
    min-width: 0;
    padding: 5px 9px;
    border-radius: 7px;
    background: var(--bg-field);
  }

  .field .glass {
    flex-shrink: 0;
    width: 14px;
    height: 14px;
    stroke-width: 1.8;
    color: var(--text-tertiary);
  }

  .field input {
    width: 100%;
    padding: 0;
    border: none;
    background: transparent;
    outline: none;
    font: inherit;
    color: var(--text-primary);
  }
</style>
