import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import clsx from "clsx";
import { ErrorAction, ErrorText } from "./ErrorText";
import { useHotkeys } from "../hooks/useHotkeys";
import { DEFAULT_FOLD, foldHunk, isSelectable, toSplitRows, type SplitCell } from "../lib/diffLayout";
import { api, errorMessage, IpcError } from "../lib/ipc";
import { formatShortcut } from "../lib/platform";
import type {
  DiffLine,
  DiffResult,
  DiffTarget,
  FileDiff,
  FileStatus,
  Hunk,
  HunkSelection,
  Uuid,
} from "../types/models";

export type DiffMode = "unified" | "split";

export interface DiffViewerProps {
  repoId: Uuid;
  target: DiffTarget;
  /** Fired after a successful stage/unstage so sibling views can refresh. */
  onChanged?: () => void;
  /** Bump to force a reload (e.g. after a fetch or external change). */
  reloadToken?: number;
}

const FULL_CONTEXT = 1_000_000;
const DEFAULT_CONTEXT = 3;
const MODE_STORAGE_KEY = "octonode.diff.mode";
const TOGGLE_MODE_SHORTCUT = "Alt+V";

// Fixed row heights → the virtualizer never has to measure the DOM.
const HEIGHT = { file: 36, hunk: 28, line: 20, fold: 24, notice: 32 } as const;

type Row =
  | { t: "file"; fileIndex: number; file: FileDiff }
  | { t: "hunk"; fileIndex: number; hunkIndex: number; hunk: Hunk }
  | { t: "line"; fileIndex: number; hunkIndex: number; index: number; line: DiffLine }
  | { t: "split"; fileIndex: number; hunkIndex: number; left: SplitCell | null; right: SplitCell | null }
  | { t: "fold"; fileIndex: number; key: string; count: number }
  | { t: "notice"; fileIndex: number; text: string };

function readStoredMode(): DiffMode {
  try {
    return localStorage.getItem(MODE_STORAGE_KEY) === "split" ? "split" : "unified";
  } catch {
    return "unified";
  }
}

function filePath(f: FileDiff): string {
  return f.newPath ?? f.oldPath ?? "";
}

const selKey = (fileIndex: number, hunkIndex: number) => `${fileIndex}:${hunkIndex}`;

/**
 * Unified / split diff viewer with context folding and hunk/line staging.
 *
 * Hunks arrive fully structured from Rust (git2). Staging sends hunk/line
 * *indices* plus the hunk header; the backend re-derives the exact bytes and
 * rejects the request as `stale` if the file changed in the meantime.
 */
