import {
  cancelSummary,
  newRequestId,
  onTranslationPartial,
  onTranslationProgress,
  translateMessage,
} from "./api";
import type { SummaryRun } from "./summaries";
import type { MessageBody, TranslatedBody } from "./types";

/** What a translation row shows: progress while the model works, then the
 *  translated body (or why there is none). The body grows batch by batch
 *  while `loading`. */
export interface TranslationState {
  loading: boolean;
  /** Pieces of text translated so far, out of `total` (0 until the first
   *  batch reports). */
  done: number;
  total: number;
  body: TranslatedBody | null;
  error: string | null;
}

/** The original body with a translation's text shown in its place. */
export function translatedView(
  original: MessageBody,
  translated: TranslatedBody,
): MessageBody {
  return { ...original, html: translated.html, text: translated.text };
}

/**
 * Translate one message, feeding `update` with progress and then once more
 * with the translated body (a cache hit skips straight to that). A
 * cancelled run sends no further updates — the caller has dropped its row.
 */
export function runTranslation(
  messageId: number,
  loadRemote: boolean,
  update: (state: TranslationState) => void,
): SummaryRun {
  const requestId = newRequestId();
  let settled = false;
  let progress = { done: 0, total: 0 };
  let body: TranslatedBody | null = null;
  const working = () =>
    update({ loading: true, ...progress, body, error: null });
  working();
  const unlistenProgress = onTranslationProgress((event) => {
    if (event.requestId !== requestId || settled) return;
    progress = { done: event.done, total: event.total };
    working();
  });
  const unlistenPartial = onTranslationPartial((event) => {
    if (event.requestId !== requestId || settled) return;
    body = event.body;
    working();
  });
  const done = translateMessage(messageId, loadRemote, requestId)
    .then((full) => {
      if (settled) return;
      settled = true;
      update({ loading: false, ...progress, body: full, error: null });
    })
    .catch((err: unknown) => {
      if (settled) return;
      settled = true;
      // A failed run shows the original again, with the reason.
      update({ loading: false, ...progress, body: null, error: String(err) });
    })
    .finally(() => {
      void unlistenProgress.then((stop) => stop());
      void unlistenPartial.then((stop) => stop());
    });
  return {
    cancel: () => {
      if (settled) return;
      settled = true;
      void cancelSummary(requestId);
    },
    done,
  };
}
