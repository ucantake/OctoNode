// Lane palette shared by the Canvas renderer and DOM badges. The backend emits
// color indices modulo `GRAPH_PALETTE.length` (12, see graph.rs PALETTE_SIZE).
export const GRAPH_PALETTE = [
  "#7c5cff", // violet
  "#22d3ee", // cyan
  "#f472b6", // pink
  "#a3e635", // lime
  "#fb923c", // orange
  "#60a5fa", // blue
  "#facc15", // yellow
  "#34d399", // emerald
  "#f87171", // red
  "#c084fc", // purple
  "#2dd4bf", // teal
  "#fda4af", // rose
] as const;

export function laneColor(index: number): string {
  return GRAPH_PALETTE[((index % GRAPH_PALETTE.length) + GRAPH_PALETTE.length) % GRAPH_PALETTE.length];
}

/** Deterministic color for names/emails (avatars without network access). */
export function hashColor(seed: string): string {
  let h = 0;
  for (let i = 0; i < seed.length; i++) h = (Math.imul(31, h) + seed.charCodeAt(i)) | 0;
  return laneColor(Math.abs(h));
}

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  const first = parts[0][0] ?? "";
  const last = parts.length > 1 ? (parts[parts.length - 1][0] ?? "") : "";
  return (first + last).toUpperCase();
}

const rtf = typeof Intl !== "undefined" ? new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }) : null;

export function relativeTime(unixSeconds: number, now = Date.now()): string {
  const diff = unixSeconds - Math.floor(now / 1000);
  const abs = Math.abs(diff);
  const units: Array<[Intl.RelativeTimeFormatUnit, number]> = [
    ["year", 31_536_000],
    ["month", 2_592_000],
    ["week", 604_800],
    ["day", 86_400],
    ["hour", 3_600],
    ["minute", 60],
  ];
  for (const [unit, secs] of units) {
    if (abs >= secs) {
      const v = Math.round(diff / secs);
      return rtf ? rtf.format(v, unit) : `${Math.abs(v)} ${unit}s ago`;
    }
  }
  return rtf ? rtf.format(0, "second") : "now";
}
