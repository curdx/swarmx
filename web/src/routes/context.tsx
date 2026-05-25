/**
 * Context Board — Pencil frame a3RrDG.
 *
 * Three-pane read surface over /api/blackboard:
 *   FileTree (280) | MarkdownView (fill) | MetaSidebar (280)
 *
 * Intentionally read-only. BlackboardPanel under /debug already owns
 * write + HITL approve/reject; product surface is for browsing. The
 * data hooks here re-call the same /api/blackboard endpoints rather
 * than sharing state with BlackboardPanel — keeps /debug stable while
 * we iterate on the product UI.
 */

import { useEffect, useMemo, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  ChevronDown,
  ChevronRight,
  FileText,
  History,
  RefreshCw,
  Search,
} from "lucide-react";
import { api } from "../api/http";
import type {
  BlackboardEntry,
  BlackboardHistoryEntry,
  BlackboardSnapshot,
} from "../api/types";
import { useSwarmFeed } from "../hooks/useSwarmFeed";
import { cn } from "@/lib/cn";

interface TreeNode {
  name: string;
  fullPath: string | null; // null for folder nodes
  children: TreeNode[];
}

function buildTree(entries: BlackboardEntry[]): TreeNode {
  const root: TreeNode = { name: "", fullPath: null, children: [] };
  for (const e of entries) {
    const parts = e.path.split("/");
    let cur = root;
    for (let i = 0; i < parts.length; i++) {
      const part = parts[i];
      const isLeaf = i === parts.length - 1;
      let child = cur.children.find((c) => c.name === part);
      if (!child) {
        child = {
          name: part,
          fullPath: isLeaf ? e.path : null,
          children: [],
        };
        cur.children.push(child);
      }
      cur = child;
    }
  }
  // Sort: folders first, then files; alpha within group.
  const sort = (n: TreeNode) => {
    n.children.sort((a, b) => {
      const aFolder = a.fullPath === null;
      const bFolder = b.fullPath === null;
      if (aFolder !== bFolder) return aFolder ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    n.children.forEach(sort);
  };
  sort(root);
  return root;
}

function FileTree({
  root,
  selected,
  onSelect,
  filter,
}: {
  root: TreeNode;
  selected: string | null;
  onSelect: (path: string) => void;
  filter: string;
}) {
  const [expanded, setExpanded] = useState<Set<string>>(
    () => new Set(), // empty = everything collapsed; toggled below
  );
  // First mount: open every folder so the user sees something immediately.
  useEffect(() => {
    if (expanded.size > 0) return;
    const all = new Set<string>();
    const collect = (n: TreeNode, prefix: string) => {
      for (const c of n.children) {
        if (c.fullPath === null) {
          const key = prefix ? `${prefix}/${c.name}` : c.name;
          all.add(key);
          collect(c, key);
        }
      }
    };
    collect(root, "");
    setExpanded(all);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [root]);

  const q = filter.trim().toLowerCase();
  const matchesQuery = (n: TreeNode, path: string): boolean => {
    if (!q) return true;
    if (n.fullPath?.toLowerCase().includes(q)) return true;
    return n.children.some((c) =>
      matchesQuery(c, path ? `${path}/${c.name}` : c.name),
    );
  };

  const renderNode = (node: TreeNode, path: string, depth: number) => {
    if (!matchesQuery(node, path)) return null;
    if (node.fullPath !== null) {
      const active = node.fullPath === selected;
      return (
        <button
          key={node.fullPath}
          onClick={() => onSelect(node.fullPath!)}
          className={cn(
            "flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-left text-xs transition-colors",
            active
              ? "bg-accent-primary-soft text-foreground-primary"
              : "text-foreground-secondary hover:bg-surface-tertiary",
          )}
          style={{ paddingLeft: 6 + depth * 12 }}
        >
          <FileText className="size-3 shrink-0 text-foreground-tertiary" />
          <span className="truncate font-mono">{node.name}</span>
        </button>
      );
    }
    const open = expanded.has(path);
    return (
      <div key={path || "(root)"}>
        <button
          onClick={() =>
            setExpanded((prev) => {
              const next = new Set(prev);
              if (next.has(path)) next.delete(path);
              else next.add(path);
              return next;
            })
          }
          className="flex w-full items-center gap-1 rounded px-1.5 py-1 text-left text-xs text-foreground-secondary hover:bg-surface-tertiary"
          style={{ paddingLeft: 6 + depth * 12 }}
        >
          {open ? (
            <ChevronDown className="size-3 shrink-0" />
          ) : (
            <ChevronRight className="size-3 shrink-0" />
          )}
          <span className="truncate font-mono font-medium">{node.name}/</span>
        </button>
        {open &&
          node.children.map((c) =>
            renderNode(c, path ? `${path}/${c.name}` : c.name, depth + 1),
          )}
      </div>
    );
  };

  // Always expand folders when a search is active — easier to scan.
  const renderRoot = () => {
    if (q) {
      // Flatten: show every matching leaf at depth 0 with its full path.
      const matches: BlackboardEntry[] = [];
      const walk = (n: TreeNode) => {
        if (n.fullPath && n.fullPath.toLowerCase().includes(q)) {
          matches.push({ path: n.fullPath, sha256: "", at: 0, op: "" });
        }
        n.children.forEach(walk);
      };
      walk(root);
      return matches.map((m) => {
        const active = m.path === selected;
        return (
          <button
            key={m.path}
            onClick={() => onSelect(m.path)}
            className={cn(
              "flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-left text-xs",
              active
                ? "bg-accent-primary-soft text-foreground-primary"
                : "text-foreground-secondary hover:bg-surface-tertiary",
            )}
          >
            <FileText className="size-3 shrink-0 text-foreground-tertiary" />
            <span className="truncate font-mono">{m.path}</span>
          </button>
        );
      });
    }
    return root.children.map((c) => renderNode(c, c.name, 0));
  };

  return <div className="flex flex-col gap-0.5">{renderRoot()}</div>;
}

function formatTime(ms: number): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleString();
}

function shortSha(sha: string): string {
  return sha.slice(0, 8);
}

export default function ContextRoute() {
  const [entries, setEntries] = useState<BlackboardEntry[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [snap, setSnap] = useState<BlackboardSnapshot | null>(null);
  const [history, setHistory] = useState<BlackboardHistoryEntry[]>([]);
  const [filter, setFilter] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loadingDoc, setLoadingDoc] = useState(false);
  const [view, setView] = useState<"rendered" | "raw">("rendered");

  const refreshList = async () => {
    try {
      const rows = await api.listBlackboard();
      setEntries(rows);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  useEffect(() => {
    refreshList();
  }, []);

  // Hot-refresh: list every blackboard_changed; doc only if the changed
  // path matches what we're viewing.
  useSwarmFeed({
    onEvent: (ev) => {
      if (ev.type !== "blackboard_changed") return;
      refreshList();
      if (ev.path === selected) {
        api
          .readBlackboard(ev.path)
          .then(setSnap)
          .catch(() => {});
        api
          .listBlackboardHistory(ev.path, 50, false)
          .then(setHistory)
          .catch(() => {});
      }
    },
    onReconnect: () => refreshList(),
  });

  useEffect(() => {
    if (!selected) {
      setSnap(null);
      setHistory([]);
      return;
    }
    let cancelled = false;
    setLoadingDoc(true);
    Promise.all([
      api.readBlackboard(selected),
      api.listBlackboardHistory(selected, 50, false),
    ])
      .then(([s, h]) => {
        if (cancelled) return;
        setSnap(s);
        setHistory(h);
      })
      .catch((e) => {
        if (!cancelled) setError((e as Error).message);
      })
      .finally(() => {
        if (!cancelled) setLoadingDoc(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selected]);

  const tree = useMemo(() => buildTree(entries), [entries]);

  const selectedEntry = useMemo(
    () => entries.find((e) => e.path === selected) ?? null,
    [entries, selected],
  );

  return (
    <div className="flex h-full flex-col bg-surface-primary">
      {/* Header */}
      <header className="flex h-14 shrink-0 items-center gap-3 border-b border-border-subtle bg-surface-elevated px-5">
        <span className="flex size-8 items-center justify-center rounded-md bg-accent-primary-soft">
          <FileText className="size-4 text-accent-primary-deep" />
        </span>
        <div className="flex flex-col">
          <h1 className="font-heading text-sm font-semibold text-foreground-primary">
            上下文看板
          </h1>
          <span className="font-caption text-[10px] text-foreground-tertiary">
            {entries.length} 个 key · 浏览 + 历史（编辑去 /debug）
          </span>
        </div>
        <span className="flex-1" />
        <div className="flex h-8 w-60 items-center gap-2 rounded-md bg-surface-tertiary px-3">
          <Search className="size-3.5 text-foreground-tertiary" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="按 key 搜索"
            className="min-w-0 flex-1 bg-transparent text-xs text-foreground-primary placeholder:text-foreground-tertiary focus:outline-none"
          />
        </div>
        <button
          onClick={refreshList}
          className="flex size-8 items-center justify-center rounded-md bg-surface-tertiary text-foreground-secondary hover:bg-surface-secondary"
          title="刷新"
        >
          <RefreshCw className="size-4" />
        </button>
      </header>

      {/* Body */}
      <div className="flex min-h-0 flex-1">
        {/* Left — file tree */}
        <aside className="flex w-[280px] shrink-0 flex-col gap-1 overflow-y-auto border-r border-border-subtle bg-surface-secondary px-2 py-3">
          {entries.length === 0 ? (
            <p className="px-3 py-2 font-caption text-xs text-foreground-tertiary">
              暂无 blackboard 内容
            </p>
          ) : (
            <FileTree
              root={tree}
              selected={selected}
              onSelect={setSelected}
              filter={filter}
            />
          )}
        </aside>

        {/* Center — markdown */}
        <section className="flex min-w-0 flex-1 flex-col bg-surface-primary">
          <div className="flex h-11 shrink-0 items-center gap-3 border-b border-border-subtle px-5">
            {selected ? (
              <>
                <FileText className="size-4 text-foreground-tertiary" />
                <span className="truncate font-mono text-sm text-foreground-primary">
                  {selected}
                </span>
                <span className="flex-1" />
                <div className="flex rounded-md border border-border-subtle bg-surface-elevated p-0.5">
                  {(["rendered", "raw"] as const).map((v) => (
                    <button
                      key={v}
                      onClick={() => setView(v)}
                      className={cn(
                        "rounded px-2 py-0.5 text-xs",
                        view === v
                          ? "bg-accent-primary text-foreground-on-accent"
                          : "text-foreground-secondary hover:bg-surface-tertiary",
                      )}
                    >
                      {v === "rendered" ? "渲染" : "Raw"}
                    </button>
                  ))}
                </div>
              </>
            ) : (
              <span className="font-caption text-xs text-foreground-tertiary">
                左侧选一个 key 查看
              </span>
            )}
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-8 py-6">
            {error && (
              <div className="mb-3 rounded-md border border-state-danger/40 bg-status-danger-soft px-3 py-2 text-xs text-state-danger">
                {error}
              </div>
            )}
            {!selected && (
              <div className="flex h-full flex-col items-center justify-center gap-2 text-foreground-tertiary">
                <FileText className="size-10 opacity-40" />
                <p className="font-caption text-sm">没有选中任何 key</p>
              </div>
            )}
            {selected && loadingDoc && (
              <p className="font-caption text-sm text-foreground-tertiary">
                加载中…
              </p>
            )}
            {selected && !loadingDoc && snap && view === "rendered" && (
              <article className="prose-context max-w-3xl text-foreground-primary">
                <ReactMarkdown remarkPlugins={[remarkGfm]}>
                  {snap.content}
                </ReactMarkdown>
              </article>
            )}
            {selected && !loadingDoc && snap && view === "raw" && (
              <pre className="overflow-x-auto rounded-md border border-border-subtle bg-surface-tertiary p-4 font-mono text-xs text-foreground-primary">
                {snap.content}
              </pre>
            )}
          </div>
        </section>

        {/* Right — meta + history */}
        <aside className="flex w-[280px] shrink-0 flex-col gap-4 overflow-y-auto border-l border-border-subtle bg-surface-elevated p-4">
          {selectedEntry ? (
            <>
              <section>
                <h3 className="mb-2 font-heading text-[11px] font-semibold uppercase tracking-wider text-foreground-tertiary">
                  元数据
                </h3>
                <dl className="grid grid-cols-[64px_1fr] gap-y-2 font-caption text-xs">
                  <dt className="text-foreground-tertiary">path</dt>
                  <dd className="break-all font-mono text-foreground-primary">{selectedEntry.path}</dd>
                  <dt className="text-foreground-tertiary">sha256</dt>
                  <dd className="font-mono text-foreground-primary">
                    {shortSha(selectedEntry.sha256)}
                  </dd>
                  <dt className="text-foreground-tertiary">op</dt>
                  <dd className="font-mono text-foreground-primary">{selectedEntry.op}</dd>
                  <dt className="text-foreground-tertiary">at</dt>
                  <dd className="text-foreground-primary">{formatTime(selectedEntry.at)}</dd>
                  {snap && (
                    <>
                      <dt className="text-foreground-tertiary">size</dt>
                      <dd className="text-foreground-primary">{snap.content.length} B</dd>
                    </>
                  )}
                </dl>
              </section>
              <section>
                <h3 className="mb-2 flex items-center gap-1.5 font-heading text-[11px] font-semibold uppercase tracking-wider text-foreground-tertiary">
                  <History className="size-3" />
                  历史 ({history.length})
                </h3>
                <ul className="flex flex-col gap-1">
                  {history.length === 0 && (
                    <li className="font-caption text-xs text-foreground-tertiary">
                      暂无历史
                    </li>
                  )}
                  {history.map((h) => (
                    <li
                      key={h.id}
                      className="rounded border border-border-subtle bg-surface-primary px-2 py-1.5 font-caption text-[11px]"
                    >
                      <div className="flex items-center gap-2">
                        <span className="font-mono text-foreground-primary">
                          #{h.id}
                        </span>
                        <span className="text-foreground-tertiary">
                          {h.op}
                        </span>
                        <span className="flex-1" />
                        <span className="font-mono text-foreground-tertiary">
                          {shortSha(h.sha256)}
                        </span>
                      </div>
                      <div className="text-foreground-tertiary">
                        {h.agent_id ?? "system"} · {formatTime(h.at)}
                      </div>
                    </li>
                  ))}
                </ul>
              </section>
            </>
          ) : (
            <p className="font-caption text-xs text-foreground-tertiary">
              选中 key 查看元数据与历史
            </p>
          )}
        </aside>
      </div>
    </div>
  );
}
