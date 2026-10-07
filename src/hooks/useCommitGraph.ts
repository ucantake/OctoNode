import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorMessage } from "../lib/ipc";
import type { CommitNode, Uuid } from "../types/models";

export const GRAPH_PAGE_SIZE = 400;

interface GraphMeta {
  total: number;
  maxLanes: number;
  truncated: boolean;
}

export interface CommitGraphSource {
  meta: GraphMeta | null;
  error: string | null;
  /** Bumps whenever a page arrives, so consumers re-render. */
  version: number;
  getRow: (index: number) => CommitNode | undefined;
  ensureRange: (start: number, end: number) => void;
  refresh: () => void;
}

/**
 * Paged access to the backend's laid-out commit graph. The full layout lives
 * in Rust; the webview only holds pages that were scrolled into view.
 */
export function useCommitGraph(repoId: Uuid | null): CommitGraphSource {
  const [meta, setMeta] = useState<GraphMeta | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [version, setVersion] = useState(0);

  const pages = useRef(new Map<number, CommitNode[]>());
  const inflight = useRef(new Set<number>());
  // Responses from a previous repo/refresh are dropped by generation.
  const generation = useRef(0);

  const loadPage = useCallback(
    (page: number) => {
      if (!repoId || pages.current.has(page) || inflight.current.has(page)) return;
      const gen = generation.current;
      inflight.current.add(page);
      api
        .getCommitGraph(repoId, page * GRAPH_PAGE_SIZE, GRAPH_PAGE_SIZE)
        .then((res) => {
          if (gen !== generation.current) return;
          pages.current.set(page, res.rows);
          setMeta({ total: res.total, maxLanes: res.maxLanes, truncated: res.truncated });
          setError(null);
          setVersion((v) => v + 1);
        })
        .catch((e: unknown) => {
          if (gen === generation.current) setError(errorMessage(e));
        })
        .finally(() => {
          if (gen === generation.current) inflight.current.delete(page);
        });
    },
    [repoId],
  );

  const reset = useCallback(() => {
    generation.current += 1;
    pages.current = new Map();
    inflight.current = new Set();
    setMeta(null);
    setError(null);
    setVersion((v) => v + 1);
  }, []);

  useEffect(() => {
    reset();
    loadPage(0);
  }, [repoId, reset, loadPage]);

  const getRow = useCallback((index: number) => {
    const page = pages.current.get(Math.floor(index / GRAPH_PAGE_SIZE));
    return page?.[index % GRAPH_PAGE_SIZE];
  }, []);

  const ensureRange = useCallback(
    (start: number, end: number) => {
      const first = Math.max(0, Math.floor(start / GRAPH_PAGE_SIZE));
      const last = Math.floor(Math.max(start, end) / GRAPH_PAGE_SIZE);
      for (let p = first; p <= last; p++) loadPage(p);
    },
    [loadPage],
  );

  const refresh = useCallback(() => {
    reset();
    loadPage(0);
  }, [reset, loadPage]);

  return { meta, error, version, getRow, ensureRange, refresh };
}