export function DiffViewer({ repoId, target, onChanged, reloadToken = 0 }: DiffViewerProps) {
  const [mode, setMode] = useState<DiffMode>(readStoredMode);
  const [fullContext, setFullContext] = useState(false);
  const [ignoreWhitespace, setIgnoreWhitespace] = useState(false);
  const [result, setResult] = useState<DiffResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [expandedFolds, setExpandedFolds] = useState<ReadonlySet<string>>(new Set());
  const [collapsedFiles, setCollapsedFiles] = useState<ReadonlySet<number>>(new Set());
  const [selection, setSelection] = useState<ReadonlyMap<string, ReadonlySet<number>>>(new Map());

  const stageAction = target.type === "workingTree" ? "stage" : target.type === "index" ? "unstage" : null;
  // Whitespace-insensitive hunks do not map 1:1 onto file bytes.
  const canStageLines = stageAction !== null && !ignoreWhitespace;
  const targetKey = target.type === "commit" ? `commit:${target.id}` : target.type;

  // ---- Loading --------------------------------------------------------------
  const request = useRef(0);
  const load = useCallback(async () => {
    const id = ++request.current;
    setError(null);
    try {
      const res = await api.getDiff(
        repoId,
        target,
        fullContext ? FULL_CONTEXT : DEFAULT_CONTEXT,
        ignoreWhitespace,
      );
      if (id !== request.current) return; // a newer request superseded this one
      setResult(res);
      setSelection(new Map());
    } catch (e) {
      if (id === request.current) setError(errorMessage(e));
    }
    // Keyed on `targetKey`, not `target`: callers may pass a fresh object
    // literal every render, and only its value matters.
  }, [repoId, targetKey, fullContext, ignoreWhitespace]);

  useEffect(() => {
    void load();
  }, [load, reloadToken]);

  useEffect(() => {
    setExpandedFolds(new Set());
    setCollapsedFiles(new Set());
  }, [repoId, targetKey]);

  const toggleMode = useCallback(() => {
    setMode((m) => {
      const next = m === "unified" ? "split" : "unified";
      try {
        localStorage.setItem(MODE_STORAGE_KEY, next);
      } catch {
        /* storage unavailable: preference is per-session only */
      }
      return next;
    });
  }, []);
  useHotkeys({ [TOGGLE_MODE_SHORTCUT]: toggleMode });

  // ---- Flatten into virtual rows -------------------------------------------
  const rows = useMemo<Row[]>(() => {
    if (!result) return [];
    const out: Row[] = [];
    result.files.forEach((file, fileIndex) => {
      out.push({ t: "file", fileIndex, file });
      if (collapsedFiles.has(fileIndex)) return;
      if (file.isBinary) {
        out.push({ t: "notice", fileIndex, text: "Binary file not shown" });
        return;
      }
      if (file.hunks.length === 0) {
        out.push({ t: "notice", fileIndex, text: "No textual changes (mode or rename only)" });
        return;
      }
      file.hunks.forEach((hunk, hunkIndex) => {
        out.push({ t: "hunk", fileIndex, hunkIndex, hunk });
        const items = foldHunk(hunk, hunkIndex, foldSet(expandedFolds, fileIndex), DEFAULT_FOLD);
        if (mode === "unified") {
          for (const item of items) {
            if (item.kind === "fold") {
              out.push({ t: "fold", fileIndex, key: `${fileIndex}/${item.key}`, count: item.count });
            } else {
              out.push({ t: "line", fileIndex, hunkIndex, index: item.index, line: item.line });
            }
          }
        } else {
          for (const r of toSplitRows(items)) {
            if (r.kind === "fold") {
              out.push({ t: "fold", fileIndex, key: `${fileIndex}/${r.key}`, count: r.count });
            } else {
              out.push({ t: "split", fileIndex, hunkIndex, left: r.left, right: r.right });
            }
          }
        }
      });
      if (file.truncated) {
        out.push({ t: "notice", fileIndex, text: "Diff truncated — file is too large to show in full" });
      }
    });
    return out;
  }, [result, mode, expandedFolds, collapsedFiles]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => HEIGHT[rows[i].t === "split" ? "line" : rows[i].t],
    overscan: 30,
  });
  // Row heights depend on row *types*; re-measure when the list changes.
  useEffect(() => virtualizer.measure(), [rows, virtualizer]);

  // ---- Selection & staging ---------------------------------------------------
  const toggleLine = (fileIndex: number, hunkIndex: number, index: number) => {
    setSelection((prev) => {
      const key = selKey(fileIndex, hunkIndex);
      const next = new Map(prev);
      const set = new Set(next.get(key) ?? []);
      if (set.has(index)) set.delete(index);
      else set.add(index);
      if (set.size === 0) next.delete(key);
      else next.set(key, set);
      return next;
    });
  };

  const runStage = async (file: FileDiff, hunks: HunkSelection[]) => {
    if (!stageAction || !result) return;
    setBusy(true);
    setNotice(null);
    try {
      await api.stageChanges({
        repoId,
        path: filePath(file),
        action: stageAction,
        contextLines: result.contextLines,
        hunks,
      });
      onChanged?.();
    } catch (e) {
      setNotice(
        e instanceof IpcError && e.kind === "stale"
          ? "The file changed on disk — the diff was refreshed, please retry."
          : errorMessage(e),
      );
    } finally {
      setBusy(false);
      await load();
    }
  };

  const stageSelectedLines = (fileIndex: number, hunkIndex: number) => {
    const file = result?.files[fileIndex];
    const lines = selection.get(selKey(fileIndex, hunkIndex));
    if (!file || !lines || lines.size === 0) return;
    void runStage(file, [
      { hunkIndex, header: file.hunks[hunkIndex].header, lines: [...lines].sort((a, b) => a - b) },
    ]);
  };

  // ---- Render -----------------------------------------------------------------
  const verb = stageAction === "stage" ? "Stage" : "Unstage";
  const totals = useMemo(
    () =>
      result?.files.reduce((acc, f) => ({ add: acc.add + f.additions, del: acc.del + f.deletions }), {
        add: 0,
        del: 0,
      }) ?? { add: 0, del: 0 },
    [result],
  );

  const renderRow = (row: Row) => {
    switch (row.t) {
      case "file":
        return (
          <FileHeader
            file={row.file}
            collapsed={collapsedFiles.has(row.fileIndex)}
            onToggle={() =>
              setCollapsedFiles((prev) => toggled(prev, row.fileIndex))
            }
            action={stageAction ? `${verb} file` : null}
            busy={busy}
            onAction={() => void runStage(row.file, [])}
          />
        );
      case "hunk": {
        const file = result?.files[row.fileIndex];
        const partialOk = canStageLines && file && isLineStageable(file);
        const selected = selection.get(selKey(row.fileIndex, row.hunkIndex))?.size ?? 0;
        return (
          <div className="flex h-full items-center gap-2 border-y border-line/60 bg-surface-1 px-3 font-mono text-xs text-sky-300/80">
            <span className="truncate">{row.hunk.header}</span>
            <span className="ml-auto flex gap-1.5 font-sans">
              {partialOk && selected > 0 && (
                <HunkButton disabled={busy} onClick={() => stageSelectedLines(row.fileIndex, row.hunkIndex)}>
                  {verb} {selected} line{selected === 1 ? "" : "s"}
                </HunkButton>
              )}
              {partialOk && (
                <HunkButton
                  disabled={busy}
                  onClick={() =>
                    file &&
                    void runStage(file, [{ hunkIndex: row.hunkIndex, header: row.hunk.header, lines: null }])
                  }
                >
                  {verb} hunk
                </HunkButton>
              )}
            </span>
          </div>
        );
      }
      case "fold":
        return (
          <button
            type="button"
            onClick={() => setExpandedFolds((prev) => toggled(prev, row.key))}
            className="flex h-full w-full items-center gap-2 bg-surface-1/60 px-3 text-left text-xs text-fg-muted hover:bg-surface-2 hover:text-fg"
          >
            <span aria-hidden>⋯</span> Show {row.count} unchanged line{row.count === 1 ? "" : "s"}
          </button>
        );
      case "notice":
        return <div className="flex h-full items-center px-4 text-xs italic text-fg-muted">{row.text}</div>;
      case "line": {
        const selectable = isLineSelectable(row.fileIndex, row.line);
        const selected = selection.get(selKey(row.fileIndex, row.hunkIndex))?.has(row.index) ?? false;
        return (
          <UnifiedLine
            line={row.line}
            selected={selected}
            onToggle={selectable ? () => toggleLine(row.fileIndex, row.hunkIndex, row.index) : undefined}
          />
        );
      }
      case "split": {
        const sel = selection.get(selKey(row.fileIndex, row.hunkIndex));
        const cell = (c: SplitCell | null, side: "left" | "right") => (
          <SplitSide
            cell={c}
            side={side}
            selected={c ? (sel?.has(c.index) ?? false) : false}
            onToggle={
              c && isLineSelectable(row.fileIndex, c.line)
                ? () => toggleLine(row.fileIndex, row.hunkIndex, c.index)
                : undefined
            }
          />
        );
        return (
          <div className="grid h-full grid-cols-2 divide-x divide-line/60">
            {cell(row.left, "left")}
            {cell(row.right, "right")}
          </div>
        );
      }
    }
  };

  function isLineSelectable(fileIndex: number, line: DiffLine): boolean {
    const file = result?.files[fileIndex];
    return Boolean(canStageLines && file && isLineStageable(file) && isSelectable(line));
  }

  return (
    <section className="flex h-full min-h-0 flex-col bg-surface-0" aria-label="Diff">
      <div className="flex h-10 shrink-0 items-center gap-3 border-b border-line px-3 text-xs">
        <span className="text-fg-muted">
          {result ? `${result.files.length} file${result.files.length === 1 ? "" : "s"}` : "Loading…"}
        </span>
        {result && (
          <span className="tabular-nums">
            <span className="text-emerald-400">+{totals.add}</span>{" "}
            <span className="text-rose-400">−{totals.del}</span>
          </span>
        )}
        <div className="ml-auto flex items-center gap-1">
          <Toggle active={fullContext} onClick={() => setFullContext((v) => !v)} title="Show whole files (folded)">
            Full file
          </Toggle>
          <Toggle
            active={ignoreWhitespace}
            onClick={() => setIgnoreWhitespace((v) => !v)}
            title="Ignore whitespace (disables line staging)"
          >
            Ignore WS
          </Toggle>
          <div
            className="ml-2 flex overflow-hidden rounded-md border border-line"
            role="radiogroup"
            aria-label="Diff layout"
            title={`Toggle layout (${formatShortcut(TOGGLE_MODE_SHORTCUT)})`}
          >
            {(["unified", "split"] as const).map((m) => (
              <button
                key={m}
                type="button"
                role="radio"
                aria-checked={mode === m}
                onClick={() => mode !== m && toggleMode()}
                className={clsx(
                  "px-2.5 py-1 capitalize",
                  mode === m ? "bg-surface-3 text-fg" : "text-fg-muted hover:text-fg",
                )}
              >
                {m}
              </button>
            ))}
          </div>
        </div>
      </div>

      {notice && (
        <ErrorText
          tone="warning"
          className="m-2"
          action={<ErrorAction onClick={() => setNotice(null)}>Dismiss</ErrorAction>}
        >
          {notice}
        </ErrorText>
      )}
      {error && <ErrorText className="m-3">Failed to load diff: {error}</ErrorText>}
      {result && result.files.length === 0 && (
        <div className="p-8 text-center text-sm text-fg-muted">No changes</div>
      )}

      <div ref={scrollRef} className="relative min-h-0 flex-1 overflow-auto font-mono text-[12.5px] leading-5">
        <div style={{ height: virtualizer.getTotalSize(), position: "relative", minWidth: "100%" }}>
          {virtualizer.getVirtualItems().map((item) => (
            <div
              key={item.key}
              className="absolute left-0 w-full"
              style={{ height: item.size, transform: `translateY(${item.start}px)` }}
            >
              {renderRow(rows[item.index])}
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------

function isLineStageable(file: FileDiff): boolean {
  return file.status === "modified" && !file.isBinary && !file.truncated;
}

function foldSet(expanded: ReadonlySet<string>, fileIndex: number): ReadonlySet<string> {
  const prefix = `${fileIndex}/`;
  const out = new Set<string>();
  for (const k of expanded) if (k.startsWith(prefix)) out.add(k.slice(prefix.length));
  return out;
}

function toggled<T>(set: ReadonlySet<T>, value: T): ReadonlySet<T> {
  const next = new Set(set);
  if (next.has(value)) next.delete(value);
  else next.add(value);
  return next;
}

const STATUS_BADGE: Record<FileStatus, [string, string]> = {
  added: ["A", "text-emerald-400"],
  untracked: ["U", "text-emerald-400"],
  deleted: ["D", "text-rose-400"],
  modified: ["M", "text-amber-300"],
  renamed: ["R", "text-sky-300"],
  copied: ["C", "text-sky-300"],
  typeChange: ["T", "text-violet-300"],
  conflicted: ["!", "text-rose-500"],
  unmodified: ["·", "text-fg-muted"],
};

function FileHeader(props: {
  file: FileDiff;
  collapsed: boolean;
  onToggle: () => void;
  action: string | null;
  busy: boolean;
  onAction: () => void;
}) {
  const { file } = props;
  const [badge, color] = STATUS_BADGE[file.status];
  const renamed = file.oldPath && file.newPath && file.oldPath !== file.newPath;
  return (
    <div className="flex h-full items-center gap-2 border-b border-line bg-surface-2 px-3 font-sans text-[13px]">
      <button type="button" onClick={props.onToggle} className="text-fg-muted hover:text-fg" aria-expanded={!props.collapsed}>
        <svg viewBox="0 0 16 16" className={clsx("h-3 w-3 transition-transform", !props.collapsed && "rotate-90")} aria-hidden>
          <path d="M6 4l4 4-4 4" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
        </svg>
      </button>
      <span className={clsx("w-4 text-center font-mono text-xs font-bold", color)} title={file.status}>
        {badge}
      </span>
      <span className="min-w-0 truncate text-fg" title={file.newPath ?? file.oldPath ?? ""}>
        {renamed ? (
          <>
            <span className="text-fg-muted">{file.oldPath}</span> → {file.newPath}
          </>
        ) : (
          filePath(file)
        )}
      </span>
      <span className="ml-2 shrink-0 text-xs tabular-nums">
        <span className="text-emerald-400">+{file.additions}</span>{" "}
        <span className="text-rose-400">−{file.deletions}</span>
      </span>
      {props.action && (
        <HunkButton className="ml-auto" disabled={props.busy} onClick={props.onAction}>
          {props.action}
        </HunkButton>
      )}
    </div>
  );
}

const LINE_STYLE = {
  addition: { row: "bg-emerald-500/10", sign: "+", text: "text-emerald-100", gutter: "bg-emerald-500/15" },
  deletion: { row: "bg-rose-500/10", sign: "−", text: "text-rose-100", gutter: "bg-rose-500/15" },
  context: { row: "", sign: " ", text: "text-fg/85", gutter: "" },
} as const;

function UnifiedLine({ line, selected, onToggle }: { line: DiffLine; selected: boolean; onToggle?: () => void }) {
  const s = LINE_STYLE[line.kind];
  return (
    <div className={clsx("flex h-full", s.row, selected && "outline outline-1 -outline-offset-1 outline-accent")}>
      <LineNo n={line.oldLineno} gutter={s.gutter} onClick={onToggle} selected={selected} />
      <LineNo n={line.newLineno} gutter={s.gutter} onClick={onToggle} selected={selected} />
      <span className={clsx("w-5 shrink-0 select-none text-center", s.text)}>{s.sign}</span>
      <code className={clsx("whitespace-pre pr-6 [tab-size:4]", s.text)}>
        {line.content}
        {line.noNewline && <NoNewline />}
      </code>
    </div>
  );
}

function SplitSide(props: {
  cell: SplitCell | null;
  side: "left" | "right";
  selected: boolean;
  onToggle?: () => void;
}) {
  if (!props.cell) return <div className="h-full bg-surface-1/40" />;
  const { line } = props.cell;
  const s = LINE_STYLE[line.kind];
  const n = props.side === "left" ? line.oldLineno : line.newLineno;
  return (
    <div
      className={clsx(
        "flex h-full min-w-0 overflow-hidden",
        s.row,
        props.selected && "outline outline-1 -outline-offset-1 outline-accent",
      )}
    >
      <LineNo n={n} gutter={s.gutter} onClick={props.onToggle} selected={props.selected} />
      <span className={clsx("w-5 shrink-0 select-none text-center", s.text)}>{s.sign}</span>
      <code className={clsx("min-w-0 truncate whitespace-pre [tab-size:4]", s.text)} title={line.content}>
        {line.content}
        {line.noNewline && <NoNewline />}
      </code>
    </div>
  );
}

function LineNo(props: { n: number | null; gutter: string; onClick?: () => void; selected: boolean }) {
  const interactive = Boolean(props.onClick);
  return (
    <span
      role={interactive ? "checkbox" : undefined}
      aria-checked={interactive ? props.selected : undefined}
      tabIndex={interactive ? 0 : undefined}
      onClick={props.onClick}
      onKeyDown={(e) => {
        if (interactive && (e.key === " " || e.key === "Enter")) {
          e.preventDefault();
          props.onClick?.();
        }
      }}
      className={clsx(
        "w-12 shrink-0 select-none pr-2 text-right tabular-nums text-fg-muted/70",
        props.gutter,
        interactive && "cursor-pointer hover:text-fg",
        props.selected && "bg-accent/40 text-fg",
      )}
    >
      {props.n ?? ""}
    </span>
  );
}

function NoNewline() {
  return (
    <span className="ml-1 select-none text-rose-400/80" title="No newline at end of file">
      ⊘
    </span>
  );
}

function Toggle(props: { active: boolean; onClick: () => void; title: string; children: ReactNode }) {
  return (
    <button
      type="button"
      aria-pressed={props.active}
      title={props.title}
      onClick={props.onClick}
      className={clsx(
        "rounded-md px-2 py-1",
        props.active ? "bg-accent/25 text-fg" : "text-fg-muted hover:bg-surface-2 hover:text-fg",
      )}
    >
      {props.children}
    </button>
  );
}

function HunkButton(props: {
  onClick: () => void;
  disabled?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      disabled={props.disabled}
      onClick={props.onClick}
      className={clsx(
        "rounded border border-line bg-surface-3 px-2 py-0.5 text-[11px] text-fg hover:border-accent/60 hover:bg-accent/20 disabled:opacity-50",
        props.className,
      )}
    >
      {props.children}
    </button>
  );
}
