import {
  cancelSummary,
  newRequestId,
  onTranslationProgress,
  translateMessage,
} from "./api";
import type { SummaryRun } from "./summaries";
import type { MessageBody } from "./types";

/** What a translation row shows: progress while the model works, then the
 *  translated body (or why there is none). */
export interface TranslationState {
  loading: boolean;
  /** Pieces of text translated so far, out of `total` (0 until the first
   *  batch reports). */
  done: number;
  total: number;
  body: MessageBody | null;
  error: string | null;
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
  const base = { body: null, error: null };
  update({ ...base, loading: true, ...progress });
  const unlisten = onTranslationProgress((event) => {
    if (event.requestId !== requestId || settled) return;
    progress = { done: event.done, total: event.total };
    update({ ...base, loading: true, ...progress });
  });
  const done = translateMessage(messageId, loadRemote, requestId)
    .then((body) => {
      if (settled) return;
      settled = true;
      update({ ...base, loading: false, ...progress, body });
    })
    .catch((err: unknown) => {
      if (settled) return;
      settled = true;
      update({ ...base, loading: false, ...progress, error: String(err) });
    })
    .finally(() => {
      void unlisten.then((stop) => stop());
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
