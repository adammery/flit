import { fireEvent, render, screen, within } from "@testing-library/svelte";
import { beforeEach, expect, it, vi } from "vitest";
import type { LlmDownloadProgress, LlmStatus } from "./types";

let enabled = false;
let translateEnabled = false;
let status: LlmStatus;
let progressListener: ((progress: LlmDownloadProgress) => void) | null = null;
let changedListener: (() => void) | null = null;

vi.mock("./api", () => ({
  getLlmSummaryEnabled: vi.fn(async () => enabled),
  setLlmSummaryEnabled: vi.fn(async () => undefined),
  getLlmTranslateEnabled: vi.fn(async () => translateEnabled),
  setLlmTranslateEnabled: vi.fn(async () => undefined),
  llmStatus: vi.fn(async () => status),
  getLlmSummaryLanguage: vi.fn(async () => "auto"),
  setLlmSummaryLanguage: vi.fn(async () => undefined),
  getLlmTranslateLanguage: vi.fn(async () => "English"),
  setLlmTranslateLanguage: vi.fn(async () => undefined),
  setLlmModel: vi.fn(async () => undefined),
  downloadLlmModel: vi.fn(async () => undefined),
  cancelLlmDownload: vi.fn(async () => undefined),
  removeLlmModel: vi.fn(async () => undefined),
  onLlmDownloadProgress: vi.fn(
    async (callback: (progress: LlmDownloadProgress) => void) => {
      progressListener = callback;
      return () => {};
    },
  ),
  onLlmModelsChanged: vi.fn(async (callback: () => void) => {
    changedListener = callback;
    return () => {};
  }),
}));

import * as api from "./api";
import ExperimentalPane from "./ExperimentalPane.svelte";

beforeEach(() => {
  vi.clearAllMocks();
  progressListener = null;
  changedListener = null;
  enabled = false;
  translateEnabled = false;
  status = {
    enabled: false,
    translateEnabled: false,
    activeModel: null,
    ready: false,
    translateReady: false,
    models: [
      {
        id: "small",
        name: "Small 2B",
        description: "Fast and small.",
        size: 1_280_835_840,
        recommended: true,
        state: { kind: "missing" },
      },
      {
        id: "big",
        name: "Big 4B",
        description: "Better summaries.",
        size: 2_740_937_888,
        recommended: false,
        state: { kind: "ready" },
      },
    ],
  };
});

it("shows on-device summaries switched off by default", async () => {
  render(ExperimentalPane);

  expect(
    await screen.findByLabelText("Summarize messages with a local AI model"),
  ).not.toBeChecked();
});

it("saves switching on-device summaries on", async () => {
  render(ExperimentalPane);
  const box = await screen.findByLabelText(
    "Summarize messages with a local AI model",
  );

  await fireEvent.click(box);

  expect(api.setLlmSummaryEnabled).toHaveBeenCalledWith(true);
});

it("rolls the switch back when saving fails", async () => {
  vi.mocked(api.setLlmSummaryEnabled).mockRejectedValueOnce(new Error("disk"));
  render(ExperimentalPane);
  const box = await screen.findByLabelText(
    "Summarize messages with a local AI model",
  );

  await fireEvent.click(box);

  expect(await screen.findByRole("alert")).toHaveTextContent("disk");
  expect(box).not.toBeChecked();
});

it("keeps the model picker hidden while the feature is off", async () => {
  render(ExperimentalPane);
  await screen.findByLabelText("Summarize messages with a local AI model");

  expect(screen.queryByText("Small 2B")).toBeNull();
});

it("lists the catalog with sizes and states once switched on", async () => {
  enabled = true;
  status.enabled = true;
  render(ExperimentalPane);

  const small = (await screen.findByText("Small 2B")).closest("li")!;
  expect(within(small).getByText("1.3 GB")).toBeInTheDocument();
  expect(within(small).getByText("Recommended")).toBeInTheDocument();
  expect(within(small).getByText("Not downloaded")).toBeInTheDocument();

  const big = screen.getByText("Big 4B").closest("li")!;
  expect(within(big).getByText("2.7 GB")).toBeInTheDocument();
  expect(within(big).getByText("Downloaded")).toBeInTheDocument();
});

it("lets only a downloaded model be picked as the one in use", async () => {
  enabled = true;
  status.enabled = true;
  render(ExperimentalPane);
  await screen.findByText("Small 2B");

  expect(screen.getByLabelText("Use Small 2B")).toBeDisabled();
  const big = screen.getByLabelText("Use Big 4B");
  expect(big).toBeEnabled();

  await fireEvent.click(big);

  expect(api.setLlmModel).toHaveBeenCalledWith("big");
});

it("marks the picked model as in use", async () => {
  enabled = true;
  status.enabled = true;
  status.activeModel = "big";
  render(ExperimentalPane);

  expect(await screen.findByLabelText("Use Big 4B")).toBeChecked();
});

