import { useState, type ReactNode } from "react";
import clsx from "clsx";

export interface ErrorTextProps {
  children: ReactNode;
  /** Plain text to copy; defaults to the rendered text content. */
  copyText?: string;
  /** e.g. an "Edit account" button that fixes the problem. */
  action?: ReactNode;
  tone?: "error" | "warning";
  className?: string;
}

/**
 * Error / warning message that can always be selected and copied.
 *
 * The app shell disables text selection (desktop-app feel), so error text
 * must opt back in explicitly; a Copy button covers trackpads and the case
 * where selecting inside a dialog is awkward.
 */
export function ErrorText({ children, copyText, action, tone = "error", className }: ErrorTextProps) {
  const [copied, setCopied] = useState(false);

  const copy = async (el: HTMLElement | null) => {
    const text = copyText ?? el?.closest("[data-error-text]")?.querySelector("[data-error-body]")?.textContent ?? "";
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard API unavailable (very old WebView): the text stays selectable.
    }
  };

  return (
    <div
      role={tone === "error" ? "alert" : "status"}
      data-error-text
      className={clsx(
        "flex items-start gap-2 rounded-md border px-2.5 py-1.5 text-xs",
        tone === "error"
          ? "border-rose-500/30 bg-rose-500/10 text-rose-300"
          : "border-amber-500/30 bg-amber-500/10 text-amber-200",
        className,
      )}
    >
      <span data-error-body className="selectable min-w-0 flex-1 cursor-text break-words">
        {children}
      </span>
      <span className="flex shrink-0 items-center gap-1">
        {action}
        <button
          type="button"
          onClick={(e) => void copy(e.currentTarget)}
          className="rounded px-1.5 py-0.5 text-[11px] opacity-70 hover:bg-white/10 hover:opacity-100"
          title="Copy message"
        >
          {copied ? "Copied" : "Copy"}
        </button>
      </span>
    </div>
  );
}

/** Small inline button used as an ErrorText action. */
export function ErrorAction(props: { onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      onClick={props.onClick}
      className="rounded border border-white/25 px-1.5 py-0.5 text-[11px] font-medium hover:bg-white/10"
    >
      {props.children}
    </button>
  );
}
