import { describe, expect, it } from "vitest";
import { foldHunk, foldKey, toSplitRows } from "./diffLayout";
import type { DiffLine, DiffLineKind, Hunk } from "../types/models";

const line = (kind: DiffLineKind, content: string): DiffLine => ({
  kind,
  oldLineno: null,
  newLineno: null,
  content,
  noNewline: false,
});

const hunk = (lines: DiffLine[]): Hunk => ({
  header: "@@ -1 +1 @@",
  oldStart: 1,
  oldLines: 0,
  newStart: 1,
  newLines: 0,
  lines,
});

const ctx = (n: number, prefix = "c") => Array.from({ length: n }, (_, i) => line("context", `${prefix}${i}`));

describe("foldHunk", () => {
  it("folds long context runs between changes, keeping 3 lines each side", () => {
    const h = hunk([line("addition", "a"), ...ctx(20), line("deletion", "d")]);
    const items = foldHunk(h, 0, new Set());
    const fold = items.find((i) => i.kind === "fold");
    expect(fold).toMatchObject({ kind: "fold", start: 4, end: 18, count: 14 });
    expect(items).toHaveLength(1 + 3 + 1 + 3 + 1);
  });

  it("does not keep context at hunk edges", () => {
    const h = hunk([...ctx(10), line("addition", "a")]);
    const items = foldHunk(h, 0, new Set());
    expect(items[0]).toMatchObject({ kind: "fold", start: 0, end: 7, count: 7 });
  });

  it("leaves short runs and expanded folds alone", () => {
    const short = hunk([line("addition", "a"), ...ctx(8), line("addition", "b")]);
    expect(foldHunk(short, 0, new Set()).some((i) => i.kind === "fold")).toBe(false);

    const long = hunk([line("addition", "a"), ...ctx(20), line("addition", "b")]);
    const expanded = new Set([foldKey(2, 4)]);
    expect(foldHunk(long, 2, expanded).some((i) => i.kind === "fold")).toBe(false);
  });
});

describe("toSplitRows", () => {
  it("pairs deletion blocks with following additions", () => {
    const h = hunk([
      line("context", "x"),
      line("deletion", "d1"),
      line("deletion", "d2"),
      line("addition", "a1"),
      line("context", "y"),
      line("addition", "a2"),
    ]);
    const rows = toSplitRows(foldHunk(h, 0, new Set()));
    const view = rows.map((r) =>
      r.kind === "pair" ? [r.left?.line.content ?? null, r.right?.line.content ?? null] : "fold",
    );
    expect(view).toEqual([
      ["x", "x"],
      ["d1", "a1"],
      ["d2", null],
      ["y", "y"],
      [null, "a2"],
    ]);
  });
});
