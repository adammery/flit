<script lang="ts">
  import { onMount } from "svelte";
  import {
    addAccount,
    addAlias,
    addOAuthAccount,
    cancelOAuthSignIn,
    oauthProviders as loadOAuthProviders,
    reconnectAccount,
    closeSettings,
    deleteAlias,
    listAliases,
    setDefaultAlias,
    updateAlias,
    confirmAccountDeletion,
    deleteAccount,
    getAppearance,
    getAvatarLookupEnabled,
    getDateTimeFormat,
    getRemoteImagePolicy,
    getSwipeActions,
    getThreadOrder,
    listAccounts,
    onAccountsChanged,
    setAccountColor,
    setAppearance,
    setAvatarLookupEnabled,
    setDateTimeFormat,
    setRemoteImagePolicy,
    setSwipeActions,
    setThreadOrder,
    testConnection,
  } from "./api";
  import { formatFullDate } from "./format";
  import { DEFAULT_SWIPE_ACTIONS } from "./swipe";
  import {
    SYSTEM_DATE_TIME_FORMAT,
    type Account,
    type Alias,
    type Appearance,
    type AuthKind,
    type DateFormat,
    type DateTimeFormat,
    type NewAccount,
    type RemoteImagePolicy,
    type SwipeAction,
    type SwipeActions,
    type ThreadOrder,
    type TimeFormat,
  } from "./types";
  import AccountsPane from "./AccountsPane.svelte";
  import ExperimentalPane from "./ExperimentalPane.svelte";
  import NotificationsPane from "./NotificationsPane.svelte";
  import ShortcutsPane from "./ShortcutsPane.svelte";
  import SignaturesPane from "./SignaturesPane.svelte";

  const POLICIES: { value: RemoteImagePolicy; label: string; hint: string }[] =
    [
      {
        value: "block",
        label: "Never load",
        hint: "Messages show without their remote images.",
      },
      {
        value: "ask",
        label: "Ask for each message",
        hint: "A banner offers to load them, one message at a time.",
      },
      {
        value: "always",
        label: "Always load",
        hint: "Senders may learn when you open their mail.",
      },
    ];

  const SWIPE_OPTIONS: { value: SwipeAction; label: string }[] = [
    { value: "none", label: "None" },
    { value: "toggleRead", label: "Mark Read / Unread" },
    { value: "archive", label: "Archive" },
    { value: "trash", label: "Trash" },
    { value: "reply", label: "Reply" },
  ];

  const APPEARANCES: { value: Appearance; label: string }[] = [
    { value: "system", label: "System" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
  ];

  let accounts = $state<Account[]>([]);
  let oauthProviders = $state<AuthKind[]>([]);
  let aliases = $state<Alias[]>([]);
  let lastError = $state<string | null>(null);
  let tab = $state<
    | "general"
    | "accounts"
    | "signatures"
    | "notifications"
    | "swipes"
    | "shortcuts"
    | "privacy"
    | "experimental"
  >("accounts");
  let policy = $state<RemoteImagePolicy>("ask");
  let avatarLookup = $state(false);
  let swipes = $state<SwipeActions>(DEFAULT_SWIPE_ACTIONS);
  let appearance = $state<Appearance>("system");
  let threadOrder = $state<ThreadOrder>("newestLast");
  let dateTime = $state<DateTimeFormat>(SYSTEM_DATE_TIME_FORMAT);

  // The sample the pickers preview: today at a fixed 14:30, so it shows the
  // clock difference without ever reading as a stale example.
  const sample = new Date();
  sample.setHours(14, 30, 0, 0);
  let preview = $derived(formatFullDate(sample.toISOString(), dateTime));

  async function refresh() {
    accounts = await listAccounts();
    aliases = await listAliases();
  }

  async function handleAdd(
    account: NewAccount,
    password: string,
  ): Promise<Account | null> {
    lastError = null;
    try {
      // why: Verify & Save — the account is only stored after both servers
      // accepted the credentials (decided 2026-07-08, see CLAUDE.md phase 2).
      await testConnection(account, password);
      const created = await addAccount(account, password);
      await refresh();
      return created;
    } catch (err) {
      lastError = String(err);
      return null;
    }
  }

  async function handleAddOAuth(
    provider: AuthKind,
    name: string,
  ): Promise<Account | null> {
    lastError = null;
    try {
      const created = await addOAuthAccount(provider, name);
      await refresh();
      return created;
    } catch (err) {
      // why: a cancel is the user's own choice, not a problem to report.
      if (!String(err).endsWith("sign-in cancelled")) lastError = String(err);
      return null;
    }
  }

  async function handleReconnect(account: Account) {
    lastError = null;
    try {
      await reconnectAccount(account.id);
      await refresh();
    } catch (err) {
      if (!String(err).endsWith("sign-in cancelled")) lastError = String(err);
    }
  }

  async function handleDelete(account: Account) {
    lastError = null;
    try {
      if (!(await confirmAccountDeletion(account))) return;
      await deleteAccount(account.id);
      await refresh();
    } catch (err) {
      lastError = String(err);
    }
  }

  async function handleSetColor(id: number, color: string | null) {
    lastError = null;
    try {
      await setAccountColor(id, color);
      await refresh();
    } catch (err) {
      lastError = String(err);
    }
  }

  async function handleAddAlias(
    accountId: number,
    name: string,
    email: string,
  ): Promise<Alias | null> {
    lastError = null;
    try {
      const created = await addAlias(accountId, name, email);
      await refresh();
      return created;
    } catch (err) {
      lastError = String(err);
      return null;
    }
  }

  async function handleUpdateAlias(id: number, name: string, email: string) {
    lastError = null;
    try {
      await updateAlias(id, name, email);
    } catch (err) {
      lastError = String(err);
    }
    // why: refresh either way — a rejected edit must snap the input back to
    // the stored value instead of showing text that was never saved.
    await refresh();
  }

  async function handleDeleteAlias(id: number) {
    lastError = null;
    try {
      await deleteAlias(id);
      await refresh();
    } catch (err) {
      lastError = String(err);
    }
  }

  async function handleSetDefaultAlias(
    accountId: number,
    aliasId: number | null,
  ) {
    lastError = null;
    try {
      await setDefaultAlias(accountId, aliasId);
      await refresh();
    } catch (err) {
      lastError = String(err);
    }
  }

  async function toggleAvatarLookup(next: boolean) {
    lastError = null;
    const previous = avatarLookup;
    avatarLookup = next;
    try {
      await setAvatarLookupEnabled(next);
    } catch (err) {
      // why: same as the policy radio — a box that failed to save must not
      // keep claiming the setting changed.
      avatarLookup = previous;
      lastError = String(err);
    }
  }

  async function selectPolicy(next: RemoteImagePolicy) {
    lastError = null;
    const previous = policy;
    policy = next;
    try {
      await setRemoteImagePolicy(next);
    } catch (err) {
      // why: the radio must not lie — a failed save rolls the dot back.
      policy = previous;
      lastError = String(err);
    }
  }

  async function selectSwipe(side: "left" | "right", action: SwipeAction) {
    lastError = null;
    const previous = swipes;
    swipes = { ...previous, [side]: action };
    try {
      await setSwipeActions(swipes);
    } catch (err) {
      // why: the select must not lie — a failed save rolls the value back.
      swipes = previous;
      lastError = String(err);
    }
  }

  async function selectAppearance(next: Appearance) {
    lastError = null;
    const previous = appearance;
    appearance = next;
    try {
      await setAppearance(next);
    } catch (err) {
      // why: the control must not lie — a failed save rolls the value back.
      appearance = previous;
      lastError = String(err);
    }
  }

  // why: the ARIA radio-group pattern — the group is one Tab stop and the
  // arrow keys move (and select) within it, as in macOS System Settings.
  function handleAppearanceKeydown(event: KeyboardEvent) {
    const step =
      event.key === "ArrowRight" || event.key === "ArrowDown"
        ? 1
        : event.key === "ArrowLeft" || event.key === "ArrowUp"
          ? -1
          : 0;
    if (step === 0) return;
    event.preventDefault();
    const index = APPEARANCES.findIndex((a) => a.value === appearance);
    const next = (index + step + APPEARANCES.length) % APPEARANCES.length;
    void selectAppearance(APPEARANCES[next].value);
    const group = event.currentTarget as HTMLElement;
    group.querySelectorAll<HTMLElement>('[role="radio"]')[next]?.focus();
  }

  async function selectThreadOrder(next: ThreadOrder) {
    lastError = null;
    const previous = threadOrder;
    threadOrder = next;
    try {
      await setThreadOrder(next);
    } catch (err) {
      // why: the select must not lie — a failed save rolls the value back.
      threadOrder = previous;
      lastError = String(err);
    }
  }

  async function selectDateTime(next: DateTimeFormat) {
    lastError = null;
    const previous = dateTime;
    dateTime = next;
    try {
      await setDateTimeFormat(next);
    } catch (err) {
      // why: the select must not lie — a failed save rolls the value back.
      dateTime = previous;
      lastError = String(err);
    }
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") void closeSettings();
  }

  onMount(() => {
    void refresh();
    void loadOAuthProviders().then((stored) => (oauthProviders = stored));
    void getRemoteImagePolicy().then((stored) => (policy = stored));
    void getAvatarLookupEnabled().then((stored) => (avatarLookup = stored));
    void getSwipeActions().then((stored) => (swipes = stored));
    void getAppearance().then((stored) => (appearance = stored));
    void getThreadOrder().then((stored) => (threadOrder = stored));
    void getDateTimeFormat().then((stored) => (dateTime = stored));
    // why: refreshes also cover changes made elsewhere (a future main-window
    // action, another settings session) — the backend broadcasts the event.
    const unlisten = onAccountsChanged(() => void refresh());
    return () => {
      void unlisten.then((stop) => stop());
    };
  });
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="window">
  <header>
    <button
      class="tab"
      class:active={tab === "general"}
      onclick={() => (tab = "general")}
    >
      General
    </button>
    <button
      class="tab"
      class:active={tab === "accounts"}
      onclick={() => (tab = "accounts")}
    >
      Accounts
    </button>
    <button
      class="tab"
      class:active={tab === "signatures"}
      onclick={() => (tab = "signatures")}
    >
      Signatures
    </button>
    <button
      class="tab"
      class:active={tab === "notifications"}
      onclick={() => (tab = "notifications")}
    >
      Notifications
    </button>
    <button
      class="tab"
      class:active={tab === "swipes"}
      onclick={() => (tab = "swipes")}
    >
      Swipes
    </button>
    <button
      class="tab"
      class:active={tab === "shortcuts"}
      onclick={() => (tab = "shortcuts")}
    >
      Shortcuts
    </button>
    <button
      class="tab"
      class:active={tab === "privacy"}
      onclick={() => (tab = "privacy")}
    >
      Privacy
    </button>
    <button
      class="tab"
      class:active={tab === "experimental"}
      onclick={() => (tab = "experimental")}
    >
      Experimental
    </button>
  </header>

  {#if tab === "general"}
    <section class="content">
      <div class="section-label">Appearance</div>
      <div class="group">
        <div class="row">
          <span class="row-label" id="appearance-label">Appearance</span>
          <div
            class="segmented"
            role="radiogroup"
            aria-labelledby="appearance-label"
            tabindex="-1"
            onkeydown={handleAppearanceKeydown}
          >
            {#each APPEARANCES as option (option.value)}
              <button
                type="button"
                role="radio"
                class="segment"
                aria-checked={appearance === option.value}
                tabindex={appearance === option.value ? 0 : -1}
                onclick={() => void selectAppearance(option.value)}
              >
                {option.label}
              </button>
            {/each}
          </div>
        </div>
      </div>
      <p class="explain">
        System follows the macOS setting and switches with it.
      </p>
      <div class="section-label">Conversations</div>
      <div class="group">
        <div class="row">
          <label class="row-label" for="thread-order">Message order</label>
          <select
            id="thread-order"
            value={threadOrder}
            onchange={(e) =>
              void selectThreadOrder(e.currentTarget.value as ThreadOrder)}
          >
            <option value="newestLast">Newest at the bottom</option>
            <option value="newestFirst">Newest on top</option>
          </select>
        </div>
      </div>
      <p class="explain">
        How messages inside a conversation are ordered when you open it.
      </p>
      <div class="section-label">Date &amp; time</div>
      <div class="group">
        <div class="row">
          <label class="row-label" for="date-format">Date</label>
          <select
            id="date-format"
            value={dateTime.date}
            onchange={(e) =>
              void selectDateTime({
                ...dateTime,
                date: e.currentTarget.value as DateFormat,
              })}
          >
            <option value="system">System</option>
            <option value="dd.mm.yyyy">dd.mm.yyyy</option>
            <option value="dd.mm.yy">dd.mm.yy</option>
            <option value="dd/mm/yyyy">dd/mm/yyyy</option>
            <option value="mm/dd/yyyy">mm/dd/yyyy</option>
            <option value="yyyy-mm-dd">yyyy-mm-dd</option>
            <option value="yyyy/mm/dd">yyyy/mm/dd</option>
          </select>
        </div>
        <div class="row">
          <label class="row-label" for="time-format">Time</label>
          <select
            id="time-format"
            value={dateTime.time}
            onchange={(e) =>
              void selectDateTime({
                ...dateTime,
                time: e.currentTarget.value as TimeFormat,
              })}
          >
            <option value="system">System</option>
            <option value="24h">24-hour (14:30)</option>
            <option value="12h">12-hour (2:30 PM)</option>
          </select>
        </div>
      </div>
      <p class="explain">Dates show as {preview}.</p>
    </section>
  {:else if tab === "accounts"}
    <AccountsPane
      {accounts}
      {aliases}
      {oauthProviders}
      onAdd={handleAdd}
      onAddOAuth={handleAddOAuth}
      onCancelSignIn={() => void cancelOAuthSignIn()}
      onReconnect={handleReconnect}
      onDelete={handleDelete}
      onSetColor={handleSetColor}
      onAddAlias={handleAddAlias}
      onUpdateAlias={handleUpdateAlias}
      onDeleteAlias={handleDeleteAlias}
      onSetDefaultAlias={handleSetDefaultAlias}
    />
  {:else if tab === "signatures"}
    <SignaturesPane {accounts} />
  {:else if tab === "notifications"}
    <NotificationsPane {accounts} />
  {:else if tab === "swipes"}
    <section class="content">
      <div class="section-label">Swipe actions in the message list</div>
      <div class="group">
        {#each [{ side: "left", label: "Swipe left" }, { side: "right", label: "Swipe right" }] as const as row (row.side)}
          <div class="row">
            <label class="row-label" for="swipe-{row.side}">{row.label}</label>
            <select
              id="swipe-{row.side}"
              value={swipes[row.side]}
              onchange={(e) =>
                void selectSwipe(
                  row.side,
                  e.currentTarget.value as SwipeAction,
                )}
            >
              {#each SWIPE_OPTIONS as option (option.value)}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
          </div>
        {/each}
      </div>
      <p class="explain">
        What a two-finger swipe on a message row does. Pick None to turn a
        direction off.
      </p>
    </section>
  {:else if tab === "shortcuts"}
    <ShortcutsPane />
  {:else if tab === "experimental"}
    <ExperimentalPane />
  {:else}
    <section class="content">
      <div class="section-label">Remote images in messages</div>
      <div class="group">
        {#each POLICIES as option (option.value)}
          <div class="choice">
            <input
              type="radio"
              id="policy-{option.value}"
              name="remote-images"
              value={option.value}
              checked={policy === option.value}
              onchange={() => void selectPolicy(option.value)}
            />
            <span>
              <label for="policy-{option.value}">{option.label}</label>
              <small>{option.hint}</small>
            </span>
          </div>
        {/each}
      </div>
      <p class="explain">
        Remote images can tell the sender when, where and on what device a
        message was opened. Known tracking images are always removed. Images
        attached inside the message always show.
      </p>

      <div class="section-label">Sender avatars</div>
      <div class="group">
        <div class="choice">
          <input
            type="checkbox"
            id="avatar-lookup"
            checked={avatarLookup}
            onchange={(e) => void toggleAvatarLookup(e.currentTarget.checked)}
          />
          <span>
            <label for="avatar-lookup">Look up sender icons online</label>
            <small>Off shows coloured initials instead.</small>
          </span>
        </div>
      </div>
      <p class="explain">
        Off by default. When on, Flit asks each sender's domain for its site
        icon — never an address, only the domain, so the request says which
        organisations write to you and never who you correspond with. Each
        domain is asked at most once a month and the answer is cached, so it
        cannot reveal which message you opened, or when. With this off, every
        sender keeps the coloured initials.
      </p>
    </section>
  {/if}
</div>

{#if lastError}
  <div class="error-banner" role="alert">
    <span>{lastError}</span>
    <button aria-label="Dismiss error" onclick={() => (lastError = null)}>
      ×
    </button>
  </div>
{/if}

<style>
  /* The grouped-settings canvas the white group boxes sit on. */
  .window {
    display: flex;
    flex-direction: column;
    height: 100vh;
    background: var(--bg-settings);
  }

  /* macOS-preferences chrome: same gradient strip as the main toolbar,
     with the centered tab pills. */
  header {
    display: flex;
    justify-content: center;
    /* Eight tabs no longer fit a window at its minimum width. */
    flex-wrap: wrap;
    gap: 4px;
    flex-shrink: 0;
    padding: 8px 12px;
    background: linear-gradient(
      var(--bg-toolbar-top),
      var(--bg-toolbar-bottom)
    );
    border-bottom: 1px solid var(--border-chrome);
  }

  .tab {
    padding: 5px 14px;
    border: none;
    border-radius: 7px;
    background: none;
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text-secondary);
    cursor: default;
  }

  .tab:hover {
    background: var(--bg-hover);
  }

  .tab.active {
    background: var(--bg-selected-muted);
    color: var(--text-primary);
  }

  .content {
    padding: 20px 26px;
    overflow-y: auto;
    overflow-x: hidden;
  }

  .section-label {
    margin-bottom: 7px;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-secondary);
  }

  /* A white grouped box of setting rows. */
  .group {
    display: flex;
    flex-direction: column;
    max-width: 520px;
    padding: 3px 0;
    border: 1px solid var(--hairline);
    border-radius: 9px;
    background: var(--bg-window);
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
    padding: 8px 12px;
  }

  .row-label {
    font-size: 13px;
    font-weight: 500;
  }

  .row select {
    max-width: 220px;
    padding: 3px 8px;
    border: 1px solid var(--border-chrome);
    border-radius: 6px;
    background: var(--bg-window);
    font: inherit;
    font-size: 12.5px;
  }

  /* macOS-style segmented control: a sunken track with the chosen
     segment raised as a card. */
  .segmented {
    display: flex;
    gap: 2px;
    padding: 2px;
    border-radius: 7px;
    background: var(--bg-field);
  }

  .segment {
    min-width: 64px;
    padding: 3px 12px;
    border: none;
    border-radius: 5px;
    background: none;
    font: inherit;
    font-size: 12.5px;
    font-weight: 500;
    color: var(--text-secondary);
    cursor: default;
  }

  .segment[aria-checked="true"] {
    background: var(--bg-card);
    box-shadow: 0 0 0 0.5px var(--card-border-strong);
    color: var(--text-primary);
  }

  .segment:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .explain {
    margin: 8px 2px 0;
    max-width: 520px;
    font-size: 11.5px;
    color: var(--text-secondary);
  }

  .choice {
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 9px 12px;
  }

  .choice label {
    font-size: 13px;
    font-weight: 600;
  }

  .choice small {
    display: block;
    margin-top: 1px;
    font-size: 11.5px;
    color: var(--text-secondary);
  }

  .error-banner {
    position: fixed;
    top: 0.75rem;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0.75rem;
    border: 1px solid var(--danger-border);
    border-radius: 0.375rem;
    background: var(--danger-bg);
    color: var(--danger-text);
  }

  .error-banner button {
    border: none;
    background: none;
    font: inherit;
    color: inherit;
    cursor: pointer;
  }
</style>
