/**
 * Workspace `<select>` shared by the global tool pages. Extracted from the cron
 * page's inline picker so files / terminal / tasks / usage scope by the same
 * control. `allowAll` prepends an "all workspaces" option (value "") for the
 * read-only aggregate pages (tasks / usage); files / terminal omit it since a
 * concrete workspace is required.
 */
import { useTranslation } from "react-i18next";
import type { Workspace } from "@/api/types";
import { cn } from "@/lib/cn";

export function WorkspacePicker({
  workspaces,
  value,
  onChange,
  allowAll = false,
  className,
}: {
  workspaces: Workspace[];
  value: string;
  onChange: (id: string) => void;
  allowAll?: boolean;
  className?: string;
}) {
  const { t } = useTranslation();
  return (
    <select
      aria-label={t("workspace.picker")}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className={cn(
        "rounded border border-border-subtle bg-surface-primary px-2 py-1 text-[13px] text-foreground-primary",
        className,
      )}
    >
      {allowAll && <option value="">{t("common.allWorkspaces")}</option>}
      {!allowAll && workspaces.length === 0 && <option value="">—</option>}
      {workspaces.map((w) => (
        <option key={w.id} value={w.id}>
          {w.name}
        </option>
      ))}
    </select>
  );
}
