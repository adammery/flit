<script lang="ts">
  // The on-device summary section of a message or conversation: a slim
  // row with a Summarize button until one exists, then the bullet lines
  // (plain text from the backend — never interpreted as HTML), foldable so
  // it stays out of the way once read.
  import type { Snippet } from "svelte";
  import { isCollapsed, setCollapsed, type SummaryState } from "./summaries";

  let {
    summary,
    label = "AI summary",
    startLabel = "Summarize this message",
    collapseKey,
    onStart,
    onRetry,
    onCancel,
    actions,
  }: {
    /** Null until a summary was asked for. */
    summary: SummaryState | null;
    /** Region name and heading. */
    label?: string;
    /** Accessible name of the button that asks for the first summary. */
    startLabel?: string;
    /** Identifies the message/conversation so a fold survives switching. */
    collapseKey: string;
    onStart: () => void;
    onRetry: () => void;
    /** Stop a summary still being written (the × while loading). */
    onCancel: () => void;
    /** More controls for the same row, after the summary's own. */
    actions?: Snippet;
  } = $props();

  let lines = $derived(
    (summary?.text ?? "")
      .split("\n")
      // A bullet marker goes; so do stray bold markers — the model is told
      // "no markdown", but a small one slips now and then.
      .map((line) => line.replace(/^\s*[-•*]\s*/, "").replace(/\*\*/g, "").trim())
      .filter((line) => line !== ""),
  );
  let hasText = $derived(summary !== null && !summary.loading && summary.text !== null);
  let collapsed = $state(false);
  $effect(() => {
    collapsed = isCollapsed(collapseKey);
  });

  function toggle() {
    collapsed = !collapsed;
    setCollapsed(collapseKey, collapsed);
  }
</script>

<div class="summary" class:empty={summary === null} role="region" aria-label={label}>
  <div class="summary-head">
    {#if summary === null}
      <button class="start" aria-label={startLabel} onclick={onStart}>
        <svg
          width="13"
          height="13"
          viewBox="0 0 20 20"
          fill="none"
          stroke="currentColor"
          stroke-width="1.6"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M10 2.5 11.8 8.2 17.5 10l-5.7 1.8L10 17.5l-1.8-5.7L2.5 10l5.7-1.8z" />
          <path d="M16 2v3M14.5 3.5h3" />
        </svg>
        Summarize
      </button>
    {:else}
      <span class="summary-title">{label}</span>
      <span class="summary-note">· local model, may be inaccurate</span>
      {#if summary.loading}
        <button
          class="summary-btn"
          title="Cancel"
          aria-label="Cancel summary"
          onclick={onCancel}
        >
          ×
        </button>
      {:else}
        <button
          class="summary-btn"
          title="Summarize again"
          aria-label="Summarize again"
          onclick={onRetry}
        >
          ↻
        </button>
        {#if hasText}
          <button
            class="summary-btn"
            title={collapsed ? "Show summary" : "Hide summary"}
            aria-label={collapsed ? "Show summary" : "Hide summary"}
            aria-expanded={!collapsed}
            onclick={toggle}
          >
            {collapsed ? "▸" : "▾"}
          </button>
        {/if}
      {/if}
    {/if}
    {#if actions}
      <span class="actions">{@render actions()}</span>
    {/if}
  </div>
  {#if summary === null || (hasText && collapsed)}
    <!-- nothing below the head -->
  {:else if summary.loading && lines.length === 0}
    <p class="summary-status">Summarizing…</p>
  {:else if summary.error}
    <p class="summary-status error" role="alert">{summary.error}</p>
  {:else}
    <ul class="summary-lines" class:writing={summary.loading}>
      {#each lines as line, i (i)}
        <li>{line}</li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  /* Deliberately quiet: no box, a hairline underneath, and the text sits
     in the same column as the mail — a note above the body, not a widget. */
  .summary {
    padding: 4px 0 8px;
    border-bottom: 1px solid var(--hairline);
    font-size: 12.5px;
  }

  .summary.empty {
    padding: 2px 0 4px;
  }

  .summary-head {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    min-height: 22px;
  }

  .actions {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-left: 6px;
  }

  .summary:not(.empty) .summary-head {
    margin-bottom: 2px;
  }

  .summary-title {
    font-size: 10.5px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-secondary);
  }

  .summary-note {
    flex: 1;
    font-size: 10.5px;
    color: var(--text-secondary);
    opacity: 0.8;
  }

  /* A text button, not a pill: the sparkle and one word. */
  .start {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    margin-left: -4px;
    padding: 2px 4px;
    border: none;
    border-radius: 5px;
    background: none;
    font: inherit;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
    cursor: default;
  }

  .start:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .summary-btn {
    padding: 0 3px;
    border: none;
    background: none;
    font: inherit;
    font-size: 13px;
    line-height: 1;
    color: var(--text-secondary);
    opacity: 0.7;
    cursor: default;
  }

  .summary-btn:hover:not(:disabled) {
    opacity: 1;
    color: var(--text-primary);
  }

  .summary-status {
    margin: 2px 0 0;
    color: var(--text-secondary);
  }

  .summary-status.error {
    color: var(--danger);
  }

  .summary-lines {
    margin: 2px 0 0;
    padding-left: 16px;
    color: var(--text-primary);
    /* why: the body opts chrome out of selection; the summary is content. */
    user-select: text;
  }

  .summary-lines li + li {
    margin-top: 2px;
  }

  /* A blinking caret after the last line while the model is still writing. */
  .summary-lines.writing li:last-child::after {
    content: "▍";
    margin-left: 2px;
    color: var(--text-secondary);
    animation: blink 1s steps(2) infinite;
  }

  @keyframes blink {
    to {
      visibility: hidden;
    }
  }
</style>
