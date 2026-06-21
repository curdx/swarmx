/**
 * ChatMarkdown — renders an AGENT chat bubble's markdown body.
 *
 * Why this exists: agents (Claude Code / Codex) emit GitHub-flavored markdown
 * — headings, **bold**, lists, `inline code`, fenced code blocks, tables. The
 * chat used to render `msg.body` as raw text, so every reply showed literal
 * `##` / `**` / triple-backticks. This renders it properly.
 *
 * Library choice (researched 2026): react-markdown@10 + remark-gfm + rehype-
 * highlight (highlight.js — synchronous, no WASM, the clean fit for a client-
 * rendered Vite/Tauri app). Streamdown rejected (its streaming-heal feature is
 * dead weight — we deliver whole messages — and it wants shadcn tokens).
 *
 * Security: agent output is untrusted-ish (prompt-injection can make an agent
 * echo arbitrary HTML). react-markdown converts markdown → React elements with
 * no dangerouslySetInnerHTML, so `<script>` in prose is inert by default. We
 * additionally run rehype-sanitize and NEVER add rehype-raw (the documented
 * stored-XSS vector). The HTML *preview* (below) is the one place agent HTML
 * runs — and it runs in a locked-down null-origin sandboxed iframe.
 *
 * Scope: AGENT bubbles only. User bubbles stay plain text (ChatGPT/Claude
 * convention) — developers paste paths/snippets/JSON and want them verbatim.
 */
import { memo, useEffect, useRef, useState, type ReactNode } from "react";
import { isValidElement } from "react";
import { useTranslation } from "react-i18next";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeHighlight from "rehype-highlight";
import rehypeSanitize from "rehype-sanitize";
import { Check, Code2, Copy, ExternalLink, Eye, Loader2 } from "lucide-react";
import { cn } from "@/lib/cn";
import { MarkdownInput, MarkdownLink } from "@/lib/markdownLinks";

// Module-level constants: react-markdown re-parses when plugin/component refs
// change identity each render, so keep these stable.
const REMARK_PLUGINS = [remarkGfm];
const REHYPE_PLUGINS = [rehypeSanitize, rehypeHighlight];

/** Recursively flatten a React node tree back to its text. rehype-highlight
 *  turns a code block's body into nested <span class="hljs-…"> elements, so the
 *  raw source isn't a plain prop — we walk it to recover the text for copy +
 *  the HTML preview srcdoc. */
function extractText(node: ReactNode): string {
  if (node == null || node === false || node === true) return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(extractText).join("");
  if (isValidElement(node)) {
    return extractText((node.props as { children?: ReactNode }).children);
  }
  return "";
}

// In-srcdoc CSP for the HTML preview: allow inline styles/scripts (they run in a
// null origin, can't touch swarmx) + images + a small whitelist of common
// CDNs agents reach for, but block all network egress (connect-src 'none') so a
// previewed page can't exfiltrate. Mirrors Claude Artifacts' CDN-whitelist +
// no-network stance.
const PREVIEW_CSP = [
  "default-src 'none'",
  "img-src data: blob: https: http:",
  "media-src data: blob: https: http:",
  "style-src 'unsafe-inline' https:",
  "font-src https: data:",
  "script-src 'unsafe-inline' 'unsafe-eval' https://cdn.jsdelivr.net https://cdnjs.cloudflare.com https://unpkg.com https://cdn.tailwindcss.com",
  "connect-src 'none'",
  "frame-src 'none'",
].join("; ");

/** Wrap an HTML fragment in a minimal document and ensure our CSP <meta> is
 *  present (injected into an existing <head>/<html>, or a fresh wrapper). */
function buildSrcDoc(code: string): string {
  const cspMeta = `<meta http-equiv="Content-Security-Policy" content="${PREVIEW_CSP}">`;
  if (/<head[\s>]/i.test(code)) {
    return code.replace(/<head([^>]*)>/i, `<head$1>${cspMeta}`);
  }
  if (/<html[\s>]/i.test(code)) {
    return code.replace(/<html([^>]*)>/i, `<html$1><head>${cspMeta}</head>`);
  }
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark">${cspMeta}</head><body>${code}</body></html>`;
}

/** Sandboxed HTML preview. Renders agent-generated HTML in a null-origin iframe
 *  (sandbox WITHOUT allow-same-origin → can't reach parent cookies/DOM/storage;
 *  no allow-forms/top-navigation; allow-popups inherits the sandbox). srcdoc is
 *  set imperatively via ref so the frame only reloads when the code changes. */
function HtmlPreview({ code }: { code: string }) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  useEffect(() => {
    if (iframeRef.current) iframeRef.current.srcdoc = buildSrcDoc(code);
  }, [code]);
  return (
    <div
      className="my-2 overflow-hidden rounded-lg border border-border-subtle bg-white"
      style={{ height: 380, resize: "vertical" }}
    >
      <iframe
        ref={iframeRef}
        title="HTML preview"
        // allow-scripts → interactivity in a NULL origin (no parent cookies/DOM/
        // storage). allow-modals so demos' alert()/confirm() work. allow-popups
        // so target=_blank works (popup inherits the sandbox). Deliberately NO
        // allow-same-origin (sandbox-escape), NO allow-forms (form action could
        // POST to an external URL = exfil), NO allow-top-navigation.
        sandbox="allow-scripts allow-popups allow-modals"
        referrerPolicy="no-referrer"
        allow="camera 'none'; microphone 'none'; geolocation 'none'; payment 'none'; usb 'none'"
        className="h-full w-full border-0 bg-white"
      />
    </div>
  );
}

