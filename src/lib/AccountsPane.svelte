<script lang="ts">
  import { ACCOUNT_COLORS } from "./accountColors";
  import type { Account, Alias, AuthKind, NewAccount } from "./types";
  import AddAccountForm from "./AddAccountForm.svelte";

  let {
    accounts,
    aliases,
    oauthProviders = [],
    onAdd,
    onAddOAuth,
    onCancelSignIn,
    onReconnect,
    onDelete,
    onSetColor,
    onAddAlias,
    onUpdateAlias,
    onDeleteAlias,
    onSetDefaultAlias,
  }: {
    accounts: Account[];
    /** All aliases across accounts; the pane filters by selection. */
    aliases: Alias[];
    // why: resolves to the created account on success, null on failure — the
    // parent owns the api call and error display; the pane only needs to know
    // whether to leave the add form.
    onAdd: (account: NewAccount, password: string) => Promise<Account | null>;
    /** The OAuth providers this build offers; none hides their buttons. */
    oauthProviders?: AuthKind[];
    // why: same success contract as onAdd — null keeps the form open.
    onAddOAuth?: (provider: AuthKind, name: string) => Promise<Account | null>;
    onCancelSignIn?: () => void;
    /** Sign an OAuth account in again; resolves when the attempt ended. */
    onReconnect?: (account: Account) => Promise<void>;
    // why: hands over the whole account — the parent's confirmation dialog
    // needs the name and email, not just the id.
    onDelete: (account: Account) => void;
    // why: `null` clears the color; the parent owns the api call + refresh.
    onSetColor: (id: number, color: string | null) => void;
    // why: same success contract as onAdd — null keeps the inline row open.
    onAddAlias: (
      accountId: number,
      name: string,
      email: string,
    ) => Promise<Alias | null>;
    onUpdateAlias: (id: number, name: string, email: string) => void;
    onDeleteAlias: (id: number) => void;
    /** `null` = back to the account's own address. */
    onSetDefaultAlias: (accountId: number, aliasId: number | null) => void;
  } = $props();

  let selectedId = $state<number | null>(null);
  let adding = $state(false);
  let addingAlias = $state(false);
  let reconnecting = $state(false);
  let aliasName = $state("");
  let aliasEmail = $state("");

  // why: falls back to the first account when nothing was selected yet or the
  // selected account was just deleted from the list.
  let selected = $derived(
    accounts.find((a) => a.id === selectedId) ?? accounts[0] ?? null,
  );
  let selectedAliases = $derived(
    aliases.filter((a) => a.accountId === selected?.id),
  );

  async function handleSubmit(account: NewAccount, password: string) {
    const created = await onAdd(account, password);
    if (created) {
      selectedId = created.id;
      adding = false;
    }
  }

  async function signInWithGoogle(name: string) {
    const created = await onAddOAuth?.("google", name);
    if (created) {
      selectedId = created.id;
      adding = false;
    }
  }

  async function reconnect(account: Account) {
    reconnecting = true;
    try {
      await onReconnect?.(account);
    } finally {
      reconnecting = false;
    }
  }

  async function submitAlias(accountId: number) {
    const created = await onAddAlias(accountId, aliasName, aliasEmail);
    if (created) addingAlias = false;
  }
</script>

