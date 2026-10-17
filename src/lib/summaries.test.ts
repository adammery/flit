import { beforeEach, expect, it, vi } from "vitest";
import type { SummaryToken } from "./types";

let tokenListener: ((token: SummaryToken) => void) | null = null;
let resolveCall: ((text: string) => void) | null = null;
let rejectCall: ((err: Error) => void) | null = null;

vi.mock("./api", () => ({
  newRequestId: vi.fn(() => "req-1"),
  onSummaryToken: vi.fn(async (callback: (token: SummaryToken) => void) => {
    tokenListener = callback;
    return () => {};
  }),
  summarizeMessage: vi.fn(
    () =>
      new Promise<string>((resolve, reject) => {
        resolveCall = resolve;
        rejectCall = reject;
      }),
  ),
  summarizeThread: vi.fn(async () => "- thread"),
  cancelSummary: vi.fn(async () => undefined),
}));

import * as api from "./api";
import {
  isCollapsed,
  runSummary,
  setCollapsed,
  type SummaryState,
} from "./summaries";

beforeEach(() => {
  vi.clearAllMocks();
  tokenListener = null;
  resolveCall = null;
  rejectCall = null;
});

async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

it("streams pieces of its own request, then settles on the full text", async () => {
  const states: SummaryState[] = [];
  runSummary("message", 7, false, (state) => states.push(state));
  await settle();

  expect(api.summarizeMessage).toHaveBeenCalledWith(7, false, "req-1");
  tokenListener!({ requestId: "other", text: "nope" });
  tokenListener!({ requestId: "req-1", text: "- Al" });
  tokenListener!({ requestId: "req-1", text: "ice asks" });
  resolveCall!("- Alice asks");
  await settle();

  expect(states).toEqual([
    { loading: true, text: null, error: null },
    { loading: true, text: "- Al", error: null },
    { loading: true, text: "- Alice asks", error: null },
    { loading: false, text: "- Alice asks", error: null },
  ]);
});

it("reports a failure", async () => {
  const states: SummaryState[] = [];
  runSummary("message", 7, true, (state) => states.push(state));
  await settle();
  rejectCall!(new Error("local model error: cannot load model"));
  await settle();

  expect(api.summarizeMessage).toHaveBeenCalledWith(7, true, "req-1");
  expect(states.at(-1)).toEqual({
    loading: false,
    text: null,
    error: "Error: local model error: cannot load model",
  });
});

it("cancelling stops the backend and silences later updates", async () => {
  const states: SummaryState[] = [];
  const run = runSummary("message", 7, false, (state) => states.push(state));
  await settle();

  run.cancel();
  tokenListener!({ requestId: "req-1", text: "late" });
  rejectCall!(new Error("Cancelled."));
  await settle();

  expect(api.cancelSummary).toHaveBeenCalledWith("req-1");
  expect(states).toEqual([{ loading: true, text: null, error: null }]);
});

it("summarizes a conversation through the thread command", async () => {
  const states: SummaryState[] = [];
  await runSummary("thread", 3, false, (state) => states.push(state)).done;

  expect(api.summarizeThread).toHaveBeenCalledWith(3, false, "req-1");
  expect(states.at(-1)).toEqual({ loading: false, text: "- thread", error: null });
});

it("remembers which panels were folded", () => {
  expect(isCollapsed("m1")).toBe(false);
  setCollapsed("m1", true);
  expect(isCollapsed("m1")).toBe(true);
  expect(isCollapsed("m2")).toBe(false);
  setCollapsed("m1", false);
  expect(isCollapsed("m1")).toBe(false);
});