/** SVG preview. Rendered via an <img> data-URL, NOT inline/iframe-with-scripts:
 *  the browser disables scripts AND external loads inside an <img>, so even a
 *  hostile SVG (`<script>`, `onload`, external `<use>`) is inert. Safest path
 *  for agent-authored SVG, zero deps. */
function SvgPreview({ code }: { code: string }) {
  const { t } = useTranslation();
  const [failed, setFailed] = useState(false);
  const src = `data:image/svg+xml;utf8,${encodeURIComponent(code)}`;
  if (failed) {
    return (
      <div className="my-2 rounded-lg border border-border-subtle bg-surface-tertiary px-3 py-4 text-center font-caption text-[11px] text-foreground-tertiary">
        {t("messages.previewError")}
      </div>
    );
  }
  return (
    <div className="my-2 flex justify-center overflow-auto rounded-lg border border-border-subtle bg-white p-3">
      <img
        src={src}
        alt="SVG preview"
        onError={() => setFailed(true)}
        className="max-h-[420px] max-w-full"
      />
    </div>
  );
}

// Module counter for unique mermaid render ids (must be valid DOM/CSS ids —
// React's useId yields colons that mermaid chokes on).
let mermaidSeq = 0;

/** Mermaid diagram preview. mermaid.js is lazy-imported (large) so it only loads
 *  when a ```mermaid block is actually previewed. securityLevel:'strict' +
 *  htmlLabels:false sanitize the output (no scripts/HTML labels) before we inject
 *  it. Falls back to the code on a parse error. */
function MermaidPreview({ code }: { code: string }) {
  const { t } = useTranslation();
  const [svg, setSvg] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setSvg(null);
    setFailed(false);
    (async () => {
      try {
        const mermaid = (await import("mermaid")).default;
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          // The preview container is always white, so use the light theme
          // regardless of the app's dark/light mode (a dark-theme diagram on a
          // white canvas reads as a mismatch).
          theme: "default",
          flowchart: { htmlLabels: false },
        });
        const { svg } = await mermaid.render(`mmd-${(mermaidSeq += 1)}`, code);
        if (!cancelled) setSvg(svg);
      } catch {
        if (!cancelled) setFailed(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [code]);
  if (failed) {
    return (
      <div className="my-2 rounded-lg border border-border-subtle bg-surface-tertiary px-3 py-4 text-center font-caption text-[11px] text-foreground-tertiary">
        {t("messages.previewError")}
      </div>
    );
  }
  if (svg == null) {
    return (
      <div className="my-2 flex items-center justify-center rounded-lg border border-border-subtle bg-white py-8">
        <Loader2 className="size-4 animate-spin text-foreground-tertiary" />
      </div>
    );
  }
  return (
    <div
      className="my-2 flex justify-center overflow-auto rounded-lg border border-border-subtle bg-white p-3 [&_svg]:max-w-full"
      // mermaid 'strict' output is sanitized (scripts/handlers stripped).
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}

/** Fenced code block: highlighted <pre> + copy button, and — for ```html — a
 *  "代码 | 预览" toggle that renders the HTML in the sandboxed iframe above. */
function CodeBlock({
  lang,
  raw,
  children,
}: {
  lang: string;
  raw: string;
  children?: ReactNode;
}) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const previewable = lang === "html" || lang === "svg" || lang === "mermaid";
  // HTML may be a whole app → default to Code (read first). SVG/Mermaid are
  // visual artifacts → default to the rendered Preview.
  const [view, setView] = useState<"code" | "preview">(
    lang === "svg" || lang === "mermaid" ? "preview" : "code",
  );

  const copy = () => {
    if (!raw || !navigator.clipboard) return;
    navigator.clipboard.writeText(raw).then(
      () => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1400);
      },
      () => {},
    );
  };

  const openExternal = () => {
    const blob = new Blob([buildSrcDoc(raw)], { type: "text/html" });
    const url = URL.createObjectURL(blob);
    window.open(url, "_blank", "noopener,noreferrer");
    window.setTimeout(() => URL.revokeObjectURL(url), 30_000);
  };

  return (
    <div className="group/code relative my-2">
      <div className="absolute right-1.5 top-1.5 z-10 flex items-center gap-1 opacity-0 transition group-hover/code:opacity-100 focus-within:opacity-100">
        {previewable && (
          <div className="flex overflow-hidden rounded-md border border-border-subtle bg-surface-elevated/85 backdrop-blur">
            <button
              type="button"
              onClick={() => setView("code")}
              className={cn(
                "flex items-center gap-1 px-1.5 py-0.5 text-[10px] transition-colors",
                view === "code"
                  ? "bg-accent-primary text-foreground-on-accent"
                  : "text-foreground-tertiary hover:text-foreground-primary",
              )}
            >
              <Code2 className="size-3" />
              {t("messages.codeTab")}
            </button>
            <button
              type="button"
              onClick={() => setView("preview")}
              className={cn(
                "flex items-center gap-1 px-1.5 py-0.5 text-[10px] transition-colors",
                view === "preview"
                  ? "bg-accent-primary text-foreground-on-accent"
                  : "text-foreground-tertiary hover:text-foreground-primary",
              )}
            >
              <Eye className="size-3" />
              {t("messages.previewTab")}
            </button>
          </div>
        )}
        {lang === "html" && view === "preview" && (
          <button
            type="button"
            onClick={openExternal}
            aria-label={t("messages.openInNewTab")}
            title={t("messages.openInNewTab")}
            className="inline-flex size-6 items-center justify-center rounded-md border border-border-subtle bg-surface-elevated/85 text-foreground-tertiary backdrop-blur transition hover:text-foreground-primary"
          >
            <ExternalLink className="size-3.5" />
          </button>
        )}
        <button
          type="button"
          onClick={copy}
          aria-label={copied ? t("messages.copied") : t("messages.copyCode")}
          title={copied ? t("messages.copied") : t("messages.copyCode")}
          className="inline-flex size-6 items-center justify-center rounded-md border border-border-subtle bg-surface-elevated/85 text-foreground-tertiary backdrop-blur transition hover:text-foreground-primary"
        >
          {copied ? (
            <Check className="size-3.5 text-status-success" />
          ) : (
            <Copy className="size-3.5" />
          )}
        </button>
      </div>
      {previewable && view === "preview" ? (
        lang === "html" ? (
          <HtmlPreview code={raw} />
        ) : lang === "svg" ? (
          <SvgPreview code={raw} />
        ) : (
          <MermaidPreview code={raw} />
        )
      ) : (
        <pre>{children}</pre>
      )}
    </div>
  );
}

