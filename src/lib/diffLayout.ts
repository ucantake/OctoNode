// Pure layout helpers for the diff viewer (unit-tested, no React).
//
// 1. `foldHunk` collapses long runs of unchanged lines inside a hunk into a
//    single "fold" item (context folding). Useful mainly when the diff was
//    requested with large/full-file context.
// 2. `toSplitRows` pairs deletions with additions for the two-column view.

import type { DiffLine, Hunk } from "../types/models";

export interface FoldOptions {
  /** Context lines kept visible next to each change. */
  keep: number;
  /** Runs hiding fewer lines than this are not folded (a fold that saves two
   * lines is just noise). */
  minHidden: number;
}

export const DEFAULT_FOLD: FoldOptions = { keep: 3, minHidden: 6 };

export type DisplayItem =
  | { kind: "line"; index: number; line: DiffLine }
  | { kind: "fold"; key: string; start: number; end: number; count: number };

export function foldKey(hunkIndex: number, start: number): string {
  return `${hunkIndex}:${start}`;
}

/** Returns the hunk's lines with collapsible context folds. `expanded` holds
 * the keys of folds the user opened. */
export function foldHunk(
  hunk: Hunk,
  hunkIndex: number,
  expanded: ReadonlySet<string>,
  opts: FoldOptions = DEFAULT_FOLD,
): DisplayItem[] {
  const out: DisplayItem[] = [];
  const lines = hunk.lines;
  let i = 0;
  while (i < lines.length) {
    if (lines[i].kind !== "context") {
      out.push({ kind: "line", index: i, line: lines[i] });
      i++;
      continue;
    }
    let j = i;
    while (j < lines.length && lines[j].kind === "context") j++;
    // Run [i, j). Edges of the hunk need no leading/trailing context.
    const keepTop = i === 0 ? 0 : opts.keep;
    const keepBottom = j === lines.length ? 0 : opts.keep;
    const hiddenStart = i + keepTop;
    const hiddenEnd = j - keepBottom;
    const hidden = hiddenEnd - hiddenStart;
    const key = foldKey(hunkIndex, hiddenStart);

    if (hidden >= opts.minHidden && !expanded.has(key)) {
      for (let k = i; k < hiddenStart; k++) out.push({ kind: "line", index: k, line: lines[k] });
      out.push({ kind: "fold", key, start: hiddenStart, end: hiddenEnd, count: hidden });
      for (let k = hiddenEnd; k < j; k++) out.push({ kind: "line", index: k, line: lines[k] });
    } else {
      for (let k = i; k < j; k++) out.push({ kind: "line", index: k, line: lines[k] });
    }
    i = j;
  }
  return out;
}

export interface SplitCell {
  index: number;
  line: DiffLine;
}

export type SplitRow =
  | { kind: "pair"; left: SplitCell | null; right: SplitCell | null }
  | { kind: "fold"; key: string; count: number };

/** Two-column layout: context on both sides, each deletion block paired
 * line-by-line with the addition block that follows it. */
export function toSplitRows(items: DisplayItem[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let dels: SplitCell[] = [];
  let adds: SplitCell[] = [];

  const flush = () => {
    const n = Math.max(dels.length, adds.length);
    for (let k = 0; k < n; k++) {
      rows.push({ kind: "pair", left: dels[k] ?? null, right: adds[k] ?? null });
    }
    dels = [];
    adds = [];
  };

  for (const item of items) {
    if (item.kind === "fold") {
      flush();
      rows.push({ kind: "fold", key: item.key, count: item.count });
      continue;
    }
    const cell = { index: item.index, line: item.line };
    switch (item.line.kind) {
      case "deletion":
        // A deletion after additions starts a new change block.
        if (adds.length > 0) flush();
        dels.push(cell);
        break;
      case "addition":
        adds.push(cell);
        break;
      case "context":
        flush();
        rows.push({ kind: "pair", left: cell, right: cell });
        break;
    }
  }
  flush();
  return rows;
}

/** Hunk lines that can be (un)staged individually. */
export function isSelectable(line: DiffLine): boolean {
  return line.kind !== "context";
}
