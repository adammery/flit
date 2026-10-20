<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import {
    attachmentPreview,
    closeCompose,
    discardDraft,
    getShortcuts,
    inspectAttachments,
    listAccounts,
    listAliases,
    listSignatures,
    onFileDrop,
    queueSend,
    saveDraft,
    scheduleSend,
    takeComposeDraft,
  } from "./api";
  import { fileExt } from "./attachments";
  import { debounce } from "./debounce";
  import {
    composeHtmlBody,
    composePlainBody,
    isDraftEmpty,
    quoteEditorHtml,
    splitComposedHtml,
  } from "./draft";
  import { formatFileSize } from "./format";
  import { textToHtml } from "./richtext";
  import type {
    Account,
    Alias,
    AttachmentInfo,
    DraftQuote,
    OutgoingMessage,
    ShortcutBinding,
    Signature,
  } from "./types";
  import { actionFor, comboFromEvent, firesIn } from "./shortcuts";
  import {
    presets,
    toDatetimeLocal,
    toEpochSeconds,
    type SendLaterPreset,
  } from "./sendLater";
  import RecipientField from "./RecipientField.svelte";
  import RichTextEditor from "./RichTextEditor.svelte";

  let accounts = $state<Account[]>([]);
  let accountId = $state<number | null>(null);
  let aliases = $state<Alias[]>([]);
  /** Send-as alias of the picked identity; null = the account's address. */
  let aliasId = $state<number | null>(null);
  // why a string key: one <select> carries account+alias pairs — "2" is the
  // account's own address, "2:10" its alias with id 10.
  let identityKey = $derived(
    accountId === null
      ? ""
      : aliasId === null
        ? String(accountId)
        : `${accountId}:${aliasId}`,
  );

  function aliasesFor(id: number): Alias[] {
    return aliases.filter((a) => a.accountId === id);
  }
  let to = $state("");
  let cc = $state("");
  let bcc = $state("");
  // why: Cc/Bcc rows stay hidden until asked for (or the draft carries
  // them) so the default envelope stays as quiet as Apple Mail's.
  let showCcBcc = $state(false);
  let subject = $state("");
  let body = $state("");
  let bodyHtml = $state("");
  /** The draft's plain text, handed to the editor exactly once at mount. */
  let initialBody = $state("");
  /** What the editor opens with: draft text (+ default signature). */
  let initialHtml = $state("");
  // why: the editor renders only after the draft resolved — Tiptap takes its
  // content at construction, so mounting early would show an empty body.
  let loaded = $state(false);

  // The reply's quote of the original, parked OUTSIDE the editor and shown
  // as a collapsed ••• bar. Expanding inserts it into the editor (and this
  // becomes null); sending/saving while parked merges it into body/bodyHtml.
  let quote = $state<DraftQuote | null>(null);

  let signatures = $state<Signature[]>([]);
  let signatureId = $state<number | null>(null);
  let editorRef = $state<
    | {
        swapBlock: (prev: string, next: string) => void;
        appendContent: (blockHtml: string) => void;
      }
    | undefined
  >();
  // Plain lets — bookkeeping only, never rendered.
  /** The signature HTML currently sitting in the body (swap target). */
  let appliedSigBody = "";
  /** Once the user picks a signature by hand, From changes stop overriding it. */
  let signatureTouched = false;

  function defaultSignatureFor(id: number | null): Signature | null {
    const account = accounts.find((a) => a.id === id);
    return signatures.find((sig) => sig.id === account?.signatureId) ?? null;
  }

  /** Swap (or insert) the signature block inside the live editor. */
  function applySignature(next: Signature | null) {
    editorRef?.swapBlock(appliedSigBody, next?.body ?? "");
    appliedSigBody = next?.body ?? "";
    signatureId = next?.id ?? null;
  }

  let showSignatureMenu = $state(false);
  let signatureToggle = $state<HTMLButtonElement>();
  let signatureMenuEl = $state<HTMLElement>();

  function pickSignature(sig: Signature | null) {
    signatureTouched = true;
    applySignature(sig);
    showSignatureMenu = false;
  }

  function onFromPicked(event: Event) {
    const [acc, alias] = (
      event.currentTarget as HTMLSelectElement
    ).value.split(":");
    accountId = Number(acc);
    aliasId = alias ? Number(alias) : null;
    // Signatures follow the account, not the alias.
    if (signatureTouched) return;
    applySignature(defaultSignatureFor(accountId));
  }
  let attachments = $state<AttachmentInfo[]>([]);
  /** Thumbnail data: URIs by attachment path; absent = placeholder card. */
  let previews = $state<Record<string, string>>({});
  /** A file drag is currently above the window — shows the drop overlay. */
  let dropHover = $state(false);

  let queueing = $state(false);
  let error = $state<string | null>(null);

  // Gmail-style silent drafts: while the user types, the message is saved to
  // the account's server Drafts folder (debounced), and once more when the
  // window closes — no dialogs. draftMessageId is the handle under which the
  // previous version is replaced on each save.
  const AUTOSAVE_MS = 30_000;
  let draftMessageId: string | null = null;
  // The reply's threading identity (bare Message-IDs), fixed at open time.
  // Every save and the final send must carry it — without In-Reply-To and
  // References the message starts its own conversation on the server
  // instead of joining the one it answers.
  let inReplyTo: string | null = null;
  let references: string | null = null;
  let saveError = $state<string | null>(null);
  // why watching, not `loaded`: the editor mounts on `loaded`, so its
  // initial bind-backs still look like edits — only changes after the whole
  // mount sequence settled count as the user's.
  let watching = false;
  let dirty = false;
  /** The user edited something in THIS window — unlike `dirty`, never
   * cleared by an autosave. Gates the close dialog: only the user's own
   * edits are worth asking about. */
  let everDirty = false;
  /** Don't Save was chosen — no save may run from here to teardown. */
  let discarded = false;
  /** The save round trip in flight, or null — closers await it rather than
   * race it. */
  let saving: Promise<void> | null = null;
  let sent = false;

  /** The Save / Don't Save / Cancel prompt shown by a close request. */
  let showCloseDialog = $state(false);

  /** The send-later popover; presets are computed fresh on every open so a
   * long-lived window never offers a moment that has already passed. */
  let showSendLater = $state(false);
  let sendLaterToggle = $state<HTMLButtonElement>();
  let sendLaterEl = $state<HTMLElement>();
  let sendLaterPresets = $state<SendLaterPreset[]>([]);
  let sendAt = $state("");
  let sendAtMin = $state("");

  function toggleSendLater() {
    showSendLater = !showSendLater;
    if (showSendLater) {
      const now = new Date();
      sendLaterPresets = presets(now);
      sendAtMin = toDatetimeLocal(now);
    }
  }

  onMount(() => {
    void getShortcuts()
      .then((bindings) => (shortcuts = bindings))
      .catch((err: unknown) =>
        console.error("failed to load keyboard shortcuts:", err),
      );
    let unlisten: (() => void) | undefined;
    let unlistenClose: (() => void) | undefined;
    // why: a listener can finish registering after the component is
    // already gone — teardown would then see no handle and the listener
    // would live on for the webview's life.
    let destroyed = false;
    void (async () => {
      const [loadedAccounts, draft, loadedSignatures, loadedAliases] =
        await Promise.all([
          listAccounts(),
          takeComposeDraft(),
          listSignatures(),
          listAliases(),
        ]);
      accounts = loadedAccounts;
      signatures = loadedSignatures;
      aliases = loadedAliases;
      // why: a null draft (webview reload after pickup) degrades to a blank
      // message from the first account instead of a broken window.
      accountId = draft?.accountId ?? accounts[0]?.id ?? null;
      // why the find: an alias deleted since the draft was parked degrades
      // to the account's own address instead of an empty From picker.
      const wanted = draft ? draft.aliasId : accounts[0]?.defaultAliasId;
      aliasId =
        aliases.find((a) => a.id === wanted && a.accountId === accountId)
          ?.id ?? null;
      to = draft?.to ?? "";
      cc = draft?.cc ?? "";
      bcc = draft?.bcc ?? "";
      showCcBcc = cc !== "" || bcc !== "";
      subject = draft?.subject ?? "";
      initialBody = draft?.body ?? "";
      body = initialBody;
      inReplyTo = draft?.inReplyTo ?? null;
      references = draft?.references ?? null;
      // A draft handed back by undo/failed-send keeps replacing the same
      // server version. Mark it dirty so closing this window re-saves it —
      // its newest text may never have been autosaved.
      draftMessageId = draft?.draftMessageId ?? null;
      dirty = draftMessageId !== null;
      if (draft?.bodyHtml) {
        // A reopened draft (undo, failed send) already carries whatever
        // signature it had — don't guess and don't insert another one.
        // Its quote block is split back out of the composed HTML so the
        // editor never chews on the original's markup; without the ridden
        // quote data the composed HTML stays whole as a fallback.
        const { own, hasQuote } = splitComposedHtml(draft.bodyHtml);
        if (hasQuote && draft.quote) {
          initialHtml = own;
          quote = draft.quote;
        } else {
          initialHtml = draft.bodyHtml;
        }
        signatureTouched = true;
      } else {
        quote = draft?.quote ?? null;
        // A reopened server draft already contains whatever signature it
        // had — re-applying the default would stack one per reopen+save.
        const sig = draftMessageId ? null : defaultSignatureFor(accountId);
        signatureId = sig?.id ?? null;
        appliedSigBody = sig?.body ?? "";
        const bodyPart = textToHtml(initialBody) || "<p></p>";
        initialHtml = sig?.body
          ? `${bodyPart}<p></p>${sig.body}`
          : bodyPart;
      }
      loaded = true;
      // why re-stat instead of trusting the draft: an undone/failed send may
      // reopen after the file changed or vanished — surface that now.
      if (draft?.attachments?.length) {
        await addAttachments(draft.attachments.map((a) => a.path));
      }
      const unlistenDrop = await onFileDrop({
        onHover: (hovering) => (dropHover = hovering),
        onDrop: (paths) => void addAttachments(paths),
      });
      if (destroyed) {
        unlistenDrop();
        return;
      }
      unlisten = unlistenDrop;
      const unlistenCloseRequested = await getCurrentWindow().onCloseRequested(
        async (event) => {
          if (sent || discarded) return;
          const message = currentMessage();
          const nothingToKeep =
            !message || (isDraftEmpty(message) && draftMessageId === null);
          if (everDirty) {
            if (nothingToKeep) return; // typed, then cleared it all again
            // The user's own edits — their call: Save / Don't Save / Cancel.
            event.preventDefault();
            showCloseDialog = true;
            return;
          }
          if (!dirty || nothingToKeep) return;
          // An untouched handback (undo, failed send) re-saves silently, no
          // questions: the text is not this window's edits, and its newest
          // version may never have been autosaved.
          // why: hold the window open until the save lands — destroying the
          // webview mid-save could lose the newest keystrokes.
          event.preventDefault();
          await saveNow();
          if (dirty) return; // save failed; keep the window and its error
          await getCurrentWindow().destroy();
        },
      );
      if (destroyed) {
        unlistenCloseRequested();
        return;
      }
      unlistenClose = unlistenCloseRequested;
      watching = true;
    })();
    return () => {
      destroyed = true;
      unlisten?.();
      unlistenClose?.();
    };
  });

  function currentMessage(): OutgoingMessage | null {
    return accountId === null ? null : buildMessage(accountId);
  }

  async function saveNow() {
    if (saving || sent || discarded || !dirty) return;
    const message = currentMessage();
    if (!message) return;
    // An untouched-then-cleared window has nothing worth a server round
    // trip; once a version exists it keeps being replaced, even by "".
    if (isDraftEmpty(message) && draftMessageId === null) return;
    // why clear before the await: keystrokes landing during the save must
    // re-mark the draft dirty, not be swallowed by a stale flag.
    dirty = false;
    saving = saveDraft(message, draftMessageId).then(
      (id) => {
        draftMessageId = id;
        saveError = null;
      },
      (err) => {
        dirty = true;
        saveError = String(err);
      },
    );
    try {
      await saving;
    } finally {
      saving = null;
    }
  }

  const scheduleAutosave = debounce(() => void saveNow(), AUTOSAVE_MS);

  $effect(() => {
    // why bodyHtml too: bold/list formatting changes the html rendering
    // while the plain text stays put — it is an edit all the same.
    void [to, cc, bcc, subject, body, bodyHtml, attachments, quote];
    if (!watching) return;
    dirty = true;
    everDirty = true;
    scheduleAutosave();
  });

  /** Resolve once no save is in flight — an autosave can overlap the close
   * dialog, and both outcomes below must act on its result, not race it. */
  async function saveSettled() {
    await saving;
  }

  async function saveAndClose() {
    showCloseDialog = false;
    await saveSettled();
    // A no-op when an autosave already holds this exact content.
    await saveNow();
    if (dirty) return; // save failed; the error strip explains, window stays
    await getCurrentWindow().destroy();
  }

  async function discardAndClose() {
    showCloseDialog = false;
    discarded = true;
    await saveSettled();
    if (draftMessageId !== null && accountId !== null) {
      try {
        await discardDraft(accountId, draftMessageId);
      } catch {
        // why swallowed: an orphaned server draft is cosmetic — never hold
        // a window the user explicitly discarded hostage over cleanup.
      }
    }
    await getCurrentWindow().destroy();
  }

  // Loaded once per window: a compose window is short-lived, so a rebinding
  // made while it is open applies from the next one.
  let shortcuts = $state<ShortcutBinding[]>([]);
  let formEl = $state<HTMLFormElement | null>(null);

  /** The Send shortcut (⌘↩ by default) submits the form exactly like the
   * Send button, so the same validation and queue flow apply. */
  function onShortcutKey(event: KeyboardEvent) {
    if (showCloseDialog) return;
    const combo = comboFromEvent(event);
    if (combo === null || !firesIn(event.target, combo)) return;
    if (actionFor(shortcuts, combo) !== "send") return;
    event.preventDefault();
    event.stopPropagation();
    formEl?.requestSubmit();
  }

  /** Escape closes whatever floats on top: the close prompt first, then an
   * open popover, handing focus back to the button that opened it. */
  function onDialogKey(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    if (showCloseDialog) {
      event.preventDefault();
      showCloseDialog = false;
    } else if (showSendLater) {
      event.preventDefault();
      showSendLater = false;
      sendLaterToggle?.focus();
    } else if (showSignatureMenu) {
      event.preventDefault();
      showSignatureMenu = false;
      signatureToggle?.focus();
    }
  }

  /** A press anywhere but a popover or its own toggle closes it. why not the
   * toggle: its click closes the popover itself, and closing it here first
   * would let that click open it again. */
  function closePopoversOnOutsidePress(event: MouseEvent) {
    const target = event.target as Node;
    const outside = (...els: (HTMLElement | undefined)[]) =>
      els.every((el) => !el?.contains(target));
    if (showSendLater && outside(sendLaterEl, sendLaterToggle)) {
      showSendLater = false;
    }
    if (showSignatureMenu && outside(signatureMenuEl, signatureToggle)) {
      showSignatureMenu = false;
    }
  }

  /** ••• click: the quote moves into the editor and becomes editable —
   * one-way, like Proton/Gmail; removing it again is ordinary editing. */
  function expandQuote() {
    if (!quote) return;
    editorRef?.appendContent(quoteEditorHtml(quote));
    quote = null;
  }

  function removeQuote() {
    quote = null;
  }

  async function addAttachments(paths: string[]) {
    if (queueing) return;
    try {
      const infos = await inspectAttachments(paths);
      const fresh = infos.filter(
        (info) => !attachments.some((a) => a.path === info.path),
      );
      attachments = [...attachments, ...fresh];
      // Thumbnails arrive per card as they render; the placeholder shows
      // in the meantime.
      fresh.forEach((info) => void loadPreview(info.path));
      error = null;
    } catch (err) {
      error = String(err);
    }
  }

  async function loadPreview(path: string) {
    try {
      const uri = await attachmentPreview(path);
      if (uri) previews[path] = uri;
    } catch {
      // why swallowed: a preview is decoration — a failure just leaves
      // the extension placeholder in place.
    }
  }

  function removeAttachment(path: string) {
    attachments = attachments.filter((a) => a.path !== path);
    delete previews[path];
  }

  function buildMessage(accountId: number) {
    return {
      accountId,
      aliasId: aliasId ?? undefined,
      to,
      cc,
      bcc,
      subject,
      // A parked quote is merged in here — the editor content stays clean.
      body: composePlainBody(body, quote),
      // why || undefined: an empty editor reports "" — the wire format
      // treats a missing field as "plain text only". An empty editor WITH
      // a quote still composes (quote-only reply).
      bodyHtml: composeHtmlBody(bodyHtml, quote) || undefined,
      quote: quote ?? undefined,
      attachments: attachments.map(({ path, name }) => ({ path, name })),
      // why: rides along into queue_send so the backend can clear the
      // autosaved server draft once the send succeeds.
      draftMessageId: draftMessageId ?? undefined,
      inReplyTo: inReplyTo ?? undefined,
      references: references ?? undefined,
    };
  }

  // why: the backend queue owns the undo window — this window only hands
  // the message over (which validates addresses) and closes. The undo badge
  // lives in the main window from here on.
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (queueing || accountId === null) return;
    queueing = true;
    error = null;
    try {
      await queueSend(buildMessage(accountId));
      // why: the message now belongs to the send queue — the close below
      // must not snapshot it back into the Drafts folder.
      sent = true;
      await closeCompose();
    } catch (err) {
      error = String(err);
    } finally {
      queueing = false;
    }
  }

  /** Send Later: park the message in the backend until `at` (unix seconds)
   * and close, mirroring submit's validate-then-close flow. */
  async function schedule(at: number) {
    if (queueing || accountId === null) return;
    queueing = true;
    error = null;
    try {
      await scheduleSend(buildMessage(accountId), at);
      // why: the message now lives in the scheduled queue — closing must
      // not snapshot it back into the Drafts folder as unfinished work.
      sent = true;
      await closeCompose();
    } catch (err) {
      error = String(err);
      showSendLater = false;
    } finally {
      queueing = false;
    }
  }
