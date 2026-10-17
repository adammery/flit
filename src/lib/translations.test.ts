import { beforeEach, expect, it, vi } from "vitest";
import type {
  MessageBody,
  TranslatedBody,
  TranslationPartial,
  TranslationProgress,
} from "./types";

let progressListener: ((progress: TranslationProgress) => void) | null = null;
let partialListener: ((partial: TranslationPartial) => void) | null = null;
let resolveCall: ((body: TranslatedBody) => void) | null = null;
let rejectCall: ((err: Error) => void) | null = null;

vi.mock("./api", () => ({
  newRequestId: vi.fn(() => "req-1"),
  onTranslationProgress: vi.fn(
    async (callback: (progress: TranslationProgress) => void) => {
      progressListener = callback;
      return () => {};
    },
  ),
  onTranslationPartial: vi.fn(
    async (callback: (partial: TranslationPartial) => void) => {
      partialListener = callback;
      return () => {};
    },
  ),
  translateMessage: vi.fn(
    () =>
      new Promise<TranslatedBody>((resolve, reject) => {
        resolveCall = resolve;
        rejectCall = reject;
      }),
  ),
  cancelSummary: vi.fn(async () => undefined),
}));

import * as api from "./api";
import {
  runTranslation,
  translatedView,
  type TranslationState,
} from "./translations";

const translated: TranslatedBody = {
  html: "<p>Ahoj</p>",
  text: "Ahoj",
  subject: "Pozdrav",
};

beforeEach(() => {
  vi.clearAllMocks();
  progressListener = null;
  partialListener = null;
  resolveCall = null;
  rejectCall = null;
});

async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

it("counts its own request's progress, then settles on the translated body", async () => {
  const states: TranslationState[] = [];
  runTranslation(7, false, (state) => states.push(state));
  await settle();

  expect(api.translateMessage).toHaveBeenCalledWith(7, false, "req-1");
  progressListener!({ requestId: "other", done: 9, total: 9 });
  progressListener!({ requestId: "req-1", done: 12, total: 33 });
  resolveCall!(translated);
  await settle();

  const base = { body: null, error: null };
  expect(states).toEqual([
    { ...base, loading: true, done: 0, total: 0 },
    { ...base, loading: true, done: 12, total: 33 },
    { ...base, loading: false, done: 12, total: 33, body: translated },
  ]);
});

it("ends with the error when the translation fails", async () => {
  const states: TranslationState[] = [];
  runTranslation(7, false, (state) => states.push(state));
  rejectCall!(new Error("no model"));
  await settle();

  expect(states.at(-1)).toMatchObject({
    loading: false,
    body: null,
    error: "Error: no model",
  });
});

it("a cancelled run stops the backend and stays silent", async () => {
  const states: TranslationState[] = [];
  const run = runTranslation(7, false, (state) => states.push(state));
  await settle();

  run.cancel();
  progressListener!({ requestId: "req-1", done: 3, total: 9 });
  resolveCall!(translated);
  await run.done;

  expect(api.cancelSummary).toHaveBeenCalledWith("req-1");
  expect(states).toHaveLength(1);
});

it("shows the body translated so far while the model works", async () => {
  const states: TranslationState[] = [];
  runTranslation(7, false, (state) => states.push(state));
  await settle();

  const half: TranslatedBody = {
    html: "<p>Ahoj</p><p>Welt</p>",
    text: null,
    subject: null,
  };
  partialListener!({ requestId: "other", body: translated });
  partialListener!({ requestId: "req-1", body: half });
  progressListener!({ requestId: "req-1", done: 20, total: 40 });

  expect(states.at(-1)).toEqual({
    loading: true,
    done: 20,
    total: 40,
    body: half,
    error: null,
  });
});

it("lays a translation's text over the original body", () => {
  const original: MessageBody = {
    html: "<p>Hallo</p>",
    text: "Hallo",
    quotedText: "> alt",
    blockedImages: 0,
    canLoadRemote: false,
    attachments: [],
    auth: null,
    senderAnomaly: null,
  };

  expect(translatedView(original, translated)).toEqual({
    ...original,
    html: "<p>Ahoj</p>",
    text: "Ahoj",
  });
});
