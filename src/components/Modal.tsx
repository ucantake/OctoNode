import { useEffect, useRef, type ReactNode } from "react";

export interface ModalProps {
  title: string;
  onClose?: () => void;
  children: ReactNode;
  footer?: ReactNode;
  size?: "md" | "lg";
}

/**
 * Minimal accessible dialog. Native `window.prompt/confirm` are not used:
 * they are unsupported or inconsistent across the three system WebViews.
 */
export function Modal({ title, onClose, children, footer, size = "md" }: ModalProps) {
  const ref = useRef<HTMLDivElement>(null);
  // Latest onClose without re-running the focus effect on every render.
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>("input, select, textarea, button")?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && onCloseRef.current) {
        e.preventDefault();
        onCloseRef.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      prev?.focus();
    };
  }, []);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-sm">
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={`w-full ${size === "lg" ? "max-w-xl" : "max-w-md"} rounded-xl border border-line bg-surface-1 shadow-2xl`}
      >
        <header className="border-b border-line px-5 py-3 text-sm font-semibold text-fg">{title}</header>
        <div className="space-y-3 px-5 py-4 text-sm">{children}</div>
        {footer && <footer className="flex justify-end gap-2 border-t border-line px-5 py-3">{footer}</footer>}
      </div>
    </div>
  );
}

export function Field(props: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="block">
      <span className="mb-1 block text-xs font-medium text-fg-muted">{props.label}</span>
      {props.children}
      {props.hint && <span className="mt-1 block text-[11px] text-fg-muted/80">{props.hint}</span>}
    </label>
  );
}

export const inputClass =
  "w-full rounded-md border border-line bg-surface-0 px-2.5 py-1.5 text-sm text-fg outline-none placeholder:text-fg-muted/60 focus:border-accent/70";

export function Button(props: {
  kind?: "primary" | "ghost";
  type?: "button" | "submit";
  disabled?: boolean;
  onClick?: () => void;
  children: ReactNode;
  form?: string;
}) {
  const primary = props.kind !== "ghost";
  return (
    <button
      type={props.type ?? "button"}
      form={props.form}
      disabled={props.disabled}
      onClick={props.onClick}
      className={
        primary
          ? "rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-white hover:bg-accent/90 disabled:opacity-50"
          : "rounded-md px-3 py-1.5 text-sm text-fg-muted hover:bg-surface-2 hover:text-fg"
      }
    >
      {props.children}
    </button>
  );
}
