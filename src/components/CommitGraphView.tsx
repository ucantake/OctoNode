import { useCallback, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import clsx from "clsx";
import { ErrorText } from "./ErrorText";
import { Avatar } from "./Avatar";
import type { CommitGraphSource } from "../hooks/useCommitGraph";
import { laneColor, relativeTime } from "../lib/theme";
import type { CommitNode, RefBadge } from "../types/models";

export const ROW_HEIGHT = 28;
const LANE_WIDTH = 14;
const GRAPH_PADDING = 10;
const NODE_RADIUS = 4.5;
/** Initial gutter width in lanes; the column is resizable from its header. */
const DEFAULT_VISIBLE_LANES = 8;
const MAX_VISIBLE_LANES = 48;

export interface CommitGraphViewProps {
  source: CommitGraphSource;
  selectedId: string | null;
  onSelect: (commit: CommitNode) => void;
}

/**
 * Virtualized commit list with a Canvas-rendered DAG gutter.
 *
 * Only the visible window (+ overscan) exists in the DOM, and the canvas is
 * sized to that window and positioned inside the scroll content, so cost is
 * independent of history length (tested with 50k+ commits). Each row carries
 * its own incoming edges, so any window can be drawn without its neighbours.
 */
export function CommitGraphView({ source, selectedId, onSelect }: CommitGraphViewProps) {
  const { meta, error, version, getRow, ensureRange } = source;
  const total = meta?.total ?? 0;
  // User-chosen width wins; otherwise fit the history up to a sane default.
  // Lanes beyond the gutter are clipped by the canvas.
  const [userLanes, setUserLanes] = useState<number | null>(null);
  const maxLanes = Math.max(meta?.maxLanes ?? 1, 1);
  const lanes = userLanes ?? Math.min(maxLanes, DEFAULT_VISIBLE_LANES);
  const graphWidth = GRAPH_PADDING * 2 + lanes * LANE_WIDTH;

  const startResize = (e: PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const startX = e.clientX;
    const startLanes = lanes;
    const onMove = (ev: globalThis.PointerEvent) => {
      const delta = Math.round((ev.clientX - startX) / LANE_WIDTH);
      setUserLanes(Math.min(Math.max(startLanes + delta, 1), Math.min(maxLanes, MAX_VISIBLE_LANES)));
    };
    const onUp = () => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
  };

  const scrollRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  const virtualizer = useVirtualizer({
    count: total,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 16,
  });
  const items = virtualizer.getVirtualItems();
  const first = items.length > 0 ? items[0].index : 0;
  const last = items.length > 0 ? items[items.length - 1].index : -1;

  useEffect(() => {
    // Prefetch one row past the window so its incoming edges are known.
    if (last >= 0) ensureRange(first, Math.min(last + 1, total - 1));
  }, [first, last, total, ensureRange]);

  // ---- Canvas drawing -----------------------------------------------------
  // The canvas spans rows [first-1, last+1] so edges that leave the window
  // at the top/bottom are drawn too.
  const canvasTop = Math.max(0, first - 1) * ROW_HEIGHT;
  const canvasRows = last >= first ? last - Math.max(0, first - 1) + 2 : 0;

  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || canvasRows === 0) return;
    const dpr = window.devicePixelRatio || 1;
    const cssHeight = canvasRows * ROW_HEIGHT;
    // Re-allocating the backing store is costly; only do it on size change.
    if (canvas.width !== Math.round(graphWidth * dpr) || canvas.height !== Math.round(cssHeight * dpr)) {
      canvas.width = Math.round(graphWidth * dpr);
      canvas.height = Math.round(cssHeight * dpr);
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, graphWidth, cssHeight);

    const startRow = Math.max(0, first - 1);
    const x = (lane: number) => GRAPH_PADDING + lane * LANE_WIDTH + LANE_WIDTH / 2;
    const y = (row: number) => (row - startRow) * ROW_HEIGHT + ROW_HEIGHT / 2;

    ctx.lineWidth = 2;
    ctx.lineCap = "round";
    const endRow = Math.min(last + 1, total - 1);
    for (let i = startRow + 1; i <= endRow; i++) {
      const row = getRow(i);
      if (!row) continue;
      for (const [from, to, color] of row.edges) {
        if (from >= lanes && to >= lanes) continue; // entirely outside the gutter
        const x1 = x(from);
        const x2 = x(to);
        const y1 = y(i - 1);
        const y2 = y(i);
        ctx.strokeStyle = laneColor(color);
        ctx.beginPath();
        ctx.moveTo(x1, y1);
        if (x1 === x2) {
          ctx.lineTo(x2, y2);
        } else {
          // Smooth S-curve; control points at mid-height keep curves tidy
          // when several lanes bend in the same row.
          const midY = (y1 + y2) / 2;
          ctx.bezierCurveTo(x1, midY, x2, midY, x2, y2);
        }
        ctx.stroke();
      }
    }

    for (let i = Math.max(first, 0); i <= last; i++) {
      const row = getRow(i);
      if (!row) continue;
      if (row.lane >= lanes) continue; // clipped; the row text still shows
      const cx = x(row.lane);
      const cy = y(i);
      const color = laneColor(row.color);
      const isHead = row.refs.some((r) => r.isHead);
      const isMerge = row.parents.length > 1;
      const radius = isHead ? NODE_RADIUS + 1.5 : NODE_RADIUS;

      ctx.beginPath();
      ctx.arc(cx, cy, radius, 0, Math.PI * 2);
      if (isMerge) {
        ctx.fillStyle = "#0b0d12";
        ctx.fill();
        ctx.lineWidth = 2;
        ctx.strokeStyle = color;
        ctx.stroke();
      } else {
        ctx.fillStyle = color;
        ctx.fill();
      }
      if (isHead || row.id === selectedId) {
        ctx.beginPath();
        ctx.arc(cx, cy, radius + 2.5, 0, Math.PI * 2);
        ctx.lineWidth = 1.5;
        ctx.strokeStyle = row.id === selectedId ? "#e6e8ee" : color;
        ctx.stroke();
      }
    }
  }, [first, last, total, canvasRows, graphWidth, lanes, getRow, version, selectedId]);

  // ---- Keyboard navigation ------------------------------------------------
  const selectedIndex = useRef<number>(-1);
  const select = useCallback(
    (index: number) => {
      const row = getRow(index);
      if (!row) return;
      selectedIndex.current = index;
      onSelect(row);
      virtualizer.scrollToIndex(index, { align: "auto" });
    },
    [getRow, onSelect, virtualizer],
  );

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const page = Math.max(1, Math.floor((scrollRef.current?.clientHeight ?? 0) / ROW_HEIGHT) - 1);
    const cur = selectedIndex.current;
    const moves: Record<string, number> = {
      ArrowDown: cur + 1,
      ArrowUp: cur - 1,
      PageDown: cur + page,
      PageUp: cur - page,
      Home: 0,
      End: total - 1,
    };
    const next = moves[e.key];
    if (next === undefined || total === 0) return;
    e.preventDefault();
    const clamped = Math.min(Math.max(next, 0), total - 1);
    ensureRange(clamped, clamped);
    select(clamped);
  };

  if (error) {
    return (
      <div className="p-4">
        <ErrorText>Failed to load history: {error}</ErrorText>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div
        className="flex h-7 shrink-0 items-center border-b border-line text-[11px] uppercase tracking-wide text-fg-muted"
        role="row"
      >
        <div style={{ width: graphWidth }} className="relative h-full shrink-0 pl-3 leading-7">
          Graph
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize graph column"
            title="Drag to resize · double-click to fit"
            onPointerDown={startResize}
            onDoubleClick={() => setUserLanes(Math.min(maxLanes, MAX_VISIBLE_LANES))}
            className="absolute right-0 top-0 h-full w-1.5 cursor-col-resize hover:bg-accent/50"
          />
        </div>
        <div className="flex-1 px-2">Description</div>
        <div className="w-44 px-2">Author</div>
        <div className="w-20 px-2">Commit</div>
        <div className="w-28 px-2 text-right">Date</div>
      </div>

      <div
        ref={scrollRef}
        tabIndex={0}
        onKeyDown={onKeyDown}
        role="grid"
        aria-rowcount={total}
        aria-label="Commit history"
        className="relative min-h-0 flex-1 overflow-auto outline-none"
      >
        <div style={{ height: virtualizer.getTotalSize(), position: "relative", width: "100%" }}>
          <canvas
            ref={canvasRef}
            aria-hidden
            className="pointer-events-none absolute left-0 z-10"
            style={{ top: canvasTop, width: graphWidth, height: canvasRows * ROW_HEIGHT }}
          />
          {items.map((item) => {
            const row = getRow(item.index);
            return (
              <div
                key={item.key}
                role="row"
                aria-rowindex={item.index + 1}
                aria-selected={row?.id === selectedId}
                onClick={() => {
                  if (!row) return;
                  selectedIndex.current = item.index;
                  onSelect(row);
                }}
                className={clsx(
                  "absolute left-0 flex w-full cursor-default items-center text-[13px]",
                  row?.id === selectedId ? "bg-accent/20" : "hover:bg-surface-2",
                )}
                style={{ height: ROW_HEIGHT, transform: `translateY(${item.start}px)` }}
              >
                <div style={{ width: graphWidth }} className="shrink-0" />
                {row ? <CommitCells commit={row} /> : <SkeletonCells />}
              </div>
            );
          })}
        </div>
      </div>

      {meta?.truncated && (
        <div className="border-t border-line px-3 py-1 text-xs text-amber-300">
          History truncated at {total.toLocaleString()} commits.
        </div>
      )}
    </div>
  );
}

