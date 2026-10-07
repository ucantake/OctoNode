import { useState } from "react";
import clsx from "clsx";
import { hashColor, initials } from "../lib/theme";

interface AvatarProps {
  name: string;
  /** Seed for the fallback color (email is stabler than the display name). */
  seed?: string;
  url?: string | null;
  size?: number;
  className?: string;
}

/** Remote image when available (and allowed by CSP), initials otherwise. */
export function Avatar({ name, seed, url, size = 20, className }: AvatarProps) {
  const [failed, setFailed] = useState(false);
  const style = { width: size, height: size, fontSize: Math.max(8, Math.round(size * 0.42)) };

  if (url && !failed) {
    return (
      <img
        src={url}
        alt={name}
        width={size}
        height={size}
        onError={() => setFailed(true)}
        className={clsx("shrink-0 rounded-full object-cover", className)}
        style={style}
        draggable={false}
      />
    );
  }
  return (
    <span
      aria-label={name}
      className={clsx(
        "inline-flex shrink-0 select-none items-center justify-center rounded-full font-semibold text-black/80",
        className,
      )}
      style={{ ...style, backgroundColor: hashColor(seed ?? name) }}
    >
      {initials(name)}
    </span>
  );
}
