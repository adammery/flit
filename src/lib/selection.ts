// Which message rows the list has selected. Kept pure and separate from
// App.svelte so the modifier-click rules are unit-tested without a DOM —
// same split as messageNav.ts.

export type Selection = {
  /** Every selected row id. */
  ids: number[];
  /** Where a shift-range starts: the row last picked without shift. */
  anchor: number | null;
};

export const EMPTY_SELECTION: Selection = { ids: [], anchor: null };

/** What a click on a row means for the selection. */
export type SelectMode = "replace" | "toggle" | "range";

/**
 * Read a click's modifiers as an intent, so the list maps DOM events and
 * App.svelte maps intents to state — neither has to know the other's half.
 *
 * why ctrl is not read: on a Mac ctrl-click is the right-click, which opens
 * the row menu instead of touching the selection.
 */
export function modeFor(event: {
  metaKey: boolean;
  shiftKey: boolean;
}): SelectMode {
  if (event.shiftKey) return "range";
  if (event.metaKey) return "toggle";
  return "replace";
}

/** Plain click or arrow key — the selection collapses to this row. */
export function selectOne(id: number): Selection {
  return { ids: [id], anchor: id };
}

/**
 * Cmd-click: add the row, or drop it when it was already selected. The row
 * becomes the anchor either way, so a following shift-click spans from
 * wherever the user last clicked.
 *
 * why appended, not sorted into list order: nothing downstream depends on the
 * order (rendering asks "is this id in?", actions run over the whole set), and
 * insertion order keeps this independent of the list currently on screen.
 */
export function toggleId(selection: Selection, id: number): Selection {
  const selected = selection.ids.includes(id);
  return {
    ids: selected
      ? selection.ids.filter((each) => each !== id)
      : [...selection.ids, id],
    anchor: id,
  };
}

/**
 * Shift-click: select the span between the anchor and `id` in the list's
 * current order, inclusive at both ends, and keep the anchor so repeated
 * shift-clicks redraw the span from the same start.
 *
 * The span replaces the selection rather than merging into it — that is what
 * Finder and Apple Mail do. Without a usable anchor it degrades to a plain
 * pick; a row the list doesn't hold leaves the selection alone.
 */
export function extendRange(
  selection: Selection,
  orderedIds: readonly number[],
  id: number,
): Selection {
  const to = orderedIds.indexOf(id);
  if (to === -1) return selection;
  const from =
    selection.anchor === null ? -1 : orderedIds.indexOf(selection.anchor);
  if (from === -1) return selectOne(id);
  const [start, end] = from <= to ? [from, to] : [to, from];
  return {
    ids: orderedIds.slice(start, end + 1),
    anchor: selection.anchor,
  };
}

/**
 * Drop what the list no longer holds — rows evicted by an action, or gone
 * after a folder switch, a search or a refresh. Returns the same object when
 * everything survived, so an assignment can't spuriously re-render.
 */
export function pruneSelection(
  selection: Selection,
  orderedIds: readonly number[],
): Selection {
  const live = new Set(orderedIds);
  const ids = selection.ids.filter((id) => live.has(id));
  const anchor =
    selection.anchor !== null && live.has(selection.anchor)
      ? selection.anchor
      : null;
  if (ids.length === selection.ids.length && anchor === selection.anchor) {
    return selection;
  }
  return { ids, anchor };
}
