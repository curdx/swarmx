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
  const text = textChild(children);
  if (trailingText && text) {
    const decodedText = safeDecodeUri(text);
    const linkedText = decodedText.endsWith(trailingText)
      ? decodedText.slice(0, -trailingText.length)
      : decodedText;
    return (
      <Fragment>
        <a {...props} href={cleanHref} target="_blank" rel="noopener noreferrer">
          {linkedText || cleanHref}
        </a>
        {trailingText}
      </Fragment>
    );
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
