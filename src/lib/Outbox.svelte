<script module lang="ts">
  /** One send in flight, mirrored from the backend's send-* events. */
  export interface OutboxEntry {
    id: number;
    subject: string;
    status: "sending" | "sent" | "failed";
    error: string | null;
    /** Undo window length — the countdown donut fills over exactly this. */
    undoMs: number;
  }
</script>

<script lang="ts">
  let {
    entries,
    onUndo,
  }: {
    entries: OutboxEntry[];
    onUndo: (id: number) => void;
  } = $props();

  function title(entry: OutboxEntry): string {
    const subject = entry.subject.trim();
    return subject === "" ? "(No subject)" : subject;
  }

  function label(entry: OutboxEntry): string {
    if (entry.status === "sending") return `Sending: ${title(entry)}`;
    if (entry.status === "sent") return `Sent: ${title(entry)}`;
    return `Couldn't send: ${title(entry)}`;
  }
</script>

{#if entries.length > 0}
  <!-- Floating badge stack at the bottom of the message list, one pill per
       send in flight — Gmail's "Sending… / Undo" pattern. -->
  <div class="outbox">
    {#each entries as entry (entry.id)}
      <div class="badge" class:failed={entry.status === "failed"} role="status">
        <span class="indicator" aria-hidden="true">
          {#if entry.status === "sending"}
            <!-- Countdown donut: the ring fills over the undo window, then
                 holds full (blue) while the SMTP delivery is in flight. -->
            <svg class="donut" viewBox="0 0 20 20">
              <circle class="track" cx="10" cy="10" r="8" />
              <circle
                class="fill"
                cx="10"
                cy="10"
                r="8"
                style="animation-duration: {entry.undoMs}ms"
              />
            </svg>
          {:else if entry.status === "sent"}
            <svg class="check" viewBox="0 0 20 20">
              <circle cx="10" cy="10" r="9" />
              <path d="M6 10.5l2.6 2.6L14 7.5" />
            </svg>
          {:else}
            <svg class="fail" viewBox="0 0 20 20">
              <circle cx="10" cy="10" r="9" />
              <path d="M7 7l6 6M13 7l-6 6" />
            </svg>
          {/if}
        </span>
        <span class="text">
          {label(entry)}
          {#if entry.status === "failed" && entry.error}
            <span class="detail">{entry.error}</span>
          {/if}
        </span>
        {#if entry.status === "sending"}
          <button type="button" onclick={() => onUndo(entry.id)}>Undo</button>
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  .outbox {
    position: absolute;
    bottom: 12px;
    left: 12px;
    right: 12px;
    z-index: 5;
    display: flex;
    flex-direction: column;
    gap: 6px;
    pointer-events: none;
  }

  .badge {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-radius: 10px;
    background: var(--bg-sidebar);
    box-shadow:
      0 0 0 1px var(--hairline),
      0 4px 16px rgba(0, 0, 0, 0.18);
    font-size: 12px;
    color: var(--text-primary);
    pointer-events: auto;
  }

  .indicator {
    flex-shrink: 0;
    width: 16px;
    height: 16px;
  }

  .indicator svg {
    display: block;
    width: 100%;
    height: 100%;
  }

  .donut .track {
    fill: none;
    stroke: var(--divider);
    stroke-width: 3;
  }

  .donut .fill {
    fill: none;
    stroke: var(--accent);
    stroke-width: 3;
    stroke-linecap: round;
    /* 2π·r for r=8 — the full ring; the animation walks the offset to 0. */
    stroke-dasharray: 50.27;
    stroke-dashoffset: 50.27;
    /* start at 12 o'clock, not 3 */
    transform: rotate(-90deg);
    transform-origin: center;
    animation: donut-fill linear forwards;
  }

  @keyframes donut-fill {
    to {
      stroke-dashoffset: 0;
    }
  }

  .check circle {
    fill: var(--success);
  }

  .check path {
    fill: none;
    stroke: var(--on-accent);
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .fail circle {
    fill: var(--danger);
  }

  .fail path {
    stroke: var(--on-accent);
    stroke-width: 2;
    stroke-linecap: round;
  }

  .text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .detail {
    margin-left: 6px;
    color: var(--text-secondary);
  }

  .badge.failed .text {
    color: var(--danger);
  }

  .badge.failed .detail {
    color: var(--text-secondary);
  }

  button {
    flex-shrink: 0;
    padding: 2px 8px;
    border: none;
    border-radius: 6px;
    background: transparent;
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    color: var(--accent);
    cursor: pointer;
  }

  button:hover {
    background: var(--bg-hover);
  }
</style>