function CommitCells({ commit }: { commit: CommitNode }) {
  return (
    <>
      <div className="flex min-w-0 flex-1 items-center gap-1.5 px-2">
        {commit.refs.map((r) => (
          <RefPill key={`${r.kind}:${r.name}`} badge={r} color={laneColor(commit.color)} />
        ))}
        <span className="truncate text-fg" title={commit.summary}>
          {commit.summary}
        </span>
      </div>
      <div className="flex w-44 min-w-0 items-center gap-2 px-2 text-fg-muted">
        <Avatar name={commit.authorName} seed={commit.authorEmail} size={18} />
        <span className="truncate" title={`${commit.authorName} <${commit.authorEmail}>`}>
          {commit.authorName}
        </span>
      </div>
      <div className="w-20 px-2 font-mono text-xs text-fg-muted">{commit.shortId}</div>
      <div
        className="w-28 px-2 text-right text-xs tabular-nums text-fg-muted"
        title={new Date(commit.time * 1000).toLocaleString()}
      >
        {relativeTime(commit.time)}
      </div>
    </>
  );
}

function SkeletonCells() {
  return (
    <div className="flex flex-1 items-center gap-3 px-2">
      <div className="h-2.5 w-1/3 animate-pulse rounded bg-surface-3" />
      <div className="h-2.5 w-24 animate-pulse rounded bg-surface-2" />
    </div>
  );
}