it("starts a download from the card of a missing model", async () => {
  enabled = true;
  status.enabled = true;
  render(ExperimentalPane);
  const small = (await screen.findByText("Small 2B")).closest("li")!;

  await fireEvent.click(within(small).getByRole("button", { name: "Download" }));

  expect(api.downloadLlmModel).toHaveBeenCalledWith("small");
});

it("shows progress for a download in flight and lets it be cancelled", async () => {
  enabled = true;
  status.enabled = true;
  status.models[0].state = {
    kind: "downloading",
    received: 640_417_920,
    total: 1_280_835_840,
  };
  render(ExperimentalPane);
  const small = (await screen.findByText("Small 2B")).closest("li")!;

  expect(within(small).getByRole("progressbar")).toHaveAttribute("value", "640417920");
  expect(within(small).getByText("640 MB of 1.3 GB")).toBeInTheDocument();
  expect(within(small).queryByRole("button", { name: "Download" })).toBeNull();

  await fireEvent.click(within(small).getByRole("button", { name: "Cancel" }));

  expect(api.cancelLlmDownload).toHaveBeenCalledWith("small");
});

it("moves the bar on progress events without re-reading status", async () => {
  enabled = true;
  status.enabled = true;
  status.models[0].state = { kind: "downloading", received: 0, total: 100 };
  render(ExperimentalPane);
  const small = (await screen.findByText("Small 2B")).closest("li")!;
  vi.mocked(api.llmStatus).mockClear();

  progressListener!({ id: "small", received: 42, total: 100 });

  expect(
    await within(small).findByText("42 B of 100 B"),
  ).toBeInTheDocument();
  expect(api.llmStatus).not.toHaveBeenCalled();
});

it("re-reads status when a download ends", async () => {
  enabled = true;
  status.enabled = true;
  render(ExperimentalPane);
  await screen.findByText("Small 2B");
  status.models[0].state = { kind: "ready" };

  changedListener!();

  const small = (await screen.findByText("Small 2B")).closest("li")!;
  expect(await within(small).findByText("Downloaded")).toBeInTheDocument();
});

it("shows why a download failed and offers a retry", async () => {
  enabled = true;
  status.enabled = true;
  status.models[0].state = { kind: "failed", error: "download error: server answered 503" };
  render(ExperimentalPane);
  const small = (await screen.findByText("Small 2B")).closest("li")!;

  expect(within(small).getByText("download error: server answered 503")).toBeInTheDocument();
  await fireEvent.click(within(small).getByRole("button", { name: "Retry" }));

  expect(api.downloadLlmModel).toHaveBeenCalledWith("small");
});

it("removes a downloaded model from its card", async () => {
  enabled = true;
  status.enabled = true;
  render(ExperimentalPane);
  const big = (await screen.findByText("Big 4B")).closest("li")!;

  await fireEvent.click(within(big).getByRole("button", { name: "Remove" }));

  expect(api.removeLlmModel).toHaveBeenCalledWith("big");
});

it("offers the summary language once switched on, auto by default", async () => {
  enabled = true;
  status.enabled = true;
  render(ExperimentalPane);

  const select = await screen.findByLabelText("Summary language");
  expect(select).toHaveValue("auto");

  await fireEvent.change(select, { target: { value: "Slovak" } });

  expect(api.setLlmSummaryLanguage).toHaveBeenCalledWith("Slovak");
});

it("shows the stored summary language", async () => {
  enabled = true;
  status.enabled = true;
  vi.mocked(api.getLlmSummaryLanguage).mockResolvedValueOnce("German");
  render(ExperimentalPane);

  expect(await screen.findByLabelText("Summary language")).toHaveValue("German");
});

it("saves switching translation on apart from summaries", async () => {
  render(ExperimentalPane);
  const box = await screen.findByLabelText(
    "Translate messages with a local AI model",
  );
  expect(box).not.toBeChecked();

  await fireEvent.click(box);

  expect(api.setLlmTranslateEnabled).toHaveBeenCalledWith(true);
  expect(api.setLlmSummaryEnabled).not.toHaveBeenCalled();
});

it("offers the model picker for translation alone", async () => {
  translateEnabled = true;
  status.translateEnabled = true;
  render(ExperimentalPane);

  expect(await screen.findByText("Small 2B")).toBeInTheDocument();
  expect(screen.getByLabelText("Translate messages into")).toBeInTheDocument();
  expect(screen.queryByLabelText("Summary language")).toBeNull();
});

it("stores the language messages are translated into", async () => {
  translateEnabled = true;
  status.translateEnabled = true;
  vi.mocked(api.getLlmTranslateLanguage).mockResolvedValueOnce("Slovak");
  render(ExperimentalPane);

  const select = await screen.findByLabelText("Translate messages into");
  expect(select).toHaveValue("Slovak");
  // A translation needs a target: no "same as the message" here.
  expect(
    Array.from((select as HTMLSelectElement).options).map((o) => o.value),
  ).not.toContain("auto");

  await fireEvent.change(select, { target: { value: "German" } });

  expect(api.setLlmTranslateLanguage).toHaveBeenCalledWith("German");
});
