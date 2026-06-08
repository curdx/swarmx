/**
 * Image-path detection + URL helper for chat previews.
 *
 * Chat messages reference screenshots by absolute filesystem PATH (the same
 * string the agents read — Claude Code inline, Codex `-i`). We scan a message /
 * composer body for those paths so the UI can render a thumbnail next to the
 * text. The bytes are served by the backend `GET /api/file?path=` endpoint
 * (browsers can't load `file:///` from an http origin).
 */
import { HTTP_BASE } from "./apiBase";
import { apiRoutes } from "@/api/endpoints";

const IMG_EXT = "png|jpe?g|gif|webp|bmp|avif|svg|ico";

// Two shapes, both ABSOLUTE (start with `/`): a backtick-wrapped path (allows
// spaces, e.g. `/Users/me/My Shots/a.png`) or a bare path (no spaces). We only
// match absolute paths — a relative one has no stable base to resolve against,
// and `~` isn't expanded server-side. The `(?<![\w:/])` lookbehind keeps us off
// the `//` in `http://…` and mid-token slashes.
const IMAGE_PATH_RE = new RegExp(
  "`(/[^`\\n]+?\\.(?:" +
    IMG_EXT +
    "))`" +
    "|(?<![\\w:/])(/[^\\s`'\"<>()]+\\.(?:" +
    IMG_EXT +
    "))",
  "gi",
);

/** Distinct absolute image paths referenced in `text`, in first-seen order. */
export function extractImagePaths(text: string): string[] {
  if (!text || text.indexOf("/") === -1) return [];
  const out: string[] = [];
  const seen = new Set<string>();
  IMAGE_PATH_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = IMAGE_PATH_RE.exec(text)) !== null) {
    const p = m[1] ?? m[2];
    if (p && !seen.has(p)) {
      seen.add(p);
      out.push(p);
    }
  }
  return out;
}

const IMG_EXT_RE = new RegExp(`\\.(?:${IMG_EXT})$`, "i");

/** True if `path` ends in an image extension the backend `/api/file` serves. */
export function isImagePath(path: string): boolean {
  return IMG_EXT_RE.test(path);
}

/** Backend URL that streams the local image at `path` (works in browser + Tauri
 *  via HTTP_BASE). */
export function fileUrl(path: string): string {
  return `${HTTP_BASE}${apiRoutes.files.serve(path)}`;
}

/** Basename of a path, for alt text / chip labels. */
export function baseName(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}
