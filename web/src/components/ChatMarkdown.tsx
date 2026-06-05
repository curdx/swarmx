/**
 * ChatMarkdown — renders an AGENT chat bubble's markdown body.
 *
 * Why this exists: agents (Claude Code / Codex) emit GitHub-flavored markdown
 * — headings, **bold**, lists, `inline code`, fenced code blocks, tables. The
 * chat used to render `msg.body` as raw text, so every reply showed literal
 * `##` / `**` / triple-backticks. This renders it properly.
 *
 * Library choice (researched 2026): react-markdown@10 + remark-gfm (already a
 * dep, used by Context Board / Ledger) + rehype-highlight (highlight.js —
 * synchronous, no WASM, the clean fit for a client-rendered Vite/Tauri app;
 * Shiki's async model fights React's render cycle). Streamdown was rejected:
 * its headline feature (healing partial markdown during token-streaming) is
 * dead weight here because flockmux delivers whole messages, and it wants
 * shadcn tokens we don't use.
 *
 * Security: agent output is untrusted-ish (prompt-injection can make an agent
 * echo arbitrary HTML). react-markdown converts markdown → React elements with
 * no dangerouslySetInnerHTML, so `<script>` is inert by default. We additionally
 * run rehype-sanitize (default schema) as belt-and-suspenders and NEVER add
 * rehype-raw (the documented stored-XSS vector). sanitize runs BEFORE highlight
 * so highlight's hljs spans (added after) survive.
 *
 * Scope: AGENT bubbles only. User bubbles stay plain text (ChatGPT/Claude
 * convention) — developers paste paths/snippets/JSON and want them verbatim,
 * not silently italicised by a stray `*` or `_`.
 */
import { memo, useRef, useState } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeHighlight from "rehype-highlight";
import rehypeSanitize from "rehype-sanitize";
import { Check, Copy } from "lucide-react";
import { cn } from "@/lib/cn";

// Module-level constants: react-markdown re-parses when plugin/component refs
// change identity each render (also a documented flicker cause), so keep these
// stable instead of inline literals.
const REMARK_PLUGINS = [remarkGfm];
const REHYPE_PLUGINS = [rehypeSanitize, rehypeHighlight];

/** Fenced code block: the highlighted <pre><code> plus a hover copy button. */
function CodeBlock({ children }: { children?: React.ReactNode }) {
  const ref = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState(false);
  const copy = () => {
    const text = ref.current?.innerText ?? "";
    if (!text || !navigator.clipboard) return;
    navigator.clipboard.writeText(text).then(
      () => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1400);
      },
      () => {
        /* clipboard blocked — no-op */
      },
    );
  };
  return (
    <div className="group/code relative my-2">
      <pre ref={ref}>{children}</pre>
      <button
        type="button"
        onClick={copy}
        aria-label={copied ? "已复制" : "复制代码"}
        title={copied ? "已复制" : "复制代码"}
        className="absolute right-1.5 top-1.5 inline-flex size-6 items-center justify-center rounded-md border border-border-subtle bg-surface-elevated/85 text-foreground-tertiary opacity-0 backdrop-blur transition hover:text-foreground-primary focus-visible:opacity-100 group-hover/code:opacity-100"
      >
        {copied ? (
          <Check className="size-3.5 text-status-success" />
        ) : (
          <Copy className="size-3.5" />
        )}
      </button>
    </div>
  );
}

const COMPONENTS: Components = {
  // Wrap fenced blocks so we can attach the copy button; the inner highlighted
  // <code> is passed through untouched.
  pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
  // External-safe links (agent-provided URLs open in a new tab, no referrer).
  a: ({ children, ...props }) => (
    <a {...props} target="_blank" rel="noopener noreferrer">
      {children}
    </a>
  ),
};

export const ChatMarkdown = memo(function ChatMarkdown({
  content,
  className,
}: {
  content: string;
  className?: string;
}) {
  return (
    <div className={cn("prose-chat", className)}>
      <ReactMarkdown
        remarkPlugins={REMARK_PLUGINS}
        rehypePlugins={REHYPE_PLUGINS}
        components={COMPONENTS}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
});
