/**
 * ImageAttachments — renders thumbnails for the image paths referenced in a
 * chat message (below the bubble text), with click-to-zoom and a graceful
 * fallback when the file no longer exists (agents move/delete screenshots a lot).
 *
 * The image bytes come from the backend `/api/file?path=` endpoint (browsers
 * can't load `file:///` from an http page). Used for both user and agent
 * messages — the endpoint serves real image bytes only, so an agent-named path
 * to a non-image is rejected server-side.
 */
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ImageOff, X } from "lucide-react";
import { fileUrl, baseName } from "@/lib/imagePaths";

function Thumb({ path, onZoom }: { path: string; onZoom: () => void }) {
  const { t } = useTranslation();
  const [failed, setFailed] = useState(false);
  if (failed) {
    return (
      <div
        className="flex items-center gap-1.5 rounded-md border border-border-subtle bg-surface-tertiary px-2 py-1.5 font-mono text-[10px] text-foreground-tertiary"
        title={path}
      >
        <ImageOff className="size-3.5 shrink-0" />
        <span className="max-w-[180px] truncate">{baseName(path)}</span>
        <span className="text-foreground-tertiary/70">· {t("messages.imageBroken")}</span>
      </div>
    );
  }
  return (
    <button
      type="button"
      onClick={onZoom}
      className="block overflow-hidden rounded-lg border border-border-subtle transition hover:opacity-90"
      title={path}
    >
      <img
        src={fileUrl(path)}
        alt={baseName(path)}
        loading="lazy"
        onError={() => setFailed(true)}
        className="max-h-56 max-w-[280px] object-contain"
      />
    </button>
  );
}

function Lightbox({ path, onClose }: { path: string; onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div
      role="dialog"
      aria-modal="true"
      onClick={onClose}
      className="fixed inset-0 z-[100] flex items-center justify-center bg-black/80 p-8 backdrop-blur-sm"
    >
      <button
        type="button"
        onClick={onClose}
        aria-label="Close"
        className="absolute right-4 top-4 inline-flex size-9 items-center justify-center rounded-full bg-white/10 text-white hover:bg-white/20"
      >
        <X className="size-5" />
      </button>
      {/* stopPropagation so clicking the image itself doesn't close */}
      <img
        src={fileUrl(path)}
        alt={baseName(path)}
        onClick={(e) => e.stopPropagation()}
        className="max-h-full max-w-full rounded-lg object-contain shadow-2xl"
      />
    </div>
  );
}

export function ImageAttachments({ paths }: { paths: string[] }) {
  const [zoom, setZoom] = useState<string | null>(null);
  if (paths.length === 0) return null;
  return (
    <>
      <div className="mt-1.5 flex flex-wrap gap-2">
        {paths.map((p) => (
          <Thumb key={p} path={p} onZoom={() => setZoom(p)} />
        ))}
      </div>
      {zoom && <Lightbox path={zoom} onClose={() => setZoom(null)} />}
    </>
  );
}
