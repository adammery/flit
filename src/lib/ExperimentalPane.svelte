<script lang="ts">
  import { onMount } from "svelte";
  import {
    cancelLlmDownload,
    downloadLlmModel,
    getLlmSummaryEnabled,
    getLlmSummaryLanguage,
    getLlmTranslateEnabled,
    getLlmTranslateLanguage,
    llmStatus,
    onLlmDownloadProgress,
    onLlmModelsChanged,
    removeLlmModel,
    setLlmModel,
    setLlmSummaryEnabled,
    setLlmSummaryLanguage,
    setLlmTranslateEnabled,
    setLlmTranslateLanguage,
  } from "./api";
  import { formatFileSize } from "./format";
  import { LANGUAGES } from "./languages";
  import type { LlmStatus } from "./types";

  let enabled = $state(false);
  let translateEnabled = $state(false);
  let status = $state<LlmStatus | null>(null);
  let language = $state("auto");
  let translateLanguage = $state("English");
  let error = $state<string | null>(null);

  async function refresh() {
    status = await llmStatus();
  }

  async function toggle(next: boolean) {
    error = null;
    const previous = enabled;
    enabled = next;
    try {
      await setLlmSummaryEnabled(next);
      await refresh();
    } catch (err) {
      // why: the box must not lie — a failed save rolls the value back.
      enabled = previous;
      error = String(err);
    }
  }

  async function toggleTranslate(next: boolean) {
    error = null;
    const previous = translateEnabled;
    translateEnabled = next;
    try {
      await setLlmTranslateEnabled(next);
      await refresh();
    } catch (err) {
      translateEnabled = previous;
      error = String(err);
    }
  }

  /** Run a backend action, then re-read status so the cards catch up. */
  async function act(action: () => Promise<void>) {
    error = null;
    try {
      await action();
      await refresh();
    } catch (err) {
      error = String(err);
    }
  }

  async function pickLanguage(next: string) {
    error = null;
    const previous = language;
    language = next;
    try {
      await setLlmSummaryLanguage(next);
    } catch (err) {
      language = previous;
      error = String(err);
    }
  }

  async function pickTranslateLanguage(next: string) {
    error = null;
    const previous = translateLanguage;
    translateLanguage = next;
    try {
      await setLlmTranslateLanguage(next);
    } catch (err) {
      translateLanguage = previous;
      error = String(err);
    }
  }

  onMount(() => {
    void getLlmSummaryEnabled().then((stored) => (enabled = stored));
    void getLlmTranslateEnabled().then((stored) => (translateEnabled = stored));
    void getLlmSummaryLanguage().then((stored) => (language = stored));
    void getLlmTranslateLanguage().then((stored) => (translateLanguage = stored));
    void refresh();
    // why patch in place: progress ticks several times a second and the
    // status call walks the models directory — re-reading it per tick
    // would be wasteful for a bar that only needs the new byte count.
    const unlistenProgress = onLlmDownloadProgress((progress) => {
      const model = status?.models.find((m) => m.id === progress.id);
      if (model && model.state.kind === "downloading") {
        model.state = { ...progress, kind: "downloading" };
      }
    });
    const unlistenChanged = onLlmModelsChanged(() => void refresh());
    return () => {
      void unlistenProgress.then((stop) => stop());
      void unlistenChanged.then((stop) => stop());
    };
  });
</script>

