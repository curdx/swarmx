import { useCallback, useState } from "react";
import { getThemeMode, setTheme, type ThemeMode } from "./theme";

const STORAGE_KEY = "swarmx:settings:v1";

/**
 * 主题快捷切换的读写口。主题的运行时应用由 `theme.ts` 负责(data-theme +
 * 跟随系统监听),这里只管把选择写回 `swarmx:settings:v1` blob —— 那是
 * Settings 页拥有的同一个存储(schema 归它),所以读-改-写整个 blob 以
 * 保留 lang / 其他偏好字段,而不是只写 theme 键覆盖。
 *
 * 局限(已知可接受):同标签页内若 Settings 页正开着,它的主题行要重挂载
 * 才反映这次切换 —— 但 `data-theme` 全局即时生效,视觉零延迟。
 */
export function useThemePreference(): [ThemeMode, (m: ThemeMode) => void] {
  const [mode, setMode] = useState<ThemeMode>(() => getThemeMode());
  const set = useCallback((m: ThemeMode) => {
    try {
      const raw = window.localStorage.getItem(STORAGE_KEY);
      const blob = raw ? (JSON.parse(raw) as Record<string, unknown>) : {};
      blob.theme = m;
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(blob));
    } catch {
      /* ignore — runtime apply below still works for this session */
    }
    setTheme(m);
    setMode(m);
  }, []);
  return [mode, set];
}
