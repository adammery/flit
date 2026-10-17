<script lang="ts">
  // The on-device translation controls of a message, inline in the summary
  // row: ask (into the language picked in Settings → Experimental), watch
  // the count while the model works, then flip between the translation and
  // the original. The translated body itself renders in
  // the card's sandboxed frame — built by the backend like any other body.
  import type { TranslationState } from "./translations";

  let {
    translation,
    showing,
    onStart,
    onCancel,
    onToggle,
  }: {
    /** Null until a translation was asked for. */
    translation: TranslationState | null;
    /** Whether the card shows the translation (else the original). */
    showing: boolean;
    onStart: () => void;
    /** Stop a running translation, or close a finished one. */
    onCancel: () => void;
    onToggle: () => void;
  } = $props();
</script>

<span class="translate" role="group" aria-label="Translation">
  {#if translation === null || (!translation.loading && translation.body === null)}
    <button class="start" aria-label="Translate this message" onclick={onStart}>
      <svg
        width="13"
        height="13"
        viewBox="0 0 20 20"
        fill="none"
        stroke="currentColor"
        stroke-width="1.6"
        aria-hidden="true"
      >
        <circle cx="10" cy="10" r="7.5" />
        <path d="M2.5 10h15M10 2.5c2.2 2.4 2.2 12.6 0 15M10 2.5c-2.2 2.4-2.2 12.6 0 15" />
      </svg>
      Translate
    </button>
    {#if translation?.error}
      <span class="status error" role="alert">{translation.error}</span>
    {/if}
  {:else if translation.loading}
    <span class="title">Translating…</span>
    <span class="note">
      {translation.total > 0
        ? `${translation.done} of ${translation.total} parts`
        : "starting the model"}
    </span>
    <button
      class="btn"
      title="Cancel"
      aria-label="Cancel translation"
      onclick={onCancel}
    >
      ×
    </button>
  {:else}
    <span class="title">
      {showing ? "Translated" : "Original"}
    </span>
    <span class="note">· may be inaccurate</span>
    <button class="start" onclick={onToggle}>
      {showing ? "Show original" : "Show translation"}
    </button>
    <button
      class="btn"
      title="Close translation"
      aria-label="Close translation"
      onclick={onCancel}
    >
      ×
    </button>
  {/if}
</span>

<style>
  /* Styled like the summary row it sits in (SummaryPanel). */
  .translate {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .title {
    font-size: 10.5px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-secondary);
  }

  .note {
    font-size: 10.5px;
    color: var(--text-secondary);
    opacity: 0.8;
  }

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

  .btn {
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

  .btn:hover {
    opacity: 1;
    color: var(--text-primary);
  }

  .status.error {
    color: var(--danger);
  }
</style>
