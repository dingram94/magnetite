import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { send, splitIds, type UiState } from "./bridge";

/** How a click chooses: one row more or less, or all rows from the last one chosen to this one. */
export type SelectMode = "toggle" | "range";

interface Picked {
  /** The space the rows are of: a selection is gone with a change of space. */
  space: string;
  ids: string[];
  /** Where a range starts: the row chosen last by itself. */
  anchor: string | null;
}

const NONE: Picked = { space: "", ids: [], anchor: null };

/**
 * The Today tabs chosen together in the sidebar, for one action on all of
 * them (`selectionMenu`, and the key that closes a tab). Only rows of
 * their own count: a split is one row for several tabs and is left out.
 *
 * A selection lasts while the sidebar has the keyboard. A click into a
 * page, another app or Esc ends it, so the key that closes a tab never
 * meets one the user no longer looks at. The app is told each change: the
 * key is its own (`select_tabs`).
 */
export function useTabSelection(state: UiState | null) {
  const [picked, setPicked] = useState<Picked>(NONE);
  const space = state ? (state.sidebar.spaces.find((s) => s.id === state.active_space) ?? state.sidebar.spaces[0]) : undefined;
  const rows = useMemo(() => {
    if (!state || !space) return [];
    const inSplit = new Set(state.splits.flatMap(splitIds));
    return space.today.filter((t) => !inSplit.has(t.id)).map((t) => t.id);
  }, [state, space]);
  const ids = useMemo(() => (picked.space === space?.id ? rows.filter((id) => picked.ids.includes(id)) : []), [picked, rows, space?.id]);
  // The same set until the chosen tabs change: the rows redraw with it.
  const key = ids.join("\n");
  const selected = useMemo(() => new Set(key ? key.split("\n") : []), [key]);

  // The newest of what a click chooses from: `select` stays the same function.
  const now = useRef({ rows, ids, anchor: picked.anchor, space: space?.id ?? "", front: state?.active_tab ?? null });
  now.current = { rows, ids, anchor: picked.anchor, space: space?.id ?? "", front: state?.active_tab ?? null };

  const clear = useCallback(() => setPicked((p) => (p.ids.length > 0 ? NONE : p)), []);
  const select = useCallback((id: string, mode: SelectMode) => {
    const { rows, ids, anchor, space, front } = now.current;
    if (!rows.includes(id)) return;
    // A click on a row gives the sidebar the keyboard on most systems; ask, for the others.
    send({ cmd: "focus_sidebar" });
    if (mode === "toggle") {
      setPicked({ space, ids: ids.includes(id) ? ids.filter((x) => x !== id) : [...ids, id], anchor: id });
      return;
    }
    // From the row chosen last, else from the tab in front.
    const from = [anchor, front].find((x): x is string => !!x && rows.includes(x)) ?? id;
    const [a, b] = [rows.indexOf(from), rows.indexOf(id)].sort((x, y) => x - y) as [number, number];
    setPicked({ space, ids: rows.slice(a, b + 1), anchor: from });
  }, []);

  const told = useRef("");
  useEffect(() => {
    if (key === told.current) return;
    told.current = key;
    send({ cmd: "select_tabs", ids: key ? key.split("\n") : [] });
  }, [key]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && clear();
    window.addEventListener("blur", clear);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("blur", clear);
      window.removeEventListener("keydown", onKey);
    };
  }, [clear]);

  return { selected, select, clear };
}
