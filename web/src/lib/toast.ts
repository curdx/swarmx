/**
 * The app's toast API — one import site (`@/lib/toast`) wrapping Sonner, so we
 * can layer project defaults / i18n here later without touching call sites.
 * Render <AppToaster/> once at the root (components/ui/sonner.tsx).
 *
 * Prefer `toast.promise` for async actions so the user sees the REAL outcome
 * instead of a silent failure:
 *
 *   toast.promise(applyThing(), {
 *     loading: "正在处理…",
 *     success: "已完成",
 *     error: (e) => `失败:${(e as Error)?.message ?? "未知错误"}`,
 *   });
 *
 * Also available: toast.success / toast.error / toast.warning / toast.info /
 * toast.loading / toast.message, each accepting { description, action, duration }.
 */
export { toast } from "sonner";
export type { ExternalToast } from "sonner";
