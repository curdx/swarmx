import { Fragment, type ComponentPropsWithoutRef, type ReactNode } from "react";

/** GFM autolinks can swallow trailing CJK/full-width punctuation, e.g.
 *  `http://localhost:5173（若...` becomes one broken href. Strip only the
 *  punctuation/explainer tail from autolink-like hrefs; authored markdown links
 *  with normal query/hash text are left intact. */
const STOP_RE = /[（【「《，。；、\u3000\s]/u;
const TRAILING_PUNCT_RE = /[)\]}>.,;:!?，。；：！？、]+$/u;

function safeDecodeUri(value: string): string {
  try {
    return decodeURI(value);
  } catch {
    return value;
  }
}

function splitMarkdownHref(href?: string): {
  href?: string;
  trailingText: string;
} {
  if (!href) return { href, trailingText: "" };
  const out = href.trim();
  const decoded = safeDecodeUri(out);
  const stop = decoded.search(STOP_RE);
  if (stop > 0) {
    return {
      href: decoded.slice(0, stop).replace(TRAILING_PUNCT_RE, ""),
      trailingText: decoded.slice(stop),
    };
  }
  const clean = out.replace(TRAILING_PUNCT_RE, "");
  return {
    href: clean,
    trailingText: out.slice(clean.length),
  };
}

export function cleanMarkdownHref(href?: string): string | undefined {
  return splitMarkdownHref(href).href;
}

/** Defense-in-depth scheme whitelist. Only http/https/mailto (plus relative
 *  paths and `#` anchors, which carry no scheme) are clickable. Anything with
 *  another scheme — javascript:, data:, file:, vbscript:, … — is rejected so it
 *  renders as inert text instead of an exploitable link. rehype-sanitize already
 *  blocks these by default; this is a second, explicit gate. */
const ALLOWED_SCHEMES = new Set(["http", "https", "mailto"]);
const SCHEME_RE = /^([a-z][a-z0-9+.-]*):/i;

function isSafeHref(href?: string): href is string {
  if (!href) return false;
  // Leading control chars/whitespace can disguise a scheme (e.g. "java\tscript:").
  const trimmed = href.replace(/[\u0000-\u0020]+/g, "");
  const match = SCHEME_RE.exec(trimmed);
  // No scheme → relative path or `#` anchor → safe.
  if (!match) return true;
  return ALLOWED_SCHEMES.has(match[1].toLowerCase());
}

function textChild(children: ReactNode): string | null {
  if (typeof children === "string") return children;
  if (Array.isArray(children) && children.every((c) => typeof c === "string")) {
    return children.join("");
  }
  return null;
}

export function MarkdownLink({
  href,
  children,
  ...props
}: ComponentPropsWithoutRef<"a">) {
  const { href: cleanHref, trailingText } = splitMarkdownHref(href);
  const safe = isSafeHref(cleanHref);
  const text = textChild(children);
  if (trailingText && text) {
    const decodedText = safeDecodeUri(text);
    const linkedText = decodedText.endsWith(trailingText)
      ? decodedText.slice(0, -trailingText.length)
      : decodedText;
    const label = linkedText || cleanHref;
    return (
      <Fragment>
        {safe ? (
          <a {...props} href={cleanHref} target="_blank" rel="noopener noreferrer">
            {label}
          </a>
        ) : (
          <span {...props}>{label}</span>
        )}
        {trailingText}
      </Fragment>
    );
  }
  if (!safe) {
    return <span {...props}>{children}</span>;
  }
  return (
    <a {...props} href={cleanHref} target="_blank" rel="noopener noreferrer">
      {children}
    </a>
  );
}

export function MarkdownInput(props: ComponentPropsWithoutRef<"input">) {
  const name =
    props.name ?? (props.type === "checkbox" ? "markdown-task-checkbox" : "markdown-input");
  const ariaLabel =
    props["aria-label"] ?? (props.type === "checkbox" ? "Markdown task" : undefined);
  const title = props.title ?? (props.type === "checkbox" ? "Markdown task" : undefined);
  return <input {...props} name={name} aria-label={ariaLabel} title={title} />;
}
