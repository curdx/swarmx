/**
 * File browser (`/files`).
 *
 * A minimal local file browser over GET /api/files/{list,read}: navigate
 * directories (dirs first), preview text files. Scoped to the selected
 * workspace's roots by default (the backend 403s on escape); the "browse whole
 * filesystem" toggle lifts the jail (sends `all=1`) for peeking at sibling
 * repos / config / logs. Binary / oversized files are flagged, not dumped.
 */
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Folder, FileText, ArrowUp, Loader2 } from "lucide-react";
import { api, ApiError } from "@/api/http";
import type { FileListResp, FileReadResp } from "@/api/types";
import { cn } from "@/lib/cn";
import { useToolWorkspaces } from "@/lib/useToolWorkspaces";
import { WorkspacePicker } from "@/components/WorkspacePicker";

function fmtSize(n: number): string {
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)}M`;
  if (n >= 1 << 10) return `${(n / (1 << 10)).toFixed(1)}K`;
  return `${n}B`;
}

export default function FilesRoute() {
  const { t } = useTranslation();
  const { workspaces, wsId, setWsId, ready } = useToolWorkspaces();
  const [browseAll, setBrowseAll] = useState(false);
  const [list, setList] = useState<FileListResp | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [preview, setPreview] = useState<FileReadResp | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);

  const open = useCallback(
    async (dir?: string, all = false) => {
      setLoading(true);
      setErr(null);
      // Reset the preview pane — otherwise the previously-selected file's content
      // lingers on the right while you browse into an unrelated directory.
      setPreview(null);
      setPreviewLoading(false);
      try {
        setList(await api.filesList(dir, wsId || undefined, all));
      } catch (e) {
        if (e instanceof ApiError && e.status === 403) setErr(t("files.jailBlocked"));
        else setErr((e as Error).message);
      } finally {
        setLoading(false);
      }
    },
    [wsId, t],
  );

  // Open the workspace root whenever the selected workspace changes (and on the
  // first load). The backend defaults an empty `dir` to the workspace cwd.
  useEffect(() => {
    if (!ready) return;
    setBrowseAll(false);
    open(undefined, false);
  }, [wsId, ready, open]);

  const openFile = useCallback(
    async (path: string, all = false) => {
      setPreviewLoading(true);
      setPreview(null);
      try {
        setPreview(await api.filesRead(path, wsId || undefined, all));
      } catch (e) {
        const msg = e instanceof ApiError && e.status === 403 ? t("files.jailBlocked") : (e as Error).message;
        setPreview({ path, binary: false, size: 0, content: `(${msg})`, truncated: false });
      } finally {
        setPreviewLoading(false);
      }
    },
    [wsId, t],
  );

  const toggleBrowseAll = () => {
    const next = !browseAll;
    setBrowseAll(next);
    open(list?.dir, next);
  };

  const join = (dir: string, name: string) => (dir.endsWith("/") ? dir + name : `${dir}/${name}`);

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <header className="flex items-center gap-2 border-b border-border-subtle px-4 py-3">
        <FileText className="size-4 text-foreground-tertiary" />
        <h1 className="font-display text-sm text-foreground-primary">{t("files.title")}</h1>
        <span className="truncate font-mono text-[11px] text-foreground-tertiary" title={list?.dir}>
          {list?.dir}
        </span>
        <div className="ml-auto flex items-center gap-3">
          <label className="flex items-center gap-1.5 font-caption text-[11px] text-foreground-tertiary">
            <input type="checkbox" checked={browseAll} onChange={toggleBrowseAll} className="size-3.5" />
            {t("files.browseAll")}
          </label>
          {workspaces.length > 0 && (
            <WorkspacePicker workspaces={workspaces} value={wsId} onChange={setWsId} />
          )}
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        {/* left: directory listing */}
        <div className="flex w-1/2 min-w-0 flex-col overflow-y-auto border-r border-border-subtle">
          {list?.parent && (
            <button
              type="button"
              onClick={() => open(list.parent!, browseAll)}
              className="flex items-center gap-2 px-4 py-1.5 text-left font-mono text-[12px] text-foreground-secondary hover:bg-surface-tertiary"
            >
              <ArrowUp className="size-3.5 shrink-0 text-foreground-tertiary" />
              ..
            </button>
          )}
          {loading && (
            <div className="flex items-center gap-2 px-4 py-3 font-caption text-xs text-foreground-tertiary">
              <Loader2 className="size-3.5 animate-spin" /> {t("common.loading")}
            </div>
          )}
          {err && <div className="px-4 py-3 font-caption text-xs text-status-danger">{err}</div>}
          {list?.entries.map((e) => (
            <button
              key={e.name}
              type="button"
              onClick={() =>
                e.is_dir ? open(join(list.dir, e.name), browseAll) : openFile(join(list.dir, e.name), browseAll)
              }
              className="flex items-center gap-2 px-4 py-1.5 text-left font-mono text-[12px] hover:bg-surface-tertiary"
            >
              {e.is_dir ? (
                <Folder className="size-3.5 shrink-0 text-accent-primary" />
              ) : (
                <FileText className="size-3.5 shrink-0 text-foreground-tertiary" />
              )}
              <span className={cn("min-w-0 flex-1 truncate", e.is_dir ? "text-foreground-primary" : "text-foreground-secondary")}>
                {e.name}
              </span>
              {!e.is_dir && (
                <span className="shrink-0 text-[10px] text-foreground-tertiary">{fmtSize(e.size)}</span>
              )}
            </button>
          ))}
        </div>

        {/* right: file preview */}
        <div className="flex w-1/2 min-w-0 flex-col overflow-hidden">
          {previewLoading && (
            <div className="flex items-center gap-2 px-4 py-3 font-caption text-xs text-foreground-tertiary">
              <Loader2 className="size-3.5 animate-spin" /> {t("common.loading")}
            </div>
          )}
          {!previewLoading && !preview && (
            <div className="flex flex-1 items-center justify-center px-6 text-center font-caption text-sm text-foreground-tertiary">
              {t("files.previewHint")}
            </div>
          )}
          {preview && (
            <div className="flex min-h-0 flex-1 flex-col">
              <div className="flex items-center gap-2 border-b border-border-subtle px-4 py-2 font-mono text-[11px] text-foreground-tertiary">
                <span className="min-w-0 flex-1 truncate" title={preview.path}>
                  {preview.path.split("/").pop()}
                </span>
                <span>{fmtSize(preview.size)}</span>
                {preview.truncated && <span className="text-status-warning">{t("files.truncated")}</span>}
              </div>
              {preview.binary ? (
                <div className="flex flex-1 items-center justify-center font-caption text-sm text-foreground-tertiary">
                  {t("files.binary")}
                </div>
              ) : (
                <pre className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-words px-4 py-3 font-mono text-[11px] text-foreground-secondary">
                  {preview.content}
                </pre>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