</script>

<!-- why capture: the editor binds ⌘↩ to a line break of its own. Catching
     the shortcut on the way down, before it reaches the editor, keeps that
     stray break out of the message being sent. -->
<svelte:window
  onkeydown={onDialogKey}
  onkeydowncapture={onShortcutKey}
  onmousedown={closePopoversOnOutsidePress}
/>

<form
  bind:this={formEl}
  class="window"
  class:drop-target={dropHover}
  aria-label="Compose message"
  onsubmit={submit}
>
  <!-- Canary-style toolbar: a tinted strip that hosts the native traffic
       lights (title bar overlay) and doubles as the window drag handle.
       Closing goes through the red traffic light — no extra ✕ here. -->
  <header class="toolbar" data-tauri-drag-region>
    <button
      type="button"
      class="icon"
      class:active={showSendLater}
      aria-label="Send later"
      bind:this={sendLaterToggle}
      title="Send later"
      onclick={toggleSendLater}
      disabled={queueing || accountId === null}
    >
      <svg
        viewBox="0 0 24 24"
        width="18"
        height="18"
        fill="none"
        stroke="currentColor"
        stroke-width="1.8"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <circle cx="12" cy="12" r="9" />
        <path d="M12 7v5l3 2" />
      </svg>
    </button>
    <button
      type="submit"
      class="icon send"
      aria-label="Send"
      title="Send"
      disabled={queueing || accountId === null}
    >
      <svg
        viewBox="0 0 24 24"
        width="18"
        height="18"
        fill="none"
        stroke="currentColor"
        stroke-width="1.8"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <path d="M22 2 11 13" />
        <path d="M22 2 15 22l-4-9-9-4z" />
      </svg>
    </button>
  </header>

  {#if showSendLater}
    <!-- Floating panel under the toolbar, Apple Mail's "Send Later" pattern:
         a couple of sensible presets plus an exact date-time picker. -->
    <div
      class="send-later"
      role="dialog"
      aria-label="Send later options"
      bind:this={sendLaterEl}
    >
      {#each sendLaterPresets as preset (preset.label)}
        <button
          type="button"
          class="preset"
          onclick={() => schedule(Math.floor(preset.date.getTime() / 1000))}
          disabled={queueing}
        >
          {preset.label}
        </button>
      {/each}
      <div class="custom">
        <input
          type="datetime-local"
          aria-label="Send at"
          bind:value={sendAt}
          min={sendAtMin}
          disabled={queueing}
        />
        <button
          type="button"
          class="confirm"
          onclick={() => schedule(toEpochSeconds(sendAt))}
          disabled={queueing || sendAt === ""}
        >
          Schedule
        </button>
      </div>
    </div>
  {/if}

  <!-- Canary-style envelope fields: quiet label + borderless input rows
       divided by hairlines, subject as a bold standalone line. -->
  <div class="row">
    <span class="key" aria-hidden="true">To:</span>
    <RecipientField
      label="To"
      bind:value={to}
      required
      disabled={queueing}
      placeholder="recipient@example.com"
    />
    {#if !showCcBcc}
      <button
        type="button"
        class="reveal"
        onclick={() => (showCcBcc = true)}
        disabled={queueing}
      >
        Cc/Bcc
      </button>
    {/if}
  </div>
  {#if showCcBcc}
    <div class="row">
      <span class="key" aria-hidden="true">Cc:</span>
      <RecipientField label="Cc" bind:value={cc} disabled={queueing} />
    </div>
    <div class="row">
      <span class="key" aria-hidden="true">Bcc:</span>
      <RecipientField label="Bcc" bind:value={bcc} disabled={queueing} />
    </div>
  {/if}
  <div class="row from-row">
    <span class="key" aria-hidden="true">From:</span>
    <select
      aria-label="From"
      value={identityKey}
      onchange={onFromPicked}
      disabled={queueing}
    >
      {#each accounts as account (account.id)}
        <!-- One group per account: its own address first, aliases below.
             why the email as label: account display names repeat across
             accounts (all "me"), only the address tells them apart. -->
        <optgroup label={account.email}>
          <option value={String(account.id)}>{account.email}</option>
          {#each aliasesFor(account.id) as alias (alias.id)}
            <option value={`${account.id}:${alias.id}`}>{alias.email}</option>
          {/each}
        </optgroup>
      {/each}
    </select>
    <span class="chevron" aria-hidden="true">⌄</span>
    {#if signatures.length > 0}
      <!-- Signature lives as a quiet icon at the row's right edge — the
           menu it opens swaps the block inside the body. -->
      <button
        type="button"
        class="sig-toggle"
        class:active={showSignatureMenu}
        aria-label="Signature"
        bind:this={signatureToggle}
        title="Signature"
        onclick={() => (showSignatureMenu = !showSignatureMenu)}
        disabled={queueing}
      >
        <svg
          viewBox="0 0 24 24"
          width="15"
          height="15"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <path d="M12 20h9" />
          <path
            d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z"
          />
        </svg>
      </button>
      {#if showSignatureMenu}
        <div
          class="signature-menu"
          role="dialog"
          aria-label="Choose signature"
          bind:this={signatureMenuEl}
        >
          <button
            type="button"
            class="sig-item"
            class:active={signatureId === null}
            onclick={() => pickSignature(null)}
          >
            None
          </button>
          {#each signatures as sig (sig.id)}
            <button
              type="button"
              class="sig-item"
              class:active={signatureId === sig.id}
              onclick={() => pickSignature(sig)}
            >
              {sig.name}
            </button>
          {/each}
        </div>
      {/if}
    {/if}
  </div>
  <div class="row subject-row">
    <input
      class="subject"
      aria-label="Subject"
      bind:value={subject}
      disabled={queueing}
      placeholder="Subject"
    />
  </div>

  {#if error || saveError}
    <p class="error" role="alert">
      {error ?? `Draft not saved: ${saveError}`}
    </p>
  {/if}

  {#if loaded}
    <div class="body-area" class:with-attachments={attachments.length > 0}>
      <RichTextEditor
        bind:this={editorRef}
        {initialHtml}
        bind:text={body}
        bind:html={bodyHtml}
        disabled={queueing}
      />
      {#if quote}
        <!-- The parked reply-quote, Gmail-style: sent as-is unless expanded
             into the editor (•••) or removed (×). -->
        <div class="quote-bar">
          <button
            type="button"
            class="quote-toggle"
            aria-label="Show quoted text"
            title="Show quoted text"
            onclick={expandQuote}
            disabled={queueing}
          >
            •••
          </button>
          <span class="quote-attribution">{quote.attribution}</span>
          <button
            type="button"
            class="quote-remove"
            aria-label="Remove quoted text"
            title="Remove quoted text"
            onclick={removeQuote}
            disabled={queueing}
          >
            ×
          </button>
        </div>
      {/if}
    </div>
  {/if}

  {#if attachments.length > 0}
    <!-- Floating panel pinned over the bottom of the body, so attachments
         never push the envelope fields or the text around. -->
    <ul class="attachments" aria-label="Attachments">
      {#each attachments as attachment (attachment.path)}
        <li class="card">
          <button
            type="button"
            class="card-remove"
            aria-label={`Remove ${attachment.name}`}
            onclick={() => removeAttachment(attachment.path)}
            disabled={queueing}
          >
            ×
          </button>
          {#if previews[attachment.path]}
            <img
              class="thumb"
              src={previews[attachment.path]}
              alt={attachment.name}
            />
          {:else}
            <span class="thumb ext" aria-hidden="true">
              <!-- why "?": the same tile the reading pane shows for a file
                   without an extension. -->
              {fileExt(attachment.name) || "?"}
            </span>
          {/if}
          <span class="card-name">{attachment.name}</span>
          <span class="card-size">{formatFileSize(attachment.size)}</span>
        </li>
      {/each}
    </ul>
  {/if}

  {#if showCloseDialog}
    <!-- macOS-alert-style prompt, rendered in-window: the native dialog
         plugin caps out at two buttons, and this stays on our flat look. -->
    <div class="dialog-backdrop">
      <div
        class="close-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="close-dialog-title"
      >
        <h2 id="close-dialog-title">Save this message as a draft?</h2>
        <p>
          This message has not been sent and contains unsaved changes. You
          can save it as a draft to work on later.
        </p>
        <!-- svelte-ignore a11y_autofocus — Enter should mean Save, like the
             default button of a native alert. -->
        <button type="button" class="primary" autofocus onclick={saveAndClose}>
          Save
        </button>
        <button type="button" onclick={discardAndClose}>Don't Save</button>
        <button type="button" onclick={() => (showCloseDialog = false)}>
          Cancel
        </button>
      </div>
    </div>
  {/if}
</form>

<style>
  .window {
    /* why relative: the attachments card is positioned against the window. */
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100vh;
    background: var(--bg-window);
  }

  .toolbar {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    flex-shrink: 0;
    /* why 38px: matches --titlebar-inset, so the send icon centers on the
       same axis as the overlay traffic lights on the left. */
    min-height: 38px;
    padding: 0 14px;
    /* Canary's pale periwinkle strip, derived from the accent so it holds
       up in dark mode too. */
    background: color-mix(in srgb, var(--accent) 16%, var(--bg-window));
  }

  .icon {
    display: grid;
    place-items: center;
    padding: 5px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
  }

  .icon:hover {
    background: var(--bg-hover);
  }

  .icon.send {
    color: var(--accent);
  }

  .icon.active {
    background: var(--bg-hover);
    color: var(--accent);
  }

  /* Send-later popover: floats under the toolbar, aligned to its right
     edge, above the envelope fields. */
  .send-later {
    position: absolute;
    top: 44px;
    right: 10px;
    z-index: 10;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px;
    border: 1px solid var(--hairline);
    border-radius: 10px;
    background: var(--bg-window);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18);
  }

  .send-later .preset {
    padding: 6px 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    font: inherit;
    font-size: 13px;
    text-align: left;
    color: var(--text-primary);
    cursor: pointer;
  }

  .send-later .preset:hover {
    background: var(--bg-hover);
  }

  .send-later .custom {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 6px;
    padding-top: 8px;
    border-top: 1px solid var(--hairline);
  }

  .send-later input[type="datetime-local"] {
    flex: none;
    padding: 3px 6px;
    border: 1px solid var(--hairline);
    border-radius: 6px;
    background: transparent;
    font: inherit;
    font-size: 12px;
    color: var(--text-primary);
  }

  .send-later .confirm {
    padding: 4px 10px;
    border: none;
    border-radius: 6px;
    background: var(--accent);
    font: inherit;
    font-size: 12px;
    font-weight: 600;
    color: var(--on-accent);
    cursor: pointer;
  }

  .send-later .preset:disabled,
  .send-later .confirm:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .icon:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .reveal {
    flex-shrink: 0;
    padding: 0;
    border: none;
    background: transparent;
    font: inherit;
    font-size: 12px;
    color: var(--text-secondary);
    cursor: pointer;
  }

  .reveal:hover {
    color: var(--text-primary);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
    margin: 0 20px;
    padding: 10px 0;
    border-bottom: 1px solid var(--hairline);
  }

  .key {
    flex-shrink: 0;
    font-size: 13px;
    color: var(--text-secondary);
  }

  input,
  select {
    border: none;
    padding: 0;
    background: transparent;
    font: inherit;
    font-size: 13px;
    color: var(--text-primary);
  }

  input {
    flex: 1;
  }

  input:focus,
  select:focus {
    outline: none;
  }

  input::placeholder {
    color: var(--text-tertiary);
  }

  /* Borderless account switcher: plain text with a small chevron, like
     Canary's From line. */
  select {
    appearance: none;
    -webkit-appearance: none;
    cursor: pointer;
  }

  .chevron {
    font-size: 11px;
    line-height: 1;
    color: var(--text-secondary);
    transform: translateY(-2px);
  }

  .subject {
    font-weight: 600;
    font-size: 14px;
  }

  /* why relative: anchors the signature popover to this row. */
  .from-row {
    position: relative;
  }

  .sig-toggle {
    display: grid;
    place-items: center;
    margin-left: auto;
    padding: 3px;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
  }

  .sig-toggle:hover,
  .sig-toggle.active {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .sig-toggle:disabled {
    opacity: 0.45;
    cursor: default;
  }

  /* Same floating-panel treatment as the send-later popover. */
  .signature-menu {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    z-index: 10;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 140px;
    padding: 6px;
    border: 1px solid var(--hairline);
    border-radius: 10px;
    background: var(--bg-window);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18);
  }

  .sig-item {
    padding: 5px 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    font: inherit;
    font-size: 13px;
    text-align: left;
    color: var(--text-primary);
    cursor: pointer;
  }

  .sig-item:hover {
    background: var(--bg-hover);
  }

  .sig-item.active {
    font-weight: 600;
    color: var(--accent);
  }

  /* Dashed accent frame over the whole window while a file drag hovers.
     pointer-events: none so it never swallows the drop itself. */
  .window.drop-target::after {
    content: "Drop files to attach";
    position: fixed;
    inset: 8px;
    display: grid;
    place-items: center;
    border: 2px dashed var(--accent);
    border-radius: 10px;
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    font-size: 13px;
    font-weight: 600;
    color: var(--accent);
    pointer-events: none;
  }

  /* Floating panel: hovers over the bottom edge of the body instead of
     occupying a row of the envelope — the compose text flows beneath it. */
  .attachments {
    position: absolute;
    bottom: 14px;
    left: 20px;
    right: 20px;
    display: grid;
    /* why auto-fill + minmax: ~3 cards per row at the default compose
       width, reflowing with the window; a lone card stays card-sized
       because empty tracks still occupy the row. */
    grid-template-columns: repeat(auto-fill, minmax(120px, 1fr));
    gap: 8px;
    margin: 0;
    padding: 10px;
    border: 1px solid var(--hairline);
    border-radius: 12px;
    /* why color-mix + blur: the panel floats over the user's own text, so
       it stays readable without fully hiding what's underneath. */
    background: color-mix(in srgb, var(--bg-window) 82%, transparent);
    backdrop-filter: blur(12px);
    -webkit-backdrop-filter: blur(12px);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.16);
    list-style: none;
  }

  .card {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 4px;
    /* why min-width 0: lets the name ellipsis instead of stretching the
       grid track. */
    min-width: 0;
    padding: 6px;
    border: 1px solid var(--hairline);
    border-radius: 8px;
    background: var(--bg-hover);
    font-size: 12px;
    color: var(--text-primary);
  }

  .thumb {
    width: 100%;
    height: 64px;
    border-radius: 5px;
    object-fit: cover;
  }

  /* Placeholder tile for files without a thumbnail: the extension in caps. */
  .thumb.ext {
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.04em;
    color: var(--text-secondary);
  }

  .card-name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .card-size {
    font-size: 11px;
    color: var(--text-tertiary);
  }

  /* Sits on top of the thumbnail, so it gets a scrim to stay visible over
     busy image corners. */
  .card-remove {
    position: absolute;
    top: 9px;
    right: 9px;
    z-index: 1;
    display: grid;
    place-items: center;
    width: 16px;
    height: 16px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: rgba(0, 0, 0, 0.45);
    font-size: 12px;
    line-height: 1;
    color: #ffffff;
    cursor: pointer;
  }

  .card-remove:hover {
    background: rgba(0, 0, 0, 0.65);
  }

  .card-remove:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .error {
    flex-shrink: 0;
    margin: 0;
    padding: 8px 20px;
    border-bottom: 1px solid var(--hairline);
    font-size: 12px;
    color: var(--danger);
  }

  .body-area {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }

  .quote-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
    margin: 0 20px;
    padding: 8px 0 12px;
    border-top: 1px solid var(--hairline);
  }

  .quote-toggle {
    padding: 1px 9px;
    border: none;
    border-radius: 9px;
    background: var(--bg-hover);
    color: var(--text-secondary);
    font-size: 11px;
    letter-spacing: 0.1em;
    cursor: pointer;
  }

  .quote-attribution {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .quote-remove {
    padding: 0 6px;
    border: none;
    background: transparent;
    color: var(--text-secondary);
    font-size: 14px;
    cursor: pointer;
  }

  .quote-remove:hover,
  .quote-toggle:hover {
    color: var(--text-primary);
  }

  /* Keep the last lines of text visible above the floating panel — cards
     with thumbnails are taller than the old chips. */
  .body-area.with-attachments :global(.tiptap) {
    padding-bottom: 140px;
  }

  /* Dims the whole window while the close prompt decides its fate. */
  .dialog-backdrop {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.18);
  }

  .close-dialog {
    display: flex;
    flex-direction: column;
    gap: 6px;
    width: 250px;
    padding: 16px 14px 14px;
    border: 1px solid var(--hairline);
    border-radius: 12px;
    background: var(--bg-window);
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.28);
    text-align: center;
  }

  .close-dialog h2 {
    margin: 0;
    font-size: 13px;
    font-weight: 700;
    color: var(--text-primary);
  }

  .close-dialog p {
    margin: 0 0 8px;
    font-size: 12px;
    line-height: 1.45;
    color: var(--text-secondary);
  }

  .close-dialog button {
    padding: 6px 10px;
    border: none;
    border-radius: 7px;
    background: var(--bg-hover);
    font: inherit;
    font-size: 13px;
    color: var(--text-primary);
    cursor: pointer;
  }

  .close-dialog button:hover {
    background: color-mix(in srgb, var(--text-primary) 12%, var(--bg-hover));
  }

  .close-dialog button.primary {
    background: var(--accent);
    font-weight: 600;
    color: var(--on-accent);
  }

  .close-dialog button.primary:hover {
    background: color-mix(in srgb, #000000 12%, var(--accent));
  }
</style>
