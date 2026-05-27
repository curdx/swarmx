/**
 * CreateWizard — 极简两步：起名字 + 选项目文件夹。
 *
 * UI 走 shadcn primitives (Dialog / Button / Input / Label)。Dialog 自带
 * focus trap + portal + aria-modal + ESC 关闭。我们额外禁掉 backdrop 点
 * 关闭（用户填了一半 path 不小心点旁边会丢全部输入，体验巨差）。
 *
 * 提交后的事：
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
  FolderOpen,
  FolderPlus,
  Loader2,
  Plus,
  Trash2,
} from "lucide-react";
import { api } from "../../api/http";
import type { SpellInfo, SwarmEvent } from "../../api/types";
import { useSwarmFeed } from "../../hooks/useSwarmFeed";
import {
  PROJECT_SUMMARY_KEY_PREFIX,
  workspaceNameKey,
} from "../../lib/workspace";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/cn";

const INIT_SPELL = "init";
const SCOUT_TIMEOUT_MS = 60_000;

// Tauri runtime detection — only the desktop shell exposes
// __TAURI_INTERNALS__ on window. Vite dev (plain browser) is undefined,
// so we hide the "选择文件夹" button there (browser security sandbox
// can't return an absolute filesystem path anyway, so the button would
// be cosmetic).
const IS_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// Tauri 2.x plugin-dialog. Bring it in via dynamic import so vite-dev (no
// Tauri runtime) doesn't try to bundle the native bridge at module init.
async function pickDirectoryViaTauri(): Promise<string | null> {
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const result = await open({
      directory: true,
      multiple: false,
      title: "选择项目文件夹",
    });
    if (typeof result === "string") return result;
    return null;
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn("[wizard] dialog.open failed", err);
    return null;
  }
}

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

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
    >
      <DialogContent
        showCloseButton={!scan}
        className="flex max-h-[90vh] w-[680px] max-w-[680px] flex-col gap-0 overflow-hidden p-0 sm:max-w-[680px]"
        // 禁掉 backdrop / outside click 关闭 — 用户填了一半路径，不小心点空白
        // 全部清空体验巨差。ESC + ✕ + 取消 三个显式入口仍然能关。
        onInteractOutside={(e) => e.preventDefault()}
        // 扫描中禁 ESC，避免误触关了 wizard 但 scout 仍在后台跑。loading 视图
        // 有 "直接进群" 按钮显式离开。
        onEscapeKeyDown={(e) => {
          if (scan) e.preventDefault();
        }}
      >
        <DialogHeader className="flex flex-row items-center gap-3 border-b border-border-subtle bg-surface-elevated px-6 py-5">
          <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-accent-primary-soft">
            <FolderPlus className="size-5 text-accent-primary-deep" />
          </span>
          <div className="flex min-w-0 flex-col text-left">
            <DialogTitle className="font-heading text-base font-semibold text-foreground-primary">
              {t("wizard.title")}
            </DialogTitle>
            <DialogDescription className="font-caption text-[11px] text-foreground-tertiary">
              {t("wizard.subtitle")}
            </DialogDescription>
          </div>
        </DialogHeader>

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
            <section className="flex flex-col gap-3">
              <StepHeader n={1} label={t("wizard.step1")} />
              <div className="flex items-center gap-3">
                <div className="flex min-w-0 flex-1 flex-col gap-1">
                  <Label htmlFor="wizard-name" className="sr-only">
                    {t("wizard.step1")}
                  </Label>
                  <Input
                    id="wizard-name"
                    autoFocus
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder={t("wizard.namePlaceholder")}
                    className="h-10"
                  />
                </div>
                <div className="flex items-center gap-1.5">
                  {ACCENT_OPTIONS.map((opt) => (
                    <button
                      key={opt.id}
                      type="button"
                      onClick={() => setAccent(opt.id)}
                      className={cn(
                        "size-7 rounded-full transition-transform",
                        accent === opt.id
                          ? "ring-2 ring-foreground-primary ring-offset-2"
                          : "hover:scale-110",
                      )}
                      style={{ background: opt.color }}
                      title={opt.id}
                      aria-label={`accent ${opt.id}`}
                    />
                  ))}
                </div>
              </div>
            </section>

            {/* Step 2: dirs */}
            <section className="flex flex-col gap-3">
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
                        "flex size-9 shrink-0 items-center justify-center rounded-md font-mono text-xs font-bold text-foreground-on-accent",
                        i === 0
                          ? "bg-agent-frontend"
                          : i === 1
                            ? "bg-agent-backend"
                            : "bg-agent-test",
                      )}
                    >
                      {i + 1}
                    </span>
                    <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                      <Input
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
                        className="h-8 border-none bg-transparent px-0 font-mono text-sm shadow-none focus-visible:ring-0"
                      />
                      {d.trim() && (
                        <span className="font-caption text-[10px] text-foreground-tertiary">
                          {t("wizard.dirHint")}
                        </span>
                      )}
                    </div>
                    {IS_TAURI && (
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={async () => {
                          const picked = await pickDirectoryViaTauri();
                          if (picked) {
                            setDirs((prev) =>
                              prev.map((x, j) => (j === i ? picked : x)),
                            );
                          }
                        }}
                        title={t("wizard.pickFolder")}
                        className="h-8 shrink-0 gap-1.5 px-2.5 text-xs"
                      >
                        <FolderOpen className="size-3.5" />
                        {t("wizard.pickFolderShort")}
                      </Button>
                    )}
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={() =>
                        setDirs((prev) => prev.filter((_, j) => j !== i))
                      }
                      disabled={dirs.length === 1}
                      title={t("wizard.removeDir")}
                      className="size-7 text-foreground-tertiary"
                    >
                      <Trash2 className="size-3.5" />
                    </Button>
                  </div>
                ))}
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => setDirs((prev) => [...prev, ""])}
                  className="h-auto justify-center gap-2 rounded-lg border-[1.5px] border-dashed border-border-strong bg-transparent py-3 text-xs text-foreground-secondary hover:bg-surface-tertiary"
                >
                  <span className="flex size-7 items-center justify-center rounded-md bg-accent-primary-soft text-accent-primary-deep">
                    <Plus className="size-4" />
                  </span>
                  {t("wizard.addDir")}
                </Button>
              </div>
            </section>
          </div>
        )}

        {/* 不用 shadcn DialogFooter — 它默认有 `-mx-4 -mb-4 bg-muted/50
            rounded-b-xl border-t`，是给标准 DialogContent p-4 用的，假设
            footer 通过负 margin 顶到 content 边缘。我们的 DialogContent
            是 p-0 + 自己 header/body/footer 控制 padding，那套负 margin
            会把 footer 顶出 modal 边界 16px，配合 overflow-hidden 把
            border-t / rounded-b 都裁掉，看上去 footer 像"飘"在外面没
            分隔线。用普通 div + 我们自己的 border-t / bg / padding 即可。 */}
        <div className="flex flex-row items-center gap-3 border-t border-border-subtle bg-surface-elevated px-6 py-4">
          <span className="font-caption text-[11px] text-foreground-tertiary">
            {scan ? t("wizard.scanningFootHint") : t("wizard.defaultInfo")}
          </span>
          <span className="flex-1" />
          {scan ? (
            <Button variant="outline" onClick={() => finishScan.current()}>
              {t("wizard.enterAnyway")}
            </Button>
          ) : (
            <>
              <Button variant="outline" onClick={onClose}>
                {t("wizard.cancel")}
              </Button>
              <Button onClick={submit} disabled={!canSubmit}>
                <Check className="size-3.5" />
                {t("wizard.create")}
              </Button>
            </>
          )}
        </div>
      </DialogContent>
    </Dialog>
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
    <div className="flex items-center gap-2">
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
