/**
 * AppToaster — the app's single toast surface (Sonner), themed to the project's
 * design tokens so success / error / warning / info match the rest of the UI
 * and flip automatically with `[data-theme]`. This is the honest-feedback
 * channel: async actions report their REAL outcome here (loading → success /
 * error via toast.promise) instead of failing silently into the console.
 *
 * Sonner is the shadcn/ui-standard toast (accessible ARIA live region, keyboard
 * dismiss, swipe, reduced-motion aware, stacking + queue). We follow shadcn's
 * own theming approach: override Sonner's color CSS variables via `style` (which
 * beats its built-in light/dark blocks) and point them at our `--color-*`
 * tokens; a MutationObserver keeps Sonner's `theme` prop in sync for its close
 * button / loader.
 */
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Toaster as SonnerToaster } from "sonner";

function readTheme(): "light" | "dark" {
  return document.documentElement.dataset.theme === "dark" ? "dark" : "light";
}

export function AppToaster() {
  const { t } = useTranslation();
  const [theme, setTheme] = useState<"light" | "dark">(readTheme);
  useEffect(() => {
    // Settings flips data-theme imperatively (no event) — observe it so the
    // toaster's chrome follows light/dark live, not just on mount.
    const obs = new MutationObserver(() => setTheme(readTheme()));
    obs.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => obs.disconnect();
  }, []);

  return (
    <SonnerToaster
      theme={theme}
      // bottom-right: the enterprise convention for action feedback (Linear /
      // Vercel / Sentry) and clear of this app's top search bar; sits near the
      // composer where most actions originate.
      position="bottom-right"
      richColors
      closeButton
      expand
      visibleToasts={4}
      gap={10}
      offset={16}
      // Container ARIA label (the live region announced to screen readers).
      containerAriaLabel={t("common.notifications", { defaultValue: "通知" })}
      toastOptions={{
        closeButtonAriaLabel: t("common.closeNotification", {
          defaultValue: "关闭通知",
        }),
        classNames: {
          toast:
            "font-body shadow-lg border backdrop-blur-sm [&_[data-icon]]:shrink-0",
          title: "font-heading text-[13px] font-semibold leading-snug",
          description: "font-body text-[12px] leading-snug opacity-90",
          actionButton:
            "!rounded-md !bg-accent-primary !text-foreground-on-accent !font-caption !text-[11px] !font-medium",
          cancelButton:
            "!rounded-md !bg-surface-tertiary !text-foreground-secondary !font-caption !text-[11px]",
          closeButton:
            "!border-border-subtle !bg-surface-elevated !text-foreground-tertiary",
        },
      }}
      style={
        {
          // Map Sonner's color variables → our design tokens (inline wins over
          // Sonner's [data-sonner-theme] blocks). All resolve through
          // [data-theme], so the toaster is correct in light AND dark for free.
          "--normal-bg": "var(--color-surface-elevated)",
          "--normal-text": "var(--color-foreground-primary)",
          "--normal-border": "var(--color-border-subtle)",
          "--success-bg": "var(--color-status-success-soft)",
          "--success-text": "var(--color-status-success)",
          "--success-border": "var(--color-status-success)",
          "--error-bg": "var(--color-status-danger-soft)",
          "--error-text": "var(--color-status-danger)",
          "--error-border": "var(--color-status-danger)",
          "--warning-bg": "var(--color-status-warning-soft)",
          "--warning-text": "var(--color-state-warning)",
          "--warning-border": "var(--color-state-warning)",
          "--info-bg": "var(--color-accent-primary-soft)",
          "--info-text": "var(--color-accent-primary-deep)",
          "--info-border": "var(--color-accent-primary)",
          "--border-radius": "var(--radius-lg)",
        } as React.CSSProperties
      }
    />
  );
}