<section class="pane">
  <fieldset>
    <legend>On-device AI</legend>
    <div class="choice">
      <input
        type="checkbox"
        id="llm-summary"
        checked={enabled}
        onchange={(e) => void toggle(e.currentTarget.checked)}
      />
      <span>
        <label for="llm-summary">Summarize messages with a local AI model</label>
        <small>Adds a summarize button to messages and threads.</small>
      </span>
    </div>
    <div class="choice">
      <input
        type="checkbox"
        id="llm-translate"
        checked={translateEnabled}
        onchange={(e) => void toggleTranslate(e.currentTarget.checked)}
      />
      <span>
        <label for="llm-translate">Translate messages with a local AI model</label>
        <small>Adds a translate button to messages.</small>
      </span>
    </div>
    <p class="explain">
      Experimental. Summaries and translations are written by a small language
      model that runs entirely on this Mac — no message ever leaves it. The
      model is a separate download you pick and start yourself; switching
      these on downloads nothing by itself.
    </p>
  </fieldset>

  {#if (enabled || translateEnabled) && status}
    <fieldset>
      <legend>Model</legend>
      <ul class="models">
        {#each status.models as model (model.id)}
          {@const ready = model.state.kind === "ready"}
          <li class="model" class:active={status.activeModel === model.id}>
            <input
              type="radio"
              name="llm-model"
              id="llm-model-{model.id}"
              aria-label="Use {model.name}"
              checked={status.activeModel === model.id}
              disabled={!ready}
              onchange={() => void act(() => setLlmModel(model.id))}
            />
            <div class="body">
              <div class="title">
                <label for="llm-model-{model.id}">{model.name}</label>
                {#if model.recommended}
                  <span class="badge">Recommended</span>
                {/if}
                <span class="size">{formatFileSize(model.size)}</span>
              </div>
              <small>{model.description}</small>
              <div class="state">
                {#if model.state.kind === "downloading"}
                  <progress value={model.state.received} max={model.state.total}
                  ></progress>
                  <span class="bytes">
                    {formatFileSize(model.state.received)} of {formatFileSize(
                      model.state.total,
                    )}
                  </span>
                  <button
                    class="action"
                    onclick={() => void act(() => cancelLlmDownload(model.id))}
                  >
                    Cancel
                  </button>
                {:else if model.state.kind === "failed"}
                  <span class="failed">{model.state.error}</span>
                  <button
                    class="action"
                    onclick={() => void act(() => downloadLlmModel(model.id))}
                  >
                    Retry
                  </button>
                  <button
                    class="action"
                    onclick={() => void act(() => removeLlmModel(model.id))}
                  >
                    Remove
                  </button>
                {:else if ready}
                  <span class="ready">Downloaded</span>
                  <button
                    class="action"
                    onclick={() => void act(() => removeLlmModel(model.id))}
                  >
                    Remove
                  </button>
                {:else}
                  <span>Not downloaded</span>
                  <button
                    class="action"
                    onclick={() => void act(() => downloadLlmModel(model.id))}
                  >
                    Download
                  </button>
                {/if}
              </div>
            </div>
          </li>
        {/each}
      </ul>
      <p class="explain">
        Models are fetched once from huggingface.co over TLS, verified, and
        kept in Flit's data folder. Several can be downloaded to compare; the
        picked one writes the summaries and translations.
      </p>
    </fieldset>

    {#if enabled}
      <fieldset>
        <legend>Summaries</legend>
        <div class="setting">
          <label for="llm-language">Summary language</label>
          <select
            id="llm-language"
            value={language}
            onchange={(e) => void pickLanguage(e.currentTarget.value)}
          >
            <option value="auto">Same as the message</option>
            {#each LANGUAGES as name (name)}
              <option value={name}>{name}</option>
            {/each}
          </select>
        </div>
        <p class="explain">
          Small models follow the message's language well for common ones; pick
          yours here if a summary comes back in the wrong language.
        </p>
      </fieldset>

    {/if}

    {#if translateEnabled}
      <fieldset>
        <legend>Translation</legend>
        <div class="setting">
          <label for="llm-translate-language">Translate messages into</label>
          <select
            id="llm-translate-language"
            value={translateLanguage}
            onchange={(e) => void pickTranslateLanguage(e.currentTarget.value)}
          >
            {#each LANGUAGES as name (name)}
              <option value={name}>{name}</option>
            {/each}
          </select>
        </div>
        <p class="explain">
          A Translate button on every message rewrites its text in this
          language, in place — on this Mac, with the same model. Long
          newsletters take a minute or more.
        </p>
      </fieldset>
    {/if}
  {/if}

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}
</section>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    gap: 18px;
    padding: 20px 26px;
    overflow-y: auto;
    overflow-x: hidden;
  }

  /* White grouped boxes on the settings canvas, like the other tabs. */
  fieldset {
    margin: 0;
    max-width: 520px;
    padding: 8px 12px;
    border: 1px solid var(--hairline);
    border-radius: 9px;
    background: var(--bg-window);
  }

  legend {
    padding: 0 4px;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-secondary);
  }

  .choice {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 5px 0;
    font-size: 13px;
  }

  .choice input {
    margin-top: 2px;
  }

  .choice span {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .choice label {
    font-weight: 500;
  }

  .choice small,
  .model small {
    font-size: 11.5px;
    color: var(--text-secondary);
  }

  .explain {
    margin: 6px 0 2px;
    font-size: 11.5px;
    color: var(--text-secondary);
  }

  .setting {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 5px 0;
    font-size: 13px;
    font-weight: 500;
  }

  .setting label {
    width: 10rem;
  }

  select {
    min-width: 11rem;
    padding: 3px 8px;
    border: 1px solid var(--border-chrome);
    border-radius: 6px;
    background: var(--bg-window);
    font: inherit;
    font-size: 12.5px;
    font-weight: 400;
  }

  .models {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .model {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 8px 0;
    border-top: 1px solid var(--hairline);
    font-size: 13px;
  }

  .model:first-child {
    border-top: none;
  }

  .model input {
    margin-top: 3px;
  }

  .body {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .title {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }

  .title label {
    font-weight: 500;
  }

  .badge {
    padding: 1px 6px;
    border-radius: 999px;
    background: var(--bg-selected-muted);
    font-size: 10.5px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .size {
    margin-left: auto;
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    color: var(--text-secondary);
  }

  .state {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 4px;
    font-size: 11.5px;
    color: var(--text-secondary);
  }

  .ready {
    color: var(--text-primary);
  }

  .failed {
    color: var(--danger);
  }

  .bytes {
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  progress {
    flex: 1;
    height: 6px;
    accent-color: var(--accent);
  }

  .action {
    margin-left: auto;
    padding: 2px 10px;
    border: 1px solid var(--border-chrome);
    border-radius: 6px;
    background: var(--bg-window);
    font: inherit;
    font-size: 11.5px;
    font-weight: 500;
    color: var(--text-primary);
    cursor: default;
  }

  .action + .action {
    margin-left: 0;
  }

  .action:hover {
    background: var(--bg-hover);
  }

  .error {
    margin: 0;
    font-size: 12px;
    color: var(--danger);
  }
</style>
