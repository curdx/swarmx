/**
 * i18n config — react-i18next + raw JSON dictionaries.
 *
 * Initial language pulled from settings localStorage (so user choice
 * survives reload). Settings panel calls `i18n.changeLanguage(...)` on
 * toggle and only that string round-trips.
 *
 * Adding a new namespace key: edit both zh.json + en.json and you're
 * done — TypeScript is loose here so missing keys fall back to the
 * literal key string (Trans / t will render the path verbatim, which
 * makes typos visible at a glance).
 */

import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import zh from "./locales/zh.json";
import en from "./locales/en.json";

const STORAGE_KEY = "flockmux:settings:v1";

function readLang(): "zh" | "en" {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return "zh";
    const parsed = JSON.parse(raw) as { lang?: string };
    return parsed.lang === "en" ? "en" : "zh";
  } catch {
    return "zh";
  }
}

i18n.use(initReactI18next).init({
  resources: {
    zh: { translation: zh },
    en: { translation: en },
  },
  lng: readLang(),
  fallbackLng: "zh",
  interpolation: { escapeValue: false },
});

export default i18n;
