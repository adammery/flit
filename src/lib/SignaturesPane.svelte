<script lang="ts">
  import { onMount } from "svelte";
  import {
    createSignature,
    deleteSignature,
    listSignatures,
    onSignaturesChanged,
    setSignatureAccounts,
    updateSignature,
  } from "./api";
  import type { Account, Signature } from "./types";
  import RichTextEditor from "./RichTextEditor.svelte";

  let { accounts }: { accounts: Account[] } = $props();

  let signatures = $state<Signature[]>([]);
  let selectedId = $state<number | null>(null);
  let name = $state("");
  let bodyHtml = $state("");
  let error = $state<string | null>(null);

  const selected = $derived(
    signatures.find((sig) => sig.id === selectedId) ?? null,
  );
  /** Accounts currently defaulting to the selected signature. */
  const defaultFor = $derived(
    new Set(
      accounts
        .filter((account) => account.signatureId === selectedId)
        .map((account) => account.id),
    ),
  );
  const allChecked = $derived(
    accounts.length > 0 && accounts.every((a) => defaultFor.has(a.id)),
  );

  function select(sig: Signature) {
    selectedId = sig.id;
    name = sig.name;
    // bodyHtml re-syncs from the remounted editor ({#key selectedId}).
  }

  async function refresh(keepSelection = true) {
    signatures = await listSignatures();
    const current = keepSelection
      ? signatures.find((sig) => sig.id === selectedId)
      : undefined;
    if (current) {
      // why: another window may have renamed it — refresh the form fields
      // only when the user isn't mid-edit on a different row.
      return;
    }
    if (signatures.length > 0) select(signatures[0]);
    else selectedId = null;
  }

  async function add() {
    error = null;
    try {
      const created = await createSignature("Signature");
      signatures = await listSignatures();
      select(created);
    } catch (err) {
      error = String(err);
    }
  }

  async function remove() {
    if (selectedId === null) return;
    error = null;
    try {
      await deleteSignature(selectedId);
      selectedId = null;
      await refresh(false);
    } catch (err) {
      error = String(err);
    }
  }

  async function save() {
    if (selectedId === null) return;
    error = null;
    try {
      await updateSignature(selectedId, name, bodyHtml);
      signatures = signatures.map((sig) =>
        sig.id === selectedId ? { ...sig, name, body: bodyHtml } : sig,
      );
    } catch (err) {
      error = String(err);
    }
  }

  /** Toggle one account's membership in the selected signature's defaults. */
  async function toggleAccount(accountId: number) {
    if (selectedId === null) return;
    const next = new Set(defaultFor);
    if (next.has(accountId)) next.delete(accountId);
    else next.add(accountId);
    await applyDefaults([...next]);
  }

  async function toggleAll() {
    await applyDefaults(allChecked ? [] : accounts.map((a) => a.id));
  }

  async function applyDefaults(accountIds: number[]) {
    if (selectedId === null) return;
    error = null;
    try {
      // The backend broadcasts accounts-changed; the parent refetches and
      // the checkboxes re-render from the new account rows.
      await setSignatureAccounts(selectedId, accountIds);
    } catch (err) {
      error = String(err);
    }
  }

  onMount(() => {
    void refresh(false);
    const unlisten = onSignaturesChanged(() => void refresh());
    return () => {
      void unlisten.then((stop) => stop());
    };
  });
</script>

