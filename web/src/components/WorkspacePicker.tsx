/**
 * Workspace `<select>` shared by the global tool pages. Extracted from the cron
 * page's inline picker so files / terminal / tasks / usage scope by the same
 * control. `allowAll` prepends an "all workspaces" option (value "") for the
 * read-only aggregate pages (tasks / usage); files / terminal omit it since a
 * concrete workspace is required.
 */
import { useTranslation } from "react-i18next";
import type { Workspace } from "@/api/types";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/cn";

const ALL_WORKSPACES_VALUE = "__all_workspaces__";
const NO_WORKSPACE_VALUE = "__no_workspace__";

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
  const selectValue = allowAll
    ? value || ALL_WORKSPACES_VALUE
    : value || NO_WORKSPACE_VALUE;
  return (
    <Select
      name="workspace"
      value={selectValue}
      onValueChange={(next) => {
        if (allowAll && next === ALL_WORKSPACES_VALUE) {
          onChange("");
          return;
        }
        if (!allowAll && next === NO_WORKSPACE_VALUE) {
          onChange("");
          return;
        }
        onChange(next);
      }}
    >
      <SelectTrigger
        aria-label={t("workspace.picker")}
        className={cn("min-w-[160px] text-[13px]", className)}
      >
        <SelectValue
          placeholder={
            allowAll ? t("common.allWorkspaces") : workspaces.length === 0 ? "—" : ""
          }
        />
      </SelectTrigger>
      <SelectContent>
        {allowAll && (
          <SelectItem value={ALL_WORKSPACES_VALUE}>
            {t("common.allWorkspaces")}
          </SelectItem>
        )}
        {!allowAll && workspaces.length === 0 && (
          <SelectItem value={NO_WORKSPACE_VALUE}>—</SelectItem>
        )}
        {workspaces.map((w) => (
          <SelectItem key={w.id} value={w.id}>
            {w.name}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
