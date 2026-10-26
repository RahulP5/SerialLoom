import { useEffect, type ReactNode } from "react";
import { X } from "lucide-react";

export function Modal({
  title,
  subtitle,
  icon,
  onClose,
  children,
  footer,
  width = 560,
}: {
  title: string;
  subtitle?: string;
  icon?: ReactNode;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  width?: number;
}) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-ink-950/70 p-3 backdrop-blur-sm sm:p-6"
      onMouseDown={onClose}
    >
      <div
        className="panel flex max-h-[calc(100dvh-1.5rem)] w-full flex-col bg-ink-850 shadow-2xl shadow-black/60 sm:max-h-[calc(100dvh-3rem)]"
        style={{ maxWidth: width }}
        onMouseDown={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        <header className="flex shrink-0 items-start gap-3 border-b border-ink-700/70 px-5 py-4">
          {icon && <div className="mt-0.5">{icon}</div>}
          <div className="min-w-0 flex-1">
            <h2 className="text-base font-semibold text-white">{title}</h2>
            {subtitle && <p className="mt-0.5 text-sm text-ink-300">{subtitle}</p>}
          </div>
          <button className="btn-ghost -mr-2 px-2" onClick={onClose} aria-label="Close">
            <X size={16} />
          </button>
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">{children}</div>
        {footer && (
          <footer className="flex shrink-0 flex-wrap justify-end gap-2 border-t border-ink-700/70 px-5 py-3">{footer}</footer>
        )}
      </div>
    </div>
  );
}
