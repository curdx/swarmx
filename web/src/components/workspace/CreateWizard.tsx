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
import { api, ApiError } from "../../api/http";
import type { SpellInfo, SwarmEvent, Workspace } from "../../api/types";
import { useSwarmFeed } from "../../hooks/useSwarmFeed";
import {
  ACCENT_OPTIONS,
  PROJECT_SUMMARY_KEY_PREFIX,
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


interface Props {
  open: boolean;
  onClose: () => void;
  /** Called when scan finishes (or times out). Receives the workspace
   *  that was just created so the parent can navigate the user into it
   *  — wizard itself is layer-agnostic and does not pull react-router.
   *  May be undefined if `createWorkspace` failed before the row was
   *  persisted; parents should fall back to a plain refresh in that case. */
  onCreated?: (workspace?: Workspace) => void;
}

interface ScanState {
  startedAt: number;
  workspace: Workspace;
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
    const ws = scanRef.current?.workspace;
    setScan(null);
    onCreated?.(ws);
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
    let created: Workspace | null = null;
    try {
      if (!hasInitSpell) {
        throw new Error(
          "后端未加载 `init` spell — 请重启 flockmux-server 让它发现 spells/init.md",
        );
      }
      // workspace-as-first-class refactor: workspace is created in the
      // DB BEFORE the init spell launches. The scout that init spawns
      // inherits the workspace_id via the spell-runner's reverse-lookup
      // (rest.rs::run_spell with workspace_id). Name + accent are
      // workspace table columns now, no blackboard writes needed.
      created = await api.createWorkspace({
        name: wsName,
        cwd: cleanDirs[0],
        accent,
      });
      // Stash the created workspace on scan state so finishScan can hand
      // it back to the parent for routing into the new chat URL — without
      // this the parent only knew "something was created, refresh."
      setScan({ startedAt, workspace: created });
      await api.runSpell({
        name: INIT_SPELL,
        task: wsName,
        workspace_dir: cleanDirs[0],
        workspace_id: created.id,
      });
    } catch (e) {
      setScan(null);
      // Roll back a half-created workspace: if createWorkspace succeeded but
      // runSpell failed (e.g. the cwd doesn't exist), the workspace row is
      // already persisted. Without this the user is left with a dead,
      // 0-member "ghost" workspace pointing at a bad path that they'd have to
      // delete by hand. (The backend also validates the cwd up-front now, but
      // this guards every other runSpell failure too.)
      if (created) {
        api.deleteWorkspace(created.id).catch(() => {});
      }
      // Show the server's plain error string, not the `METHOD path → status`
      // wrapper — friendlier for "目录不存在" style validation failures.
      setError(e instanceof ApiError ? e.detail : (e as Error).message);
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
            startedAt={scan.startedAt}
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
                {/* 5 个标识色 — 之前裸着 5 个圆，新手不知道是干啥的。加
                 *  小 label 说明用途（多 workspace 时一眼区分谁是谁）。 */}
                <div className="flex flex-col items-end gap-1">
                  <span className="font-caption text-[10px] text-foreground-tertiary">
                    {t("wizard.accentLabel")}
                  </span>
                  <div className="flex items-center gap-1.5">
                    {ACCENT_OPTIONS.map((opt) => {
                      const colorName = t(opt.nameKey);
                      return (
                        <button
                          key={opt.id}
                          type="button"
                          onClick={() => setAccent(opt.id)}
                          className={cn(
                            "size-6 rounded-full transition-transform",
                            accent === opt.id
                              ? "ring-2 ring-foreground-primary ring-offset-2"
                              : "hover:scale-110",
                          )}
                          style={{ background: opt.cssVar }}
                          title={t("wizard.accentTitle", { name: colorName })}
                          aria-label={t("wizard.accentTitle", { name: colorName })}
                        />
                      );
                    })}
                  </div>
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
                            ? IS_TAURI
                              ? t("wizard.dirPlaceholder1Tauri")
                              : t("wizard.dirPlaceholder1")
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
                    {/* Picker button — Tauri 下打开原生文件夹 dialog；
                     *  浏览器 dev / preview 模式下 disabled + tooltip 解
                     *  释。之前直接 hide，用户根本不知道桌面 app 能直接
                     *  选，audit 时被点出来"看不到选择按钮"。*/}
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={!IS_TAURI}
                      onClick={async () => {
                        if (!IS_TAURI) return;
                        const picked = await pickDirectoryViaTauri();
                        if (picked) {
                          setDirs((prev) =>
                            prev.map((x, j) => (j === i ? picked : x)),
                          );
                        }
                      }}
                      title={
                        IS_TAURI
                          ? t("wizard.pickFolder")
                          : t("wizard.pickFolderUnavailable")
                      }
                      className="h-8 shrink-0 gap-1.5 px-2.5 text-xs"
                    >
                      <FolderOpen className="size-3.5" />
                      {t("wizard.pickFolderShort")}
                    </Button>
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

function ScanView({
  label,
  hint,
  startedAt,
}: {
  label: string;
  hint: string;
  startedAt: number;
}) {
  const { t } = useTranslation();
  // 实时 elapsed 秒数 — 用户等 30 秒 spinner 转着不动会怀疑挂了，给个
  // 实时跳秒 + 隐式进度条让他知道 "在动、快好"。
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(id);
  }, []);
  const elapsedMs = Math.max(0, now - startedAt);
  const elapsedSec = Math.floor(elapsedMs / 1000);
  // 平均完成时间 ~25s；按 30s 当 90% (留 10% 给最后一公里，看着没卡住)。
  // 超过 30s 后吸到 95% 静止，等真正完成 effect 关闭 modal。
  const pct = Math.min(95, Math.floor((elapsedMs / 30_000) * 90));

  return (
    <div className="flex min-h-[300px] flex-1 flex-col items-center justify-center gap-4 p-10 text-center">
      <span className="flex size-14 items-center justify-center rounded-full bg-accent-primary-soft text-accent-primary-deep">
        <Loader2 className="size-7 animate-spin" />
      </span>
      <h3 className="font-heading text-base font-semibold text-foreground-primary">
        {label}
      </h3>
      <p className="max-w-[420px] font-body text-[13px] leading-relaxed text-foreground-secondary">
        {hint}
      </p>
      {/* 进度条 + 实时秒数。进度条是基于平均时长的"心理安抚条"，不精确
       *  跟后端 scout 真实进度挂钩 (后端没暴露阶段事件)。 */}
      <div className="flex w-full max-w-[320px] flex-col gap-1.5">
        <div className="h-1.5 w-full overflow-hidden rounded-full bg-surface-tertiary">
          <div
            className="h-full rounded-full bg-accent-primary transition-all duration-500 ease-out"
            style={{ width: `${pct}%` }}
          />
        </div>
        <span className="font-mono text-[10px] text-foreground-tertiary">
          {t("wizard.scanningElapsed", { s: elapsedSec })}
        </span>
      </div>
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
