/**
 * AgentChip — uniform agent identity render across chat, recordings,
 * blackboard history, replay player and the DAG.
 *
 * Avoids the UUID-only "claude-4ff61bda" labels that used to show up in
 * the unread popover, recording cards, etc. — users remember roles
 * (scout / planner), not 8-char hashes. The chip always leads with the
 * role, demotes the short id to a muted mono suffix.
 *
 * Variants:
 *   "inline"  (default) — role · short-id, optional small avatar disc
 *   "stacked"           — role on top, "cli · short-id" small underneath
 *                         (matches the chat sidebar member row layout)
 *   "avatar-only"       — initial circle only (for tight grids)
 *
 * Callers pass either an explicit `role`, or just `agentId` + optional
 * `roleLookup` map; resolveRole() falls back to the agent_id prefix when
 * /api/agent hasn't responded yet so the first paint isn't blank.
 */

import type { CSSProperties } from "react";
import { cn } from "@/lib/cn";
import {
  resolveRole,
  roleColorClass,
  roleInitial,
  shortAgentId,
} from "@/lib/agent";

type Variant = "inline" | "stacked" | "avatar-only";
type Size = "xs" | "sm" | "md";
type Tone = "default" | "inverse";

interface Props {
  agentId: string | null | undefined;
  /** Pre-resolved role; skip prefix-fallback when present. */
  role?: string | null;
  /** Pre-resolved CLI (claude / codex / ...) for the stacked variant's
   *  second line. Ignored elsewhere. */
  cli?: string | null;
  /** agent_id → role map. Built once at the call-site (e.g. via
   *  buildRoleLookup from listAgents()) and threaded down. */
  roleLookup?: Map<string, string> | null;
  variant?: Variant;
  size?: Size;
  /** Light/dark surface adjustment. "inverse" used on fixed dark surfaces
   *  (replay player header) that don't respect the user's theme toggle. */
  tone?: Tone;
  /** Hide the avatar disc — purely text label. */
  showAvatar?: boolean;
  /** Hide the short id suffix — role-only label. */
  showId?: boolean;
  /** Override the 8-char default. */
  idLength?: number;
  className?: string;
  /** Hover tooltip; defaults to "{role} · {full agent_id}". */
  title?: string;
  /** Optional click handler — wraps the chip in a button when set. */
  onClick?: () => void;
}

const SIZE = {
  xs: {
    avatar: "size-4 text-[9px]",
    role: "text-[11px]",
    id: "text-[10px]",
    gap: "gap-1",
  },
  sm: {
    avatar: "size-5 text-[10px]",
    role: "text-xs",
    id: "text-[10px]",
    gap: "gap-1.5",
  },
  md: {
    avatar: "size-7 text-xs",
    role: "text-sm",
    id: "text-[11px]",
    gap: "gap-2",
  },
} satisfies Record<Size, { avatar: string; role: string; id: string; gap: string }>;

export function AgentChip({
  agentId,
  role: roleProp,
  cli,
  roleLookup,
  variant = "inline",
  size = "sm",
  tone = "default",
  showAvatar = true,
  showId = true,
  idLength = 8,
  className,
  title,
  onClick,
}: Props) {
  const role = roleProp || resolveRole(agentId, roleLookup);
  const short = agentId ? shortAgentId(agentId, idLength) : "";
  const tip = title ?? (agentId ? `${role} · ${agentId}` : role);
  const sz = SIZE[size];

  const roleColor =
    tone === "inverse"
      ? "text-foreground-inverse"
      : "text-foreground-primary";
  const idColor =
    tone === "inverse"
      ? "text-foreground-inverse-secondary"
      : "text-foreground-tertiary";

  const avatar = showAvatar ? (
    <span
      className={cn(
        "flex shrink-0 items-center justify-center rounded-full font-medium text-foreground-on-accent",
        sz.avatar,
        roleColorClass(role),
      )}
      aria-hidden="true"
    >
      {roleInitial(role)}
    </span>
  ) : null;

  if (variant === "avatar-only") {
    return (
      <span className={cn("inline-flex", className)} title={tip}>
        {avatar}
      </span>
    );
  }

  const idSpan =
    showId && short ? (
      <span className={cn("font-mono", idColor, sz.id)}>{short}</span>
    ) : null;

  const inner =
    variant === "stacked" ? (
      <>
        {avatar}
        <span className="flex min-w-0 flex-col leading-tight">
          <span className={cn("truncate font-heading font-medium", roleColor, sz.role)}>
            {role}
          </span>
          {(cli || short) && (
            <span className={cn("truncate font-mono", idColor, sz.id)}>
              {cli ? `${cli}${short ? " · " : ""}${short}` : short}
            </span>
          )}
        </span>
      </>
    ) : (
      <>
        {avatar}
        <span className={cn("font-medium", roleColor, sz.role)}>{role}</span>
        {idSpan}
      </>
    );

  const baseCls = cn(
    "inline-flex items-center min-w-0",
    sz.gap,
    onClick && "cursor-pointer rounded-md px-1 py-0.5 hover:bg-surface-tertiary",
    className,
  );

  if (onClick) {
    return (
      <button
        type="button"
        onClick={onClick}
        className={baseCls}
        title={tip}
      >
        {inner}
      </button>
    );
  }

  return (
    <span className={baseCls} title={tip} style={undefined as CSSProperties | undefined}>
      {inner}
    </span>
  );
}