const COMPONENTS: Components = {
  // Wrap fenced blocks so we can attach the copy button + (for html) a preview
  // toggle. The child is the highlighted <code>; we pull its language + raw text.
  pre: ({ children }) => {
    const codeEl = isValidElement(children) ? children : null;
    const className =
      (codeEl?.props as { className?: string } | undefined)?.className ?? "";
    const lang = /language-([\w-]+)/.exec(className)?.[1]?.toLowerCase() ?? "";
    const raw = codeEl
      ? extractText((codeEl.props as { children?: ReactNode }).children)
      : extractText(children);
    return (
      <CodeBlock lang={lang} raw={raw}>
        {children}
      </CodeBlock>
    );
  },
  // External-safe links (agent-provided URLs open in a new tab, no referrer).
  a: MarkdownLink,
  input: MarkdownInput,
};

// LLMs sometimes wrap their ENTIRE reply in a single ```markdown / ```md fence
// (a known habit when asked to "output markdown"). Rendered verbatim that shows
// one giant grey code block instead of the formatted prose. Strip a SINGLE
// outer md/markdown fence that encloses the whole message; leave everything
// else (real code blocks, partial fences) untouched. Pure + conservative.
function unwrapOuterMarkdownFence(s: string): string {
  const m = s.match(/^\s*```(?:md|markdown)[^\n]*\n([\s\S]*?)\n```\s*$/);
  if (!m) return s;
  const inner = m[1];
  // Only unwrap if the inner body has no UNbalanced fence of its own — i.e. the
  // outer pair really is the whole-message wrapper, not the first of several.
  const fenceCount = (inner.match(/^```/gm) || []).length;
  return fenceCount % 2 === 0 ? inner : s;
}

// Guard rail: a pathologically long bubble (an agent pastes a multi-MB blob,
// or echoes a huge tool result) would otherwise render to thousands of DOM
// nodes and freeze the tab. Cap the markdown we feed react-markdown and show an
// honest notice — swarmx normally renders short labels, not raw payloads, so
// this only ever trips on outliers, but the freeze it prevents is total.
const MAX_RENDER_CHARS = 100_000;

export const ChatMarkdown = memo(function ChatMarkdown({
  content,
  className,
}: {
  content: string;
  className?: string;
}) {
  const { t } = useTranslation();
  const repaired = unwrapOuterMarkdownFence(content);
  const tooLong = repaired.length > MAX_RENDER_CHARS;
  const body = tooLong ? repaired.slice(0, MAX_RENDER_CHARS) : repaired;
  return (
    <div className={cn("prose-chat", className)}>
      <ReactMarkdown
        remarkPlugins={REMARK_PLUGINS}
        rehypePlugins={REHYPE_PLUGINS}
        components={COMPONENTS}
      >
        {body}
      </ReactMarkdown>
      {tooLong && (
        <div className="mt-1 rounded border border-border-subtle bg-surface-tertiary px-2 py-1 font-caption text-[11px] text-foreground-tertiary">
          {t("messages.truncatedNotice", { n: MAX_RENDER_CHARS.toLocaleString() })}
        </div>
      )}
    </div>
  );
});
