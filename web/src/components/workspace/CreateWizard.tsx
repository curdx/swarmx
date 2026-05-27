/**
 * CreateWizard — 极简两步：起名字 + 选项目文件夹。
 *
 * 提交后做的事：
 *   1. 调 `runSpell("init", workspace_dir=dirs[0])` — init spell 启动一个
 *      scout agent 进目录扫一眼、写 `project.summary.<slug>` 黑板（per-
 *      workspace 命名，见 lib/workspace.ts）+ 给 user 发开场白。
 *   2. spawn 完成后立刻 listAgents 拿到 scout 的 canonical workspace 路径
 *      （macOS /tmp → /private/tmp 这类符号链接需要 canonical 才能让 chat
 *      sidebar 算出同样的 slug），用它写 `workspace.name.<slug>` = 用户起
 *      的名字。
 *   3. wizard 切 loading 视图，订阅 /ws/swarm，看到 path 以
 *      `project.summary.` 开头的事件就关闭 wizard 进群。
 *   4. 超时 / 用户跳过 → 也直接关闭进群，scout 在后台继续跑（黑板和它发给
 *      user 的开场白会自然出现在 chat 里）。
 *
 * 用户在 chat 输入第一条消息时由 ChatRoute 检测「workspace 仅有 scout 且
 * project.summary.<slug> 已存在」→ 改走 auto-dispatch 而非普通 sendMessage。
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  FolderPlus,
  Loader2,
  Plus,
  Trash2,
  Type as TypeIcon,
  X,
} from "lucide-react";
import { api } from "../../api/http";
import type { SpellInfo, SwarmEvent } from "../../api/types";
import { useSwarmFeed } from "../../hooks/useSwarmFeed";
import {
  PROJECT_SUMMARY_KEY_PREFIX,
  workspaceNameKey,
} from "../../lib/workspace";
import { cn } from "@/lib/cn";

const INIT_SPELL = "init";
const SCOUT_TIMEOUT_MS = 60_000;

const ACCENT_OPTIONS = [
  { id: "peach", color: "var(--color-accent-primary)" },
  { id: "frontend", color: "var(--color-agent-frontend)" },
  { id: "backend", color: "var(--color-agent-backend)" },
  { id: "test", color: "var(--color-agent-test)" },
  { id: "critic", color: "var(--color-agent-critic)" },
];

interface Props {
  open: boolean;
  onClose: () => void;
  onCreated?: () => void;
}

interface ScanState {
  startedAt: number;
}

export function CreateWizard({ open, onClose, onCreated }: Props) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [accent, setAccent] = useState<string>("peach");
  const [dirs, setDirs] = useState<string[]>([""]);
  const [spells, setSpells] = useState<SpellInfo[]>([]);
  const [scan, setScan] = useState<ScanState | null>(null);
  const [error, setError] = useState<string | null>(null);

  // useSwarmFeed 必须无条件调用 — 但只在 scan 进行中才处理事件。
  // ref 让 onEvent 闭包永远拿到最新的 scan 引用，不重开 WS。
  const scanRef = useRef<ScanState | null>(null);
  useEffect(() => {
    scanRef.current = scan;
  }, [scan]);

  const finishScan = useRef(() => {});
  finishScan.current = () => {
    setScan(null);
    onCreated?.();
    onClose();
    // reset 用户输入，让下次打开是空的
    setName("");
    setDirs([""]);
    setError(null);
  };

  useSwarmFeed({
    onEvent: (ev: SwarmEvent) => {
      const cur = scanRef.current;
      if (!cur) return;
      // 用 prefix 匹配（不精确匹配 slug）— wizard 用用户填的原始 path 算 slug，
      // 但 scout 写黑板用的是 server canonicalize 过的 cwd（macOS /tmp ↔
      // /private/tmp 不一致），slug 对不上。同一时刻只有一个 scout 在跑，
      // 看到任何 project.summary.* 写入就当成本次的完成信号。
      if (
        ev.type === "blackboard_changed" &&
        ev.path.startsWith(PROJECT_SUMMARY_KEY_PREFIX) &&
        ev.at >= cur.startedAt
      ) {
        finishScan.current();
      }
    },
  });

  useEffect(() => {
    if (!open) return;
    api
      .listSpells()
      .then((rows) => setSpells(rows))
      .catch((e) => setError((e as Error).message));
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  // scan 超时兜底：scout 因为 LLM 不可用 / 目录权限等问题没有写黑板，60s
  // 后也直接进群，让用户能看到失败状态、自己处理。
  useEffect(() => {
    if (!scan) return;
    const timer = window.setTimeout(() => {
      if (scanRef.current?.startedAt === scan.startedAt) {
        finishScan.current();
      }
    }, SCOUT_TIMEOUT_MS);
    return () => window.clearTimeout(timer);
  }, [scan]);

  const cleanDirs = useMemo(() => dirs.map((d) => d.trim()).filter(Boolean), [dirs]);
  const canSubmit = name.trim().length > 0 && cleanDirs.length > 0 && !scan;
  const hasInitSpell = spells.some((s) => s.name === INIT_SPELL);

  const submit = async () => {
    if (!canSubmit) return;
    setError(null);
    const startedAt = Date.now();
    const wsName = name.trim();
    setScan({ startedAt });
    try {
      if (!hasInitSpell) {
        throw new Error(
          "后端未加载 `init` spell — 请重启 flockmux-server 让它发现 spells/init.md",
        );
      }
      const resp = await api.runSpell({
        name: INIT_SPELL,
        task: wsName,
        workspace_dir: cleanDirs[0],
      });
      // 写 workspace.name.<slug> 让 chat sidebar 显示用户起的名字。slug 必须
      // 用 canonical path（spawn_agent ensure_shared_workspace 调过
      // canonicalize），所以先从 listAgents 把刚 spawn 的 scout 取回来，
      // 用它的 workspace 字段算 slug。失败 fallback 用户输入的原始路径 —
      // sidebar 拿不到 name 时也只是 fallback basename，不致命。
      const scoutId = resp.agents[0]?.agent_id;
      let canonicalPath: string = cleanDirs[0];
      if (scoutId) {
        try {
          const all = await api.listAgents();
          const sc = all.find((a) => a.agent_id === scoutId);
          if (sc?.workspace) canonicalPath = sc.workspace;
        } catch {
          /* best-effort */
        }
      }
      api
        .writeBlackboard(workspaceNameKey(canonicalPath), {
          content: wsName,
        })
        .catch(() => {
          /* best-effort — sidebar fallback 到 basename，没 name 也能用 */
        });
    } catch (e) {
      setScan(null);
      setError((e as Error).message);
    }
  };

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6">
      <div className="flex max-h-full w-[680px] flex-col overflow-hidden rounded-xl bg-surface-primary shadow-2xl">
        {/* Head */}
        <header className="flex items-center gap-4 border-b border-border-subtle bg-surface-elevated px-6 py-5">
          <span className="flex size-9 items-center justify-center rounded-md bg-accent-primary-soft">
            <FolderPlus className="size-5 text-accent-primary-deep" />
          </span>
          <div className="flex flex-col">
            <h2 className="font-heading text-base font-semibold text-foreground-primary">
              {t("wizard.title")}
            </h2>
            <span className="font-caption text-[11px] text-foreground-tertiary">
              {t("wizard.subtitle")}
            </span>
          </div>
          <span className="flex-1" />
          <button
            onClick={onClose}
            className="flex size-8 items-center justify-center rounded-md bg-surface-tertiary text-foreground-secondary hover:bg-surface-secondary"
          >
            <X className="size-4" />
          </button>
        </header>

        {/* Body */}
        {scan ? (
          <ScanView
            label={t("wizard.scanning")}
            hint={t("wizard.scanningHint")}
          />
        ) : (
          <div className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto p-6">
            {error && (
              <div className="rounded-md border border-state-danger/40 bg-status-danger-soft px-3 py-2 text-xs text-state-danger">
                {error}
              </div>
            )}

            {/* Step 1: name + accent */}
            <section>
              <StepHeader n={1} label={t("wizard.step1")} />
              <div className="flex items-center gap-3">
                <div
                  className="flex h-11 flex-1 items-center gap-3 rounded-md border-[1.5px] bg-surface-elevated px-3.5"
                  style={{ borderColor: "var(--color-accent-primary)" }}
                >
                  <TypeIcon className="size-3.5 text-foreground-tertiary" />
                  <input
                    autoFocus
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder={t("wizard.namePlaceholder")}
                    className="min-w-0 flex-1 bg-transparent text-sm font-semibold text-foreground-primary placeholder:text-foreground-tertiary focus:outline-none"
                  />
                </div>
                <div className="flex items-center gap-1.5">
                  {ACCENT_OPTIONS.map((opt) => (
                    <button
                      key={opt.id}
                      onClick={() => setAccent(opt.id)}
                      className={cn(
                        "size-7 rounded-full transition-transform",
                        accent === opt.id
                          ? "ring-2 ring-foreground-primary ring-offset-2"
                          : "hover:scale-110",
                      )}
                      style={{ background: opt.color }}
                      title={opt.id}
                    />
                  ))}
                </div>
              </div>
            </section>

            {/* Step 2: dirs */}
            <section>
              <StepHeader
                n={2}
                label={t("wizard.step2")}
                hint={t("wizard.step2Hint")}
              />
              <div className="flex flex-col gap-2">
                {dirs.map((d, i) => (
                  <div
                    key={i}
                    className="flex items-center gap-3 rounded-lg border border-border-subtle bg-surface-elevated px-3.5 py-3 shadow-sm"
                  >
                    <span
                      className={cn(
                        "flex size-9 items-center justify-center rounded-md font-mono text-xs font-bold text-foreground-on-accent",
                        i === 0
                          ? "bg-agent-frontend"
                          : i === 1
                            ? "bg-agent-backend"
                            : "bg-agent-test",
                      )}
                    >
                      {i + 1}
                    </span>
                    <div className="flex min-w-0 flex-1 flex-col">
                      <input
                        value={d}
                        onChange={(e) =>
                          setDirs((prev) =>
                            prev.map((x, j) => (j === i ? e.target.value : x)),
                          )
                        }
                        placeholder={
                          i === 0
                            ? t("wizard.dirPlaceholder1")
                            : t("wizard.dirPlaceholderMore")
                        }
                        className="bg-transparent font-mono text-sm text-foreground-primary placeholder:text-foreground-tertiary focus:outline-none"
                      />
                      {d.trim() && (
                        <span className="font-caption text-[10px] text-foreground-tertiary">
                          {t("wizard.dirHint")}
                        </span>
                      )}
                    </div>
                    <button
                      onClick={() =>
                        setDirs((prev) => prev.filter((_, j) => j !== i))
                      }
                      disabled={dirs.length === 1}
                      className="flex size-7 items-center justify-center rounded-md text-foreground-tertiary hover:bg-surface-tertiary disabled:opacity-30"
                      title={t("wizard.removeDir")}
                    >
                      <Trash2 className="size-3.5" />
                    </button>
                  </div>
                ))}
                <button
                  onClick={() => setDirs((prev) => [...prev, ""])}
                  className="flex items-center justify-center gap-2 rounded-lg border-[1.5px] border-dashed border-border-strong bg-transparent px-4 py-3 font-caption text-xs text-foreground-secondary hover:bg-surface-tertiary"
                >
                  <span className="flex size-7 items-center justify-center rounded-md bg-accent-primary-soft text-accent-primary-deep">
                    <Plus className="size-4" />
                  </span>
                  {t("wizard.addDir")}
                </button>
              </div>
            </section>
          </div>
        )}

        {/* Foot */}
        <footer className="flex items-center gap-3 border-t border-border-subtle bg-surface-elevated px-6 py-4">
          <span className="flex items-center gap-1.5 font-caption text-[11px] text-foreground-tertiary">
            {scan ? t("wizard.scanningFootHint") : t("wizard.defaultInfo")}
          </span>
          <span className="flex-1" />
          {scan ? (
            <button
              onClick={() => finishScan.current()}
              className="rounded-md border border-border-subtle bg-surface-elevated px-4 py-2 text-xs text-foreground-secondary hover:bg-surface-tertiary"
            >
              {t("wizard.enterAnyway")}
            </button>
          ) : (
            <>
              <button
                onClick={onClose}
                className="rounded-md border border-border-subtle bg-surface-elevated px-4 py-2 text-xs text-foreground-secondary hover:bg-surface-tertiary"
              >
                {t("wizard.cancel")}
              </button>
              <button
                onClick={submit}
                disabled={!canSubmit}
                className="flex items-center gap-1.5 rounded-md bg-accent-primary px-4 py-2 text-xs font-bold text-foreground-on-accent hover:bg-accent-primary-deep disabled:opacity-50"
              >
                <Check className="size-3.5" />
                {t("wizard.create")}
              </button>
            </>
          )}
        </footer>
      </div>
    </div>
  );
}

function ScanView({ label, hint }: { label: string; hint: string }) {
  return (
    <div className="flex min-h-[280px] flex-1 flex-col items-center justify-center gap-4 p-10 text-center">
      <span className="flex size-14 items-center justify-center rounded-full bg-accent-primary-soft text-accent-primary-deep">
        <Loader2 className="size-7 animate-spin" />
      </span>
      <h3 className="font-heading text-base font-semibold text-foreground-primary">
        {label}
      </h3>
      <p className="max-w-[420px] font-body text-[13px] leading-relaxed text-foreground-secondary">
        {hint}
      </p>
    </div>
  );
}

function StepHeader({
  n,
  label,
  hint,
}: {
  n: number;
  label: string;
  hint?: string;
}) {
  return (
    <div className="mb-3 flex items-center gap-2">
      <span className="flex size-[18px] items-center justify-center rounded-full bg-accent-primary font-heading text-[10px] font-bold text-foreground-on-accent">
        {n}
      </span>
      <span className="font-heading text-[13px] font-bold text-foreground-primary">
        {label}
      </span>
      {hint && (
        <>
          <span className="flex-1" />
          <span className="font-caption text-[11px] text-foreground-tertiary">
            {hint}
          </span>
        </>
      )}
    </div>
  );
}
