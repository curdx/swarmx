/**
 * WorkspaceScopeBar — 4 个 ws-scoped 全局视图 (DAG / Replays / Context /
 * Notifications) 顶部共享的一行。
 *
 * 两种状态：
 *   - 带 ?ws=<id>：显示「← 返回 <name>」回 chat workspace + 当前 ws 名
 *     + accent 圆点。点链接回到 /chat/:wsId 让用户带着 context 切回聊天。
 *   - 不带 ?ws：显示「当前显示所有工作空间数据」提示，告诉用户这是
 *     "看全部"模式 (从 ⌘K 命令面板进来这里)。
 *
 * 数据来源：listAgents + listBlackboard，自己 fetch 一次拿 workspace 名 +
 * accent。轻量 (一个 page 只 mount 一次)，避免给每个 sub view 强加 prop
 * 链。Stale 时刷新由父组件触发自己 listAgents 时顺带触发。
 */

import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { Info } from "lucide-react";
import { api } from "../../api/http";
import type { AgentInfo } from "../../api/types";
import {
  accentToCssVar,
  WORKSPACE_ACCENT_KEY_PREFIX,
  WORKSPACE_NAME_KEY_PREFIX_VALUE,
  workspaceSlug,
} from "../../lib/workspace";

interface Props {
  /** ?ws=<id> 从 URL query 拿；id = workspace path 的最后 8 字符。
   *  null = 全局模式 (来自 ⌘K 或老 nav)。 */
  wsId: string | null;
}

export function WorkspaceScopeBar({ wsId }: Props) {
  const { t } = useTranslation();
  const [info, setInfo] = useState<{ path: string; name: string; accent: string } | null>(
    null,
  );

  useEffect(() => {
    if (!wsId) return;
    let cancelled = false;
    (async () => {
      try {
        const agents = await api.listAgents();
        const matched = agents.find(
          (a: AgentInfo) => (a.workspace ?? "").slice(-8) === wsId,
        );
        if (!matched || !matched.workspace) return;
        const path = matched.workspace;
        const slug = workspaceSlug(path);
        const [nameSnap, accentSnap] = await Promise.all([
          api
            .readBlackboard(`${WORKSPACE_NAME_KEY_PREFIX_VALUE}${slug}`)
            .catch(() => null),
          api
            .readBlackboard(`${WORKSPACE_ACCENT_KEY_PREFIX}${slug}`)
            .catch(() => null),
        ]);
        if (cancelled) return;
        // 没拿到 name 就用 path 末段，跟 chat sidebar 同 fallback 路径。
        const fallbackName = path.split("/").slice(-1)[0] || path;
        setInfo({
          path,
          name: nameSnap?.content?.trim() || fallbackName,
          accent: accentToCssVar(accentSnap?.content),
        });
      } catch {
        // best-effort
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  const back = useMemo(() => (wsId ? `/chat/${wsId}` : "/chat"), [wsId]);

  if (!wsId) {
    return (
      <div className="flex shrink-0 items-center gap-2 border-b border-border-subtle bg-status-warning-soft px-5 py-2 font-caption text-[11px] text-foreground-secondary">
        <Info className="size-3.5 text-status-warning" />
        <span>{t("chat.globalScopeHint")}</span>
      </div>
    );
  }

  return (
    <div className="flex shrink-0 items-center gap-3 border-b border-border-subtle bg-surface-secondary px-5 py-2">
      <Link
        to={back}
        className="font-caption text-[11px] text-foreground-secondary hover:text-foreground-primary"
      >
        {t("chat.backToChat", { name: info?.name ?? "…" })}
      </Link>
      {info && (
        <>
          <span
            className="size-2 shrink-0 rounded-full"
            style={{ background: info.accent }}
          />
          <span className="font-mono text-[11px] text-foreground-tertiary">
            {info.path}
          </span>
        </>
      )}
    </div>
  );
}
