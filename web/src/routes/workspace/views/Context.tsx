/**
 * Context view — blackboard browser inside WorkspaceShell.
 *
 * Strips the previous /context route's header + WorkspaceScopeBar.
 * Selected key + search live in URL state.
 *
 * wsSlug comes from Shell's workspace (we already have a canonical path),
 * so this view doesn't need to listAgents again to figure out which slug
 * to filter blackboard keys by.
 */

import { useEffect, useMemo, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router-dom";
import {
  ChevronDown,
  ChevronRight,
  FileText,
  History,
  RefreshCw,
  Search,
} from "lucide-react";
import { api } from "../../../api/http";
import type {
  AgentInfo,
  BlackboardEntry,
  BlackboardHistoryEntry,
  BlackboardSnapshot,
} from "../../../api/types";
import { useSwarmFeed } from "../../../hooks/useSwarmFeed";
import { Button } from "@/components/ui/button";
import { workspaceSlug } from "../../../lib/workspace";
import { buildRoleLookup } from "@/lib/agent";
import { AgentChip } from "@/components/agent/AgentChip";
import { cn } from "@/lib/cn";
import { useWorkspaceContext } from "../Shell";
import { MarkdownInput, MarkdownLink } from "@/lib/markdownLinks";

interface TreeNode {
  name: string;
  fullPath: string | null;
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

function stripWsSuffix(key: string, slug: string | null): string {
  if (!slug) return key;
  const suffix = `.${slug}`;
  return key.endsWith(suffix) ? key.slice(0, -suffix.length) : key;
}

function FileTree({
  root,
  selected,
  onSelect,
  filter,
  wsSlug,
}: {
  root: TreeNode;
  selected: string | null;
  onSelect: (path: string) => void;
  filter: string;
  wsSlug: string | null;
}) {
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
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
          <span className="truncate font-mono">
            {stripWsSuffix(node.name, wsSlug)}
          </span>
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

  const renderRoot = () => {
    if (q) {
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
            <span className="truncate font-mono">
              {stripWsSuffix(m.path, wsSlug)}
            </span>
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

export default function ContextView() {
  const { t } = useTranslation();
  const { workspace } = useWorkspaceContext();
  const wsSlug = useMemo(() => workspaceSlug(workspace.path), [workspace.path]);

  const [searchParams, setSearchParams] = useSearchParams();
  const selected = searchParams.get("key");
  const filter = searchParams.get("q") ?? "";

  const setSelected = (key: string | null) => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (key) next.set("key", key);
        else next.delete("key");
        return next;
      },
      { replace: true },
    );
  };
  const setFilter = (q: string) => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (q) next.set("q", q);
        else next.delete("q");
        return next;
      },
      { replace: true },
    );
  };

  const [allEntries, setAllEntries] = useState<BlackboardEntry[]>([]);
  const [snap, setSnap] = useState<BlackboardSnapshot | null>(null);
  const [history, setHistory] = useState<BlackboardHistoryEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loadingDoc, setLoadingDoc] = useState(false);
  const [view, setView] = useState<"rendered" | "raw">("rendered");
  const [roleLookup, setRoleLookup] = useState<Map<string, string>>(
    () => new Map(),
  );

  useEffect(() => {
    let cancelled = false;
    api
      .listAgents()
      .then((agents: AgentInfo[]) => {
        if (cancelled) return;
        setRoleLookup(buildRoleLookup(agents));
      })
      // L4: a failed role lookup leaves blackboard entries showing raw agent-id
      // prefixes instead of role names, with no hint why — at least log it.
      .catch((e) => {
        // eslint-disable-next-line no-console
        console.warn("[flockmux] role lookup (listAgents) failed", e);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const entries = useMemo(() => {
    // workspace 已知 → 直接过滤含 slug 的 key（namespaced 写入）。全局 key
    // 在 wsId 模式下显示不了 — 跟之前一致。
    return allEntries.filter((e) => e.path.includes(wsSlug));
  }, [allEntries, wsSlug]);

  const refreshList = async () => {
    try {
      const rows = await api.listBlackboard();
      setAllEntries(rows);
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  useEffect(() => {
    refreshList();
  }, []);

  useSwarmFeed({
    onEvent: (ev) => {
      if (ev.type !== "blackboard_changed") return;
      refreshList();
      if (ev.path === selected) {
        // L3: don't swallow — a failed re-read after a "changed" event leaves
        // the editor showing stale content while the panel claims it synced.
        api
          .readBlackboard(ev.path)
          .then(setSnap)
          .catch((e) => {
            // eslint-disable-next-line no-console
            console.warn(`[flockmux] blackboard re-read failed (${ev.path})`, e);
          });
        api
          .listBlackboardHistory(ev.path, 50, false)
          .then(setHistory)
          .catch((e) => {
            // eslint-disable-next-line no-console
            console.warn(`[flockmux] blackboard history reload failed (${ev.path})`, e);
          });
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
    <div className="flex min-h-0 flex-1 flex-col bg-surface-primary">
      {/* sub-header: search + refresh */}
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-border-subtle bg-surface-secondary px-5">
        <span className="font-caption text-[11px] text-foreground-tertiary">
          {t("context.subtitle", { count: entries.length })}
        </span>
        <span className="flex-1" />
        <div className="flex h-8 w-60 items-center gap-2 rounded-md bg-surface-primary px-3 transition-shadow focus-within:ring-2 focus-within:ring-ring/50">
          <Search className="size-3.5 text-foreground-tertiary" />
          <input
            name="context-search"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder={t("context.search")}
            className="min-w-0 flex-1 bg-transparent text-xs text-foreground-primary placeholder:text-foreground-tertiary focus:outline-none"
          />
        </div>
        <Button
          variant="ghost"
          size="icon"
          onClick={refreshList}
          title={t("common.refresh")}
          className="size-8"
        >
          <RefreshCw className="size-3.5" />
        </Button>
      </div>

      <div className="flex min-h-0 flex-1">
        <aside className="flex w-[280px] shrink-0 flex-col gap-1 overflow-y-auto border-r border-border-subtle bg-surface-secondary px-2 py-3">
          {entries.length === 0 ? (
            <p className="px-3 py-2 font-caption text-xs text-foreground-tertiary">
              {t("context.empty")}
            </p>
          ) : (
            <FileTree
              root={tree}
              selected={selected}
              onSelect={setSelected}
              filter={filter}
              wsSlug={wsSlug}
            />
          )}
        </aside>

        <section className="flex min-w-0 flex-1 flex-col bg-surface-primary">
          <div className="flex h-11 shrink-0 items-center gap-3 border-b border-border-subtle px-5">
            {selected ? (
              <>
                <FileText className="size-4 text-foreground-tertiary" />
                <span
                  className="truncate font-mono text-sm text-foreground-primary"
                  title={selected}
                >
                  {stripWsSuffix(selected, wsSlug)}
                </span>
                {selected.endsWith(`.${wsSlug}`) && (
                  <span
                    className="shrink-0 rounded bg-surface-tertiary px-1.5 py-0.5 font-caption text-[10px] text-foreground-tertiary"
                    title={`workspace · ${wsSlug}`}
                  >
                    ws
                  </span>
                )}
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
                      {v === "rendered" ? t("context.rendered") : t("context.raw")}
                    </button>
                  ))}
                </div>
              </>
            ) : (
              <span className="font-caption text-xs text-foreground-tertiary">
                {t("context.selectKey")}
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
                <p className="font-caption text-sm">{t("context.noSelection")}</p>
              </div>
            )}
            {selected && loadingDoc && (
              <p className="font-caption text-sm text-foreground-tertiary">
                {t("common.loading")}
              </p>
            )}
            {selected && !loadingDoc && snap && view === "rendered" && (
              <article className="prose-context max-w-3xl text-foreground-primary">
                <ReactMarkdown
                  remarkPlugins={[remarkGfm]}
                  components={{ a: MarkdownLink, input: MarkdownInput }}
                >
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

        <aside className="flex w-[280px] shrink-0 flex-col gap-4 overflow-y-auto border-l border-border-subtle bg-surface-elevated p-4">
          {selectedEntry ? (
            <>
              <section>
                <h3 className="mb-2 font-heading text-[11px] font-semibold uppercase tracking-wider text-foreground-tertiary">
                  {t("context.meta")}
                </h3>
                <dl className="grid grid-cols-[64px_1fr] gap-y-2 font-caption text-xs">
                  <dt className="text-foreground-tertiary">path</dt>
                  <dd
                    className="break-all font-mono text-foreground-primary"
                    title={selectedEntry.path}
                  >
                    {stripWsSuffix(selectedEntry.path, wsSlug)}
                  </dd>
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
                      <dt className="text-foreground-tertiary">{t("context.size")}</dt>
                      <dd className="text-foreground-primary">{snap.content.length} B</dd>
                    </>
                  )}
                </dl>
              </section>
              <section>
                <h3 className="mb-2 flex items-center gap-1.5 font-heading text-[11px] font-semibold uppercase tracking-wider text-foreground-tertiary">
                  <History className="size-3" />
                  {t("context.history", { count: history.length })}
                </h3>
                <ul className="flex flex-col gap-1">
                  {history.length === 0 && (
                    <li className="font-caption text-xs text-foreground-tertiary">
                      {t("context.noHistory")}
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
                        <span className="text-foreground-tertiary">{h.op}</span>
                        <span className="flex-1" />
                        <span className="font-mono text-foreground-tertiary">
                          {shortSha(h.sha256)}
                        </span>
                      </div>
                      <div className="flex items-center gap-1.5 text-foreground-tertiary">
                        {h.agent_id ? (
                          <AgentChip
                            agentId={h.agent_id}
                            roleLookup={roleLookup}
                            size="xs"
                            showAvatar={false}
                          />
                        ) : (
                          <span className="font-mono">system</span>
                        )}
                        <span>· {formatTime(h.at)}</span>
                      </div>
                    </li>
                  ))}
                </ul>
              </section>
            </>
          ) : (
            <p className="font-caption text-xs text-foreground-tertiary">
              {t("context.rightHint")}
            </p>
          )}
        </aside>
      </div>
    </div>
  );
}
