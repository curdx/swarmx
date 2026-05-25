/**
 * CreateWizard — Pencil frame UygPU.
 *
 * Three sections rendered inline (not a stepper — the mock shows all three
 * visible at once so users can scrub freely):
 *   1. 命名 & 选色  : workspace name + accent picker
 *   2. 挂载项目目录: one or more absolute paths
 *   3. 配方         : pick a spell from /api/spells
 *
 * Submission maps to POST /api/spell/run:
 *   { name: <spell>, task: <wizard name + dirs joined>, workspace_dir: dirs[0] }
 *
 * The wizard owns no agent_id state — the server returns RunSpellResponse
 * with the spawned agent ids; we surface those to the parent via
 * onCreated() so it can re-fetch /api/agent.
 *
 * Directory pickers fall back to a text input because the web build has
 * no native dir chooser. The Tauri shell will swap this for dialog.open()
 * via a future bridge.
 */

import { useEffect, useMemo, useState } from "react";
import {
  Check,
  ChevronDown,
  FolderPlus,
  Layers,
  Plus,
  Sparkles,
  Trash2,
  Type as TypeIcon,
  X,
} from "lucide-react";
import { api } from "../../api/http";
import type { SpellInfo } from "../../api/types";
import { cn } from "@/lib/cn";

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

export function CreateWizard({ open, onClose, onCreated }: Props) {
  const [name, setName] = useState("");
  const [accent, setAccent] = useState<string>("peach");
  const [dirs, setDirs] = useState<string[]>([""]);
  const [spellName, setSpellName] = useState<string>("");
  const [spells, setSpells] = useState<SpellInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    api
      .listSpells()
      .then((rows) => {
        setSpells(rows);
        if (rows[0]) setSpellName(rows[0].name);
      })
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

  const cleanDirs = useMemo(() => dirs.map((d) => d.trim()).filter(Boolean), [dirs]);
  const canSubmit = name.trim().length > 0 && cleanDirs.length > 0 && spellName;

  const submit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    setError(null);
    try {
      await api.runSpell({
        name: spellName,
        task: `${name.trim()} — dirs: ${cleanDirs.join(", ")}`,
        workspace_dir: cleanDirs[0],
      });
      onCreated?.();
      onClose();
      // reset for next open
      setName("");
      setDirs([""]);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  if (!open) return null;

  const selectedSpell = spells.find((s) => s.name === spellName);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6"
      onClick={onClose}
    >
      <div
        className="flex max-h-full w-[680px] flex-col overflow-hidden rounded-xl bg-surface-primary shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Head */}
        <header className="flex items-center gap-4 border-b border-border-subtle bg-surface-elevated px-6 py-5">
          <span className="flex size-9 items-center justify-center rounded-md bg-accent-primary-soft">
            <FolderPlus className="size-5 text-accent-primary-deep" />
          </span>
          <div className="flex flex-col">
            <h2 className="font-heading text-base font-semibold text-foreground-primary">
              创建工作空间
            </h2>
            <span className="font-caption text-[11px] text-foreground-tertiary">
              给一群 agent 起个名、挂载目录、选一个配方
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
        <div className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto p-6">
          {error && (
            <div className="rounded-md border border-state-danger/40 bg-status-danger-soft px-3 py-2 text-xs text-state-danger">
              {error}
            </div>
          )}

          {/* Step 1: name + accent */}
          <section>
            <StepHeader n={1} label="命名 & 选色" />
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
                  placeholder="my-project"
                  className="min-w-0 flex-1 bg-transparent text-sm font-semibold text-foreground-primary placeholder:text-foreground-tertiary focus:outline-none"
                />
                {name && (
                  <span className="rounded-sm bg-accent-primary-soft px-1.5 py-0.5 font-caption text-[10px] text-accent-primary-deep">
                    AI 命名
                  </span>
                )}
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
              label="挂载项目目录"
              hint="AI 自动识别栈，无需配置"
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
                          ? "/Users/you/code/myapp"
                          : "再加一个目录（可选）"
                      }
                      className="bg-transparent font-mono text-sm text-foreground-primary placeholder:text-foreground-tertiary focus:outline-none"
                    />
                    {d.trim() && (
                      <span className="font-caption text-[10px] text-foreground-tertiary">
                        会作为 workspace_dir 传给 spell
                      </span>
                    )}
                  </div>
                  <button
                    onClick={() =>
                      setDirs((prev) => prev.filter((_, j) => j !== i))
                    }
                    disabled={dirs.length === 1}
                    className="flex size-7 items-center justify-center rounded-md text-foreground-tertiary hover:bg-surface-tertiary disabled:opacity-30"
                    title="移除"
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
                添加项目目录
              </button>
            </div>
          </section>

          {/* Step 3: spell */}
          <section>
            <StepHeader n={3} label="配方" />
            <div
              className="flex items-center gap-3.5 rounded-lg border-[1.5px] bg-surface-accent-tint px-4 py-3.5"
              style={{ borderColor: "var(--color-accent-primary)" }}
            >
              <span className="flex size-9 items-center justify-center rounded-md bg-accent-primary text-foreground-on-accent">
                <Sparkles className="size-4" />
              </span>
              <div className="flex min-w-0 flex-1 flex-col gap-1">
                <span className="truncate font-heading text-sm font-semibold text-foreground-primary">
                  {selectedSpell?.name ?? "—"}
                </span>
                <span className="line-clamp-2 font-caption text-[11px] text-foreground-secondary">
                  {selectedSpell?.description ?? "选一个配方"}
                  {selectedSpell && selectedSpell.agents.length > 0 && (
                    <>
                      {" · "}
                      {selectedSpell.agents
                        .map((a) => `${a.role}(${a.cli})`)
                        .join(" · ")}
                    </>
                  )}
                </span>
              </div>
              <div className="relative">
                <select
                  value={spellName}
                  onChange={(e) => setSpellName(e.target.value)}
                  className="appearance-none rounded-md border border-border-subtle bg-surface-elevated py-1.5 pr-7 pl-3 font-caption text-xs text-foreground-secondary focus:outline-none"
                >
                  {spells.map((s) => (
                    <option key={s.name} value={s.name}>
                      {s.name}
                    </option>
                  ))}
                </select>
                <ChevronDown className="pointer-events-none absolute top-1.5 right-1.5 size-3 text-foreground-tertiary" />
              </div>
            </div>
          </section>
        </div>

        {/* Foot */}
        <footer className="flex items-center gap-3 border-t border-border-subtle bg-surface-elevated px-6 py-4">
          <span className="flex items-center gap-1.5 font-caption text-[11px] text-foreground-tertiary">
            <Layers className="size-3" />
            {selectedSpell
              ? `将启动 ${selectedSpell.agents.length} 个 agent`
              : "AI 选了 1 个目录 · 推荐 1 个配方"}
          </span>
          <span className="flex-1" />
          <button
            onClick={onClose}
            className="rounded-md border border-border-subtle bg-surface-elevated px-4 py-2 text-xs text-foreground-secondary hover:bg-surface-tertiary"
          >
            取消
          </button>
          <button
            onClick={submit}
            disabled={!canSubmit || busy}
            className="flex items-center gap-1.5 rounded-md bg-accent-primary px-4 py-2 text-xs font-bold text-foreground-on-accent hover:bg-accent-primary-deep disabled:opacity-50"
          >
            <Check className="size-3.5" />
            {busy ? "创建中…" : "创建工作空间"}
          </button>
        </footer>
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