<div class="pane">
  <aside>
    <ul>
      {#each accounts as account (account.id)}
        <li>
          <button
            class="account"
            class:active={!adding && selected?.id === account.id}
            onclick={() => {
              selectedId = account.id;
              adding = false;
            }}
          >
            <span
              class="dot"
              aria-hidden="true"
              style:background={account.color ?? undefined}
            ></span>
            <span class="text">
              <span class="name">{account.name}</span>
              <span class="email">{account.email}</span>
            </span>
            {#if account.lastError}
              <span class="warning" title="Connection problem"></span>
            {/if}
          </button>
        </li>
      {/each}
    </ul>
    <div class="actions">
      <button aria-label="Add account" onclick={() => (adding = true)}>
        +
      </button>
      <button
        aria-label="Delete account"
        disabled={adding || selected === null}
        onclick={() => selected && onDelete(selected)}
      >
        −
      </button>
    </div>
  </aside>

  <div class="detail">
    {#if adding}
      <AddAccountForm
        onSubmit={handleSubmit}
        onCancel={() => (adding = false)}
        onSignInWithGoogle={oauthProviders.includes("google")
          ? signInWithGoogle
          : undefined}
        {onCancelSignIn}
      />
    {:else if selected}
      <dl>
        <dt>Status</dt>
        <dd>
          {#if selected.lastError}
            <span class="status broken">{selected.lastError}</span>
            <!-- why any error, not just an expired sign-in: signing in
                 again is harmless, and it is the one fix an OAuth account
                 has — there is no password to correct. -->
            {#if selected.auth !== "password" && onReconnect}
              <button
                class="reconnect"
                disabled={reconnecting}
                onclick={() => reconnect(selected)}
              >
                {reconnecting ? "Waiting for the browser…" : "Sign in again"}
              </button>
              {#if reconnecting}
                <button class="link" onclick={() => onCancelSignIn?.()}>
                  Cancel
                </button>
              {/if}
            {/if}
          {:else if selected.checkedAt !== null}
            <span class="status ok">Connected</span>
            <span class="checked-at">
              checked {new Date(selected.checkedAt * 1000).toLocaleString()}
            </span>
          {:else}
            <span class="status unknown">Not checked yet</span>
          {/if}
        </dd>
        <dt>Color</dt>
        <dd class="colors">
          <button
            class="swatch none"
            class:selected={!selected.color}
            aria-label="No color"
            title="No color"
            onclick={() => onSetColor(selected.id, null)}
          ></button>
          {#each ACCOUNT_COLORS as option (option.value)}
            <button
              class="swatch"
              class:selected={selected.color === option.value}
              style:background={option.value}
              aria-label={option.name}
              title={option.name}
              onclick={() => onSetColor(selected.id, option.value)}
            ></button>
          {/each}
        </dd>
        <dt>Name</dt>
        <dd>{selected.name}</dd>
        <dt>Email</dt>
        <dd>{selected.email}</dd>
        <dt>IMAP server</dt>
        <dd>{selected.imapHost}:{selected.imapPort}</dd>
        <dt>SMTP server</dt>
        <dd>{selected.smtpHost}:{selected.smtpPort}</dd>
        <dt>Username</dt>
        <dd>{selected.username}</dd>
        <dt>Send-as aliases</dt>
        <dd class="aliases">
          <!-- The account's own address: always present, never removable. -->
          <div class="alias-row">
            <input value={selected.name} readonly aria-label="Account name" />
            <input
              value={selected.email}
              readonly
              aria-label="Account address"
            />
            <button
              class="badge"
              class:on={selected.defaultAliasId === null}
              aria-label="Make {selected.email} the default"
              onclick={() => onSetDefaultAlias(selected.id, null)}
            >
              Default
            </button>
            <span class="spacer" aria-hidden="true"></span>
          </div>
          {#each selectedAliases as alias (alias.id)}
            <div class="alias-row">
              <input
                value={alias.name}
                placeholder="Name"
                aria-label="Alias name {alias.email}"
                onchange={(e) =>
                  onUpdateAlias(alias.id, e.currentTarget.value, alias.email)}
              />
              <input
                value={alias.email}
                aria-label="Alias address {alias.email}"
                onchange={(e) =>
                  onUpdateAlias(alias.id, alias.name, e.currentTarget.value)}
              />
              <button
                class="badge"
                class:on={selected.defaultAliasId === alias.id}
                aria-label="Make {alias.email} the default"
                onclick={() => onSetDefaultAlias(selected.id, alias.id)}
              >
                Default
              </button>
              <button
                class="remove"
                aria-label="Delete alias {alias.email}"
                onclick={() => onDeleteAlias(alias.id)}
              >
                ×
              </button>
            </div>
          {/each}
          {#if addingAlias}
            <div class="alias-row">
              <input
                bind:value={aliasName}
                placeholder="Name"
                aria-label="New alias name"
              />
              <input
                bind:value={aliasEmail}
                placeholder="alias@example.com"
                aria-label="New alias address"
              />
              <button
                class="badge on"
                disabled={!aliasEmail.includes("@")}
                onclick={() => void submitAlias(selected.id)}
              >
                Add
              </button>
              <button
                class="remove"
                aria-label="Cancel new alias"
                onclick={() => (addingAlias = false)}
              >
                ×
              </button>
            </div>
          {:else}
            <button
              class="add-alias"
              onclick={() => {
                aliasName = "";
                aliasEmail = "";
                addingAlias = true;
              }}
            >
              + Add alias…
            </button>
          {/if}
        </dd>
      </dl>
    {:else}
      <p class="empty">No accounts yet. Add one with +.</p>
    {/if}
  </div>
</div>

<style>
  .pane {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  aside {
    display: flex;
    flex-direction: column;
    width: 200px;
    flex-shrink: 0;
    border-right: 1px solid var(--hairline);
  }

  ul {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    margin: 0;
    padding: 12px 8px;
    list-style: none;
    overflow-y: auto;
    overflow-x: hidden;
  }

  .account {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 9px;
    border: none;
    border-radius: 7px;
    background: none;
    font: inherit;
    text-align: left;
    cursor: default;
  }

  .account:hover {
    background: var(--bg-hover);
  }

  /* The selected account fills with the accent, like the design. */
  .account.active {
    background: var(--accent);
    color: var(--on-accent);
  }

  /* The account's identity color (the swatch below); gray when unset. */
  .dot {
    flex-shrink: 0;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--text-tertiary);
  }

  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }

  .text > span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .name {
    font-size: 12.5px;
    font-weight: 600;
  }

  .email {
    font-size: 10.5px;
    color: var(--text-secondary);
  }

  .active .email {
    color: inherit;
    opacity: 0.7;
  }

  /* Connection trouble marker on the row — details live in the status. */
  .warning {
    flex-shrink: 0;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #f5a623;
  }

  .status::before {
    content: "● ";
  }

  .status.ok::before {
    color: var(--success);
  }

  .status.broken {
    color: var(--danger);
  }

  .status.unknown::before {
    color: var(--text-tertiary);
  }

  .reconnect,
  .link {
    margin-left: 8px;
    font: inherit;
    font-size: 12px;
  }

  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    cursor: pointer;
  }

  .checked-at {
    margin-left: 6px;
    font-size: 11.5px;
    color: var(--text-secondary);
  }

  .actions {
    display: flex;
    gap: 4px;
    padding: 4px 10px 10px;
  }

  .actions button {
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

  .actions button:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .actions button:disabled {
    opacity: 0.4;
  }

  .detail {
    flex: 1;
    padding: 20px 24px;
    overflow-y: auto;
    overflow-x: hidden;
  }

  /* Stacked field-label-over-value rows, like the design's form. */
  dl {
    margin: 0;
  }

  dt {
    margin-bottom: 3px;
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
  }

  dd {
    margin: 0 0 13px;
    font-size: 13px;
  }

  .colors {
    display: flex;
    flex-wrap: wrap;
    gap: 0.375rem;
  }

  .swatch {
    width: 20px;
    height: 20px;
    padding: 0;
    border: 1px solid rgba(0, 0, 0, 0.15);
    border-radius: 50%;
    cursor: default;
  }

  /* The selected swatch gets a ring: a canvas-colored gap, then the accent. */
  .swatch.selected {
    box-shadow:
      0 0 0 2px var(--bg-settings),
      0 0 0 3.5px var(--accent);
  }

  .swatch.none {
    position: relative;
    background: #fff;
  }

  /* A red diagonal marks the "no color" swatch. */
  .swatch.none::after {
    content: "";
    position: absolute;
    inset: 0;
    border-radius: 50%;
    background: linear-gradient(
      to top right,
      transparent 45%,
      var(--danger) 45%,
      var(--danger) 55%,
      transparent 55%
    );
  }

  .empty {
    color: var(--text-secondary);
  }

  /* One box per identity row, like the mockup's grouped list. */
  .aliases {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: 440px;
    padding: 10px;
    border: 1px solid var(--hairline);
    border-radius: 9px;
    background: var(--bg-window);
  }

  .alias-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .alias-row input {
    min-width: 0;
    padding: 4px 8px;
    border: 1px solid var(--border-chrome);
    border-radius: 6px;
    background: var(--bg-window);
    font: inherit;
    font-size: 12.5px;
    color: var(--text-primary);
  }

  .alias-row input:first-child {
    flex: 2;
  }

  .alias-row input:nth-child(2) {
    flex: 3;
  }

  /* "Default" pill: filled for the active identity, outline otherwise. */
  .badge {
    flex-shrink: 0;
    padding: 3px 10px;
    border: 1px solid var(--accent);
    border-radius: 99px;
    background: none;
    font: inherit;
    font-size: 11px;
    font-weight: 600;
    color: var(--accent);
    cursor: default;
  }

  .badge.on {
    background: var(--accent);
    color: var(--on-accent);
  }

  .badge:disabled {
    opacity: 0.4;
  }

  .remove {
    flex-shrink: 0;
    width: 20px;
    border: none;
    background: none;
    font: inherit;
    font-size: 14px;
    line-height: 1;
    color: var(--text-secondary);
    cursor: default;
  }

  .remove:hover {
    color: var(--danger);
  }

  /* Keeps the primary row's columns aligned with deletable rows below. */
  .spacer {
    width: 20px;
    flex-shrink: 0;
  }

  .add-alias {
    align-self: flex-start;
    margin-top: 2px;
    padding: 2px 0;
    border: none;
    background: none;
    font: inherit;
    font-size: 12.5px;
    font-weight: 500;
    color: var(--accent);
    cursor: default;
  }
</style>
