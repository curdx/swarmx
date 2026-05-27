/**
 * Welcome — 第一次打开 flockmux / 没选工作空间时的主介绍屏。
 *
 * 用户进 app 给你的注意力大概 10 秒：要么这 10 秒里看懂"flockmux 是啥
 * 能干啥"、按下"开始"按钮，要么关掉再不打开。所以这屏必须：
 *   1. 一句话讲清楚产品定位 (不用术语)
 *   2. 一个突出主 CTA (新建工作空间)
 *   3. 二级链接 (看文档 / 看 demo) 给犹豫派
 *
 * 同一组件 ChatHome 和 WorkspaceShell empty (wsId 不存在时) 共用，避免
 * 同一句话在两个地方说，也避免左 sidebar 大卡片 + 中间大按钮的双 CTA
 * 重复。Sidebar 的 empty 是另一个组件 (WorkspaceListEmpty) 配合，那里
 * 只是个安静的提示，主战场在中间这屏。
 */

import { useTranslation } from "react-i18next";
import { ExternalLink, MessageSquare, Sparkles, Workflow } from "lucide-react";

interface Props {
  /** 触发新建工作空间 (开 wizard)。父组件持有 wizard open state。 */
  onCreateWorkspace: () => void;
}

const HIGHLIGHTS = [
  { icon: MessageSquare, key: "welcome.highlight.chat" },
  { icon: Workflow, key: "welcome.highlight.spell" },
  { icon: Sparkles, key: "welcome.highlight.auto" },
] as const;

export function Welcome({ onCreateWorkspace }: Props) {
  const { t } = useTranslation();
  return (
    <section className="flex h-full flex-col items-center justify-center gap-8 px-8 py-12">
      <div className="flex max-w-xl flex-col items-center gap-4 text-center">
        <h1 className="font-heading text-3xl font-bold leading-tight text-foreground-primary">
          {t("welcome.title")}
        </h1>
        <p className="font-body text-base leading-relaxed text-foreground-secondary">
          {t("welcome.subtitle")}
        </p>
      </div>

      {/* 三个 highlight chip — 解释 "flockmux 跟单开一个 claude 有什么不一样"，
       *  比纯一句话 slogan 更具体，比一整段产品文档轻。 */}
      <div className="flex max-w-2xl flex-wrap items-stretch justify-center gap-3">
        {HIGHLIGHTS.map((h) => {
          const Icon = h.icon;
          return (
            <div
              key={h.key}
              className="flex max-w-[200px] flex-col items-center gap-1.5 rounded-lg border border-border-subtle bg-surface-elevated px-4 py-3 text-center"
            >
              <Icon className="size-5 text-accent-primary" />
              <span className="font-heading text-xs font-semibold text-foreground-primary">
                {t(`${h.key}.title`)}
              </span>
              <span className="font-caption text-[11px] leading-snug text-foreground-tertiary">
                {t(`${h.key}.body`)}
              </span>
            </div>
          );
        })}
      </div>

      <div className="flex flex-col items-center gap-3">
        <button
          type="button"
          onClick={onCreateWorkspace}
          className="flex h-10 items-center gap-2 rounded-lg bg-accent-primary px-5 font-heading text-sm font-semibold text-foreground-on-accent shadow-sm transition-all hover:bg-accent-primary-deep hover:shadow-md active:translate-y-px"
        >
          <Sparkles className="size-4" />
          {t("welcome.primary")}
        </button>
        <a
          href="https://github.com/curdx/flockmux-core#readme"
          target="_blank"
          rel="noreferrer"
          className="flex items-center gap-1 font-caption text-[11px] text-foreground-tertiary hover:text-foreground-secondary hover:underline"
        >
          {t("welcome.secondary")}
          <ExternalLink className="size-3" />
        </a>
      </div>
    </section>
  );
}