<section class="pane">
  <aside class="sidebar">
    <ul class="list" aria-label="Signatures">
      {#each signatures as sig (sig.id)}
        <li>
          <button
            class="row"
            class:selected={sig.id === selectedId}
            onclick={() => select(sig)}
          >
            {sig.name}
          </button>
        </li>
      {/each}
    </ul>
    <div class="list-actions">
      <button aria-label="Add signature" onclick={add}>+</button>
      <button
        aria-label="Delete signature"
        disabled={selectedId === null}
        onclick={remove}
      >
        −
      </button>
    </div>
  </aside>

  <div class="detail">
    {#if selected}
      <div class="editor-card">
        <input
          class="name"
          aria-label="Signature name"
          bind:value={name}
          placeholder="Signature name"
        />
        {#key selectedId}
          <RichTextEditor
            initialHtml={selected.body}
            bind:html={bodyHtml}
            ariaLabel="Signature body"
          />
        {/key}
        <div class="save-row">
          <button class="save" onclick={save}>Save</button>
        </div>
      </div>

      <fieldset class="defaults">
        <legend>Use this signature as default for:</legend>
        <label class="choice">
          <input type="checkbox" checked={allChecked} onchange={toggleAll} />
          All
        </label>
        {#each accounts as account (account.id)}
          <label class="choice">
            <input
              type="checkbox"
              checked={defaultFor.has(account.id)}
              onchange={() => void toggleAccount(account.id)}
            />
            {account.email}
          </label>
        {/each}
      </fieldset>
    {:else}
      <p class="empty">
        No signatures yet. Click + to create one, then pick it while writing
        a message.
      </p>
    {/if}

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}
  </div>
</section>

<style>
  .pane {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  /* Same master column as the accounts tab. */
  .sidebar {
    display: flex;
    flex-direction: column;
    width: 200px;
    flex-shrink: 0;
    border-right: 1px solid var(--hairline);
  }

  .list {
    flex: 1;
    margin: 0;
    padding: 12px 8px;
    overflow-y: auto;
    overflow-x: hidden;
    list-style: none;
  }

  .row {
    display: block;
    width: 100%;
    padding: 8px 10px;
    border: none;
    border-radius: 7px;
    background: none;
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    text-align: left;
    color: var(--text-primary);
    cursor: default;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row:hover:not(.selected) {
    background: var(--bg-hover);
  }

  .row.selected {
    background: var(--accent);
    color: var(--on-accent);
  }

  .list-actions {
    display: flex;
    gap: 4px;
    padding: 4px 10px 10px;
  }

  .list-actions button {
    width: 26px;
    height: 24px;
    border: 1px solid var(--border-chrome);
    border-radius: 6px;
    background: var(--bg-window);
    font: inherit;
    font-size: 14px;
    line-height: 1;
    color: var(--text-primary);
    cursor: default;
  }

  .list-actions button:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .list-actions button:disabled {
    opacity: 0.4;
  }

  .detail {
    display: flex;
    flex-direction: column;
    gap: 12px;
    flex: 1;
    min-width: 0;
    padding: 18px 22px;
    overflow-y: auto;
    overflow-x: hidden;
  }

  /* White editor box on the grouped-gray canvas. */
  .editor-card {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    border: 1px solid var(--hairline);
    border-radius: 8px;
    background: var(--bg-window);
    overflow: hidden;
  }

  .name {
    flex-shrink: 0;
    margin: 0 20px;
    padding: 10px 0;
    border: none;
    border-bottom: 1px solid var(--hairline);
    background: transparent;
    font: inherit;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .name:focus {
    outline: none;
  }

  .save-row {
    display: flex;
    justify-content: flex-end;
    flex-shrink: 0;
    padding: 8px 12px;
    border-top: 1px solid var(--hairline);
  }

  .save {
    padding: 6px 18px;
    border: none;
    border-radius: 7px;
    background: var(--accent);
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--on-accent);
    cursor: default;
  }

  .save:hover {
    filter: brightness(1.08);
  }

  .defaults {
    flex-shrink: 0;
    margin: 0;
    padding: 10px 14px;
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
    align-items: center;
    gap: 8px;
    padding: 3px 0;
    font-size: 13px;
    cursor: pointer;
  }

  .empty {
    margin: auto;
    max-width: 26rem;
    font-size: 13px;
    text-align: center;
    color: var(--text-secondary);
  }

  .error {
    flex-shrink: 0;
    margin: 0;
    font-size: 12px;
    color: var(--danger);
  }
</style>
