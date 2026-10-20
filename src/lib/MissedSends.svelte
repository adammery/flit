<script lang="ts">
  import type { ScheduledMessage } from "./types";

  let {
    entries,
    onSendNow,
    onOpenDraft,
    onDismiss,
  }: {
    entries: ScheduledMessage[];
    onSendNow: (id: number) => void;
    onOpenDraft: (id: number) => void;
    onDismiss: () => void;
  } = $props();

  function title(entry: ScheduledMessage): string {
    const subject = entry.subject.trim();
    return subject === "" ? "(No subject)" : subject;
  }

  let dialog = $state<HTMLElement>();

  // why on mount only: focus lands once when the dialog appears; re-running it
  // as entries are handled would yank focus back to the top of the list.
  $effect(() => {
    dialog?.querySelector("button")?.focus();
  });

  function onKeydown(event: KeyboardEvent) {
    if (entries.length === 0 || event.key !== "Escape") return;
    event.preventDefault();
    onDismiss();
  }

  function formatWhen(epochSeconds: number): string {
    return new Date(epochSeconds * 1000).toLocaleString(undefined, {
      weekday: "short",
      day: "numeric",
      month: "short",
      hour: "2-digit",
      minute: "2-digit",
    });
  }
</script>

<svelte:window onkeydown={onKeydown} />

{#if entries.length > 0}
  <!-- Scheduled sends whose time passed while the app was off (or asleep).
       Nothing here was sent — the user decides per message. alertdialog:
       it interrupts on purpose; stale mail leaving unasked would be worse.
       Escape means "Decide later", the choice that sends nothing. -->
  <div class="backdrop">
    <div
      class="dialog"
      role="alertdialog"
      aria-modal="true"
      aria-label="Missed scheduled messages"
      bind:this={dialog}
    >
      <h2>Missed scheduled messages</h2>
      <p class="hint">
        These were due while Flit wasn't running. Nothing has been sent.
      </p>
      <ul>
        {#each entries as entry (entry.id)}
          <li>
            <div class="meta">
              <span class="subject">{title(entry)}</span>
              <span class="detail">
                To {entry.to} · was due {formatWhen(entry.scheduledAt)}
              </span>
            </div>
            <button
              type="button"
              class="draft"
              aria-label={`Open ${title(entry)} as draft`}
              onclick={() => onOpenDraft(entry.id)}
            >
              Open as draft
            </button>
            <button
              type="button"
              class="send"
              aria-label={`Send ${title(entry)} now`}
              onclick={() => onSendNow(entry.id)}
            >
              Send now
            </button>
          </li>
        {/each}
      </ul>
      <button type="button" class="dismiss" onclick={onDismiss}>
        Decide later
      </button>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 40;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.32);
  }

  .dialog {
    width: min(520px, calc(100vw - 48px));
    max-height: 70vh;
    overflow-y: auto;
    padding: 20px;
    border: 1px solid var(--hairline);
    border-radius: 14px;
    background: var(--bg-window);
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.28);
  }

  h2 {
    margin: 0 0 4px;
    font-size: 15px;
    font-weight: 700;
    color: var(--text-primary);
  }

  .hint {
    margin: 0 0 14px;
    font-size: 12px;
    color: var(--text-secondary);
  }

  ul {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border: 1px solid var(--hairline);
    border-radius: 10px;
    background: var(--bg-hover);
  }

  .meta {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .subject {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .detail {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--text-secondary);
  }

  button {
    flex-shrink: 0;
    padding: 5px 10px;
    border: 1px solid var(--hairline);
    border-radius: 7px;
    background: transparent;
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-primary);
    cursor: pointer;
  }

  button:hover {
    background: var(--bg-hover);
  }

  .send {
    border-color: transparent;
    background: var(--accent);
    color: var(--on-accent);
  }

  .send:hover {
    background: color-mix(in srgb, var(--accent) 85%, #000000);
  }

  .dismiss {
    margin-top: 14px;
    border: none;
    color: var(--text-secondary);
  }

  .dismiss:hover {
    color: var(--text-primary);
    background: transparent;
  }
</style>