function RefPill({ badge, color }: { badge: RefBadge; color: string }) {
  const base = "inline-flex max-w-[14rem] shrink-0 items-center gap-1 rounded px-1.5 text-[11px] leading-[18px]";
  switch (badge.kind) {
    case "localBranch":
    case "head":
      return (
        <span
          className={clsx(base, "font-medium text-black/85", badge.isHead && "ring-1 ring-white/70")}
          style={{ backgroundColor: color }}
          title={badge.isHead ? `${badge.name} (HEAD)` : badge.name}
        >
          {badge.isHead && <span aria-hidden>●</span>}
          <span className="truncate">{badge.name}</span>
        </span>
      );
    case "remoteBranch":
      return (
        <span className={clsx(base, "border text-fg-muted")} style={{ borderColor: color }} title={badge.name}>
          <span className="truncate">{badge.name}</span>
        </span>
      );
    case "tag":
      return (
        <span className={clsx(base, "bg-surface-3 text-amber-200")} title={`tag ${badge.name}`}>
          <svg viewBox="0 0 16 16" className="h-3 w-3" aria-hidden>
            <path
              d="M2 2h6l6 6-6 6-6-6V2z M5 5.5a.5.5 0 1 0 0 .01"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinejoin="round"
            />
          </svg>
          <span className="truncate">{badge.name}</span>
        </span>
      );
  }
}
