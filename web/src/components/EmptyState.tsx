/**
 * EmptyState — Pencil frames tn15e (默认) + e2VtQ9 (notfound).
 *
 * Used wherever a route legitimately has no data to show (no agents, no
 * recordings, no notifications), or — variant="notfound" — when the user
 * landed on a URL that points at a missing resource.
 *
 * Centered card, peach-soft icon plate, single primary action.
 */

import { Link } from "react-router-dom";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

interface Props {
  variant?: "default" | "notfound";
  icon?: ReactNode;
  title: string;
  hint?: string;
  primaryAction?: {
    label: string;
    href?: string;
    onClick?: () => void;
  };
  secondaryAction?: {
    label: string;
    href?: string;
    onClick?: () => void;
  };
}

export function EmptyState({
  variant = "default",
  icon,
  title,
  hint,
  primaryAction,
  secondaryAction,
}: Props) {
  return (
    <div className="flex h-full w-full items-center justify-center p-10">
      <div className="flex w-full max-w-md flex-col items-center gap-6 text-center">
        {icon && (
          <span
            className={cn(
              "flex size-16 items-center justify-center rounded-2xl",
              variant === "notfound"
                ? "bg-status-warning-soft text-status-warning"
                : "bg-accent-primary-soft text-accent-primary-deep",
            )}
          >
            {icon}
          </span>
        )}
        <div className="flex flex-col gap-2">
          <h2 className="font-heading text-xl font-semibold text-foreground-primary">
            {title}
          </h2>
          {hint && (
            <p className="font-caption text-sm text-foreground-secondary">
              {hint}
            </p>
          )}
        </div>
        {(primaryAction || secondaryAction) && (
          <div className="flex items-center gap-2">
            {secondaryAction &&
              (secondaryAction.href ? (
                <Link
                  to={secondaryAction.href}
                  className="rounded-md border border-border-subtle bg-surface-elevated px-4 py-2 text-xs text-foreground-secondary hover:bg-surface-tertiary"
                >
                  {secondaryAction.label}
                </Link>
              ) : (
                <button
                  onClick={secondaryAction.onClick}
                  className="rounded-md border border-border-subtle bg-surface-elevated px-4 py-2 text-xs text-foreground-secondary hover:bg-surface-tertiary"
                >
                  {secondaryAction.label}
                </button>
              ))}
            {primaryAction &&
              (primaryAction.href ? (
                <Link
                  to={primaryAction.href}
                  className="rounded-md bg-accent-primary px-4 py-2 text-xs font-bold text-foreground-on-accent hover:bg-accent-primary-deep"
                >
                  {primaryAction.label}
                </Link>
              ) : (
                <button
                  onClick={primaryAction.onClick}
                  className="rounded-md bg-accent-primary px-4 py-2 text-xs font-bold text-foreground-on-accent hover:bg-accent-primary-deep"
                >
                  {primaryAction.label}
                </button>
              ))}
          </div>
        )}
      </div>
    </div>
  );
}
