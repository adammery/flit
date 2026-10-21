import { describe, expect, it } from "vitest";
import {
  EMPTY_SELECTION,
  extendRange,
  modeFor,
  pruneSelection,
  type Selection,
  selectOne,
  toggleId,
} from "./selection";

describe("modeFor", () => {
  const plain = { metaKey: false, shiftKey: false };

  it("reads a bare click as replacing the selection", () => {
    expect(modeFor(plain)).toBe("replace");
  });

  it("reads cmd as toggling one row", () => {
    expect(modeFor({ ...plain, metaKey: true })).toBe("toggle");
  });

  it("reads shift as extending a range", () => {
    expect(modeFor({ ...plain, shiftKey: true })).toBe("range");
  });

  it("lets shift win over cmd when both are held", () => {
    expect(modeFor({ ...plain, metaKey: true, shiftKey: true })).toBe("range");
  });
});

describe("selectOne", () => {
  it("collapses the selection to the one row and anchors there", () => {
    expect(selectOne(20)).toEqual({ ids: [20], anchor: 20 });
  });
});

describe("toggleId", () => {
  it("adds a row that wasn't selected and anchors on it", () => {
    expect(toggleId(selectOne(10), 30)).toEqual({
      ids: [10, 30],
      anchor: 30,
    });
  });

  it("drops a row that was already selected", () => {
    const selection: Selection = { ids: [10, 20, 30], anchor: 10 };
    expect(toggleId(selection, 20)).toEqual({ ids: [10, 30], anchor: 20 });
  });

  it("can empty the selection", () => {
    expect(toggleId(selectOne(10), 10)).toEqual({ ids: [], anchor: 10 });
  });

  it("works from an empty selection", () => {
    expect(toggleId(EMPTY_SELECTION, 10)).toEqual({ ids: [10], anchor: 10 });
  });
});

describe("extendRange", () => {
  const ids = [10, 20, 30, 40];

  it("spans from the anchor down to the clicked row, inclusive", () => {
    expect(extendRange(selectOne(20), ids, 40)).toEqual({
      ids: [20, 30, 40],
      anchor: 20,
    });
  });

  it("spans upwards just as well, still in list order", () => {
    expect(extendRange(selectOne(30), ids, 10)).toEqual({
      ids: [10, 20, 30],
      anchor: 30,
    });
  });

  it("keeps the anchor so a second shift-click re-extends from it", () => {
    const first = extendRange(selectOne(20), ids, 40);
    expect(extendRange(first, ids, 30)).toEqual({
      ids: [20, 30],
      anchor: 20,
    });
  });

  it("selects just the clicked row when the range collapses", () => {
    expect(extendRange(selectOne(20), ids, 20)).toEqual({
      ids: [20],
      anchor: 20,
    });
  });

  it("falls back to a plain pick when there is no anchor yet", () => {
    expect(extendRange(EMPTY_SELECTION, ids, 30)).toEqual({
      ids: [30],
      anchor: 30,
    });
  });

  it("falls back to a plain pick when the anchor left the list", () => {
    const selection: Selection = { ids: [99], anchor: 99 };
    expect(extendRange(selection, ids, 30)).toEqual({
      ids: [30],
      anchor: 30,
    });
  });

  it("leaves the selection alone when the clicked row isn't in the list", () => {
    const selection = selectOne(20);
    expect(extendRange(selection, ids, 99)).toEqual(selection);
  });

  // Replacing rather than merging matches Finder and Apple Mail: shift-click
  // redraws the span, it doesn't accumulate one.
  it("replaces an existing selection instead of merging into it", () => {
    const selection: Selection = { ids: [10, 40], anchor: 40 };
    expect(extendRange(selection, ids, 20)).toEqual({
      ids: [20, 30, 40],
      anchor: 40,
    });
  });
});

describe("pruneSelection", () => {
  const ids = [10, 20, 30];

  it("drops rows the list no longer holds", () => {
    const selection: Selection = { ids: [10, 99, 30], anchor: 10 };
    expect(pruneSelection(selection, ids)).toEqual({
      ids: [10, 30],
      anchor: 10,
    });
  });

  it("clears an anchor that left the list", () => {
    const selection: Selection = { ids: [10], anchor: 99 };
    expect(pruneSelection(selection, ids)).toEqual({ ids: [10], anchor: null });
  });

  it("returns the same object when nothing changed", () => {
    const selection: Selection = { ids: [10, 30], anchor: 10 };
    expect(pruneSelection(selection, ids)).toBe(selection);
  });

  it("empties the selection when none of its rows survive", () => {
    const selection: Selection = { ids: [98, 99], anchor: 98 };
    expect(pruneSelection(selection, ids)).toEqual({ ids: [], anchor: null });
  });
});
