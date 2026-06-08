/**
 * OnboardingTour — 4 步介绍 modal，第一次建完 workspace 进 chat 时弹。
 *
 * 不做精确 spotlight 镂空 anchor 到具体 DOM (那是 driver.js / react-joyride
 * 干的活，引一个库带 portal 体积不值)。做最简 modal carousel：每步一个
 * icon + 标题 + 一行说明，"下一步" / "跳过"。Linear / Notion / Cursor 第
 * 一次启动都是这套路，效果跟 spotlight 接近，工程量减半。
 *
 * 触发条件：用户在 /chat/:wsId/* 下 (=已经有 workspace 了) + localStorage
 * 里 flockmux:tour:onboarding-v1 没标 seen。跳过 / 走完都会 mark seen，
 * 不会再弹。版本号 v1 — 如果以后改 tour 内容可 bump v2 让老用户也再看一次。
 */

import { useEffect, useState } from "react";
import { useLocation } from "react-router-dom";
import { useTranslation } from "react-i18next";
import {
  ChevronLeft,
  ChevronRight,
  Command,
  FileText,
  Layers,
  MessageSquare,
  X,
  type LucideIcon,
} from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/cn";

const STORAGE_KEY = "flockmux:tour:onboarding-v1";

interface Step {
  icon: LucideIcon;
  titleKey: string;
  bodyKey: string;
}

const STEPS: Step[] = [
  { icon: Layers, titleKey: "tour.sidebar.title", bodyKey: "tour.sidebar.body" },
  { icon: FileText, titleKey: "tour.tabs.title", bodyKey: "tour.tabs.body" },
  { icon: MessageSquare, titleKey: "tour.composer.title", bodyKey: "tour.composer.body" },
  { icon: Command, titleKey: "tour.cmdk.title", bodyKey: "tour.cmdk.body" },
];

function hasSeen(): boolean {
  try {
    return window.localStorage.getItem(STORAGE_KEY) === "1";
  } catch {
    return false;
  }
}

function markSeen() {
  try {
    window.localStorage.setItem(STORAGE_KEY, "1");
  } catch {
    /* ignore */
  }
}

/** Reset hook for debug: window.flockmux.resetTour() — handy when QA-ing. */
if (typeof window !== "undefined") {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (window as any).flockmux = (window as any).flockmux || {};
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (window as any).flockmux.resetTour = () => {
    try {
      window.localStorage.removeItem(STORAGE_KEY);
      // eslint-disable-next-line no-console
      console.log("[tour] seen flag cleared — reload to see tour again");
    } catch {
      /* ignore */
    }
  };
}

export function OnboardingTour() {
  const { t } = useTranslation();
  const location = useLocation();
  const [open, setOpen] = useState(false);
  const [step, setStep] = useState(0);

  // 延迟一拍触发 — 让 ChatView 先把消息列表渲染出来，用户先看到正主，
  // 再被 modal 盖一层，否则连背景都没渲染就弹 modal 体验突兀。
  useEffect(() => {
    if (hasSeen()) return;
    if (new URLSearchParams(location.search).has("agent")) return;
    const tm = window.setTimeout(() => setOpen(true), 400);
    return () => window.clearTimeout(tm);
  }, [location.search]);

  const finish = () => {
    markSeen();
    setOpen(false);
  };

  const isLast = step === STEPS.length - 1;
  const isFirst = step === 0;
  const cur = STEPS[step];
  const Icon = cur.icon;

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) finish();
      }}
    >
      <DialogContent
        className="max-w-md gap-0 overflow-hidden p-0"
        showCloseButton={false}
      >
        <DialogHeader className="sr-only">
          <DialogTitle>{t("tour.dialogTitle")}</DialogTitle>
          <DialogDescription>{t("tour.dialogDesc")}</DialogDescription>
        </DialogHeader>

        {/* 顶部 progress dots — 表明 4 步进度 */}
        <div className="flex items-center justify-between border-b border-border-subtle px-5 py-3">
          <div className="flex items-center gap-1.5">
            {STEPS.map((_, i) => (
              <span
                key={i}
                className={cn(
                  "h-1.5 rounded-full transition-all",
                  i === step
                    ? "w-6 bg-accent-primary"
                    : i < step
                      ? "w-1.5 bg-accent-primary-deep"
                      : "w-1.5 bg-border-strong",
                )}
              />
            ))}
            <span className="ml-2 font-caption text-[10px] text-foreground-tertiary">
              {step + 1} / {STEPS.length}
            </span>
          </div>
          <button
            type="button"
            onClick={finish}
            className="flex size-6 items-center justify-center rounded-md text-foreground-tertiary transition-colors hover:bg-surface-tertiary hover:text-foreground-primary"
            aria-label={t("tour.skip")}
          >
            <X className="size-3.5" />
          </button>
        </div>

        {/* 主体 — 大 icon + 标题 + body */}
        <div className="flex flex-col items-center gap-3 px-6 py-8 text-center">
          <span className="flex size-12 items-center justify-center rounded-xl bg-accent-primary-soft text-accent-primary-deep">
            <Icon className="size-6" />
          </span>
          <h2 className="font-heading text-lg font-bold text-foreground-primary">
            {t(cur.titleKey)}
          </h2>
          <p className="max-w-sm font-body text-sm leading-relaxed text-foreground-secondary">
            {t(cur.bodyKey)}
          </p>
        </div>

        {/* 底部按钮行 */}
        <div className="flex items-center justify-between border-t border-border-subtle bg-surface-secondary px-5 py-3">
          <button
            type="button"
            onClick={finish}
            className="font-caption text-xs text-foreground-tertiary hover:text-foreground-secondary"
          >
            {t("tour.skip")}
          </button>
          <div className="flex items-center gap-2">
            {!isFirst && (
              <button
                type="button"
                onClick={() => setStep((s) => Math.max(0, s - 1))}
                className="flex h-8 items-center gap-1 rounded-md border border-border-subtle bg-surface-elevated px-3 text-xs text-foreground-secondary transition-colors hover:bg-surface-tertiary"
              >
                <ChevronLeft className="size-3.5" />
                {t("tour.prev")}
              </button>
            )}
            <button
              type="button"
              onClick={() => {
                if (isLast) finish();
                else setStep((s) => s + 1);
              }}
              className="flex h-8 items-center gap-1.5 rounded-md bg-accent-primary px-3 text-xs font-medium text-foreground-on-accent shadow-sm transition-colors hover:bg-accent-primary-deep"
            >
              {isLast ? t("tour.done") : t("tour.next")}
              {!isLast && <ChevronRight className="size-3.5" />}
            </button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
