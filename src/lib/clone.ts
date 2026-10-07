// Pure helpers for the clone dialog (unit-tested).

import type { CloneProgress } from "../types/models";

/** Mirrors `folder_name_from_url` in src-tauri/src/git/clone.rs. */
export function folderNameFromUrl(url: string): string | null {
  const trimmed = url.trim();
  let path: string | undefined;
  const scheme = trimmed.indexOf("://");
  if (scheme >= 0) {
    const rest = trimmed.slice(scheme + 3);
    const slash = rest.indexOf("/");
    path = slash >= 0 ? rest.slice(slash + 1) : undefined;
  } else {
    const colon = trimmed.indexOf(":");
    path = colon >= 0 ? trimmed.slice(colon + 1) : undefined;
  }
  if (!path) return null;
  const last = path.replace(/\/+$/, "").split("/").pop() ?? "";
  const name = last.endsWith(".git") ? last.slice(0, -4) : last;
  return name && !/[\\/<>:"|?*]/.test(name) && name !== "." && name !== ".." ? name : null;
}

/** RFC 4122 v4; `crypto.randomUUID` is missing on older WebKit (Safari < 15.4). */
export function newCloneId(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6] & 0x0f) | 0x40;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

/** 0..1 for a determinate bar, `null` when the phase has no measurable total. */
export function cloneProgressFraction(p: CloneProgress): number | null {
  // Receiving objects is ~70% of the work, resolving deltas ~20%, checkout ~10%.
  const ratio = (done: number, total: number) => (total > 0 ? Math.min(done / total, 1) : 0);
  switch (p.phase) {
    case "receiving":
      return p.totalObjects > 0 ? 0.7 * ratio(p.receivedObjects, p.totalObjects) : null;
    case "resolving":
      return 0.7 + 0.2 * ratio(p.indexedDeltas, p.totalDeltas);
    case "checkout":
      return 0.9 + 0.1 * ratio(p.checkoutDone, p.checkoutTotal);
    case "done":
      return 1;
    case "fallback":
      return null;
  }
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KiB", "MiB", "GiB"];
  let v = n / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v.toFixed(v >= 100 ? 0 : 1)} ${units[u]}`;
}

export function describeProgress(p: CloneProgress): string {
  switch (p.phase) {
    case "receiving":
      return `Receiving objects ${p.receivedObjects.toLocaleString()} / ${p.totalObjects.toLocaleString()} · ${formatBytes(p.receivedBytes)}`;
    case "resolving":
      return `Resolving deltas ${p.indexedDeltas.toLocaleString()} / ${p.totalDeltas.toLocaleString()}`;
    case "checkout":
      return `Checking out files ${p.checkoutDone.toLocaleString()} / ${p.checkoutTotal.toLocaleString()}`;
    case "fallback":
      return "Retrying with the git command-line client (SSH configuration libgit2 can't handle)…";
    case "done":
      return "Done";
  }
}
