import type { ComponentChildren } from 'preact';
import { useEffect, useMemo, useState } from 'preact/hooks';

const NAVIGATED = 'cerbrix:navigate';

function currentHref(): string {
  return window.location.pathname + window.location.search;
}

/** The path and query of the address bar; follows `navigate` and the back and forward buttons. */
export function useRoute(): { path: string; query: URLSearchParams } {
  const [href, setHref] = useState(currentHref);
  useEffect(() => {
    const update = () => setHref(currentHref());
    window.addEventListener('popstate', update);
    window.addEventListener(NAVIGATED, update);
    update();
    return () => {
      window.removeEventListener('popstate', update);
      window.removeEventListener(NAVIGATED, update);
    };
  }, []);
  return useMemo(() => {
    const at = href.indexOf('?');
    return {
      path: at < 0 ? href : href.slice(0, at),
      query: new URLSearchParams(at < 0 ? '' : href.slice(at)),
    };
  }, [href]);
}

/** Goes to a path (with an optional query) of this app; the same URL as now does nothing. */
export function navigate(to: string, opts: { replace?: boolean } = {}): void {
  const target = new URL(to, window.location.href);
  const next = target.pathname + target.search + target.hash;
  if (next === window.location.pathname + window.location.search + window.location.hash) return;
  if (opts.replace === true) {
    window.history.replaceState(null, '', next);
  } else {
    window.history.pushState(null, '', next);
  }
  window.dispatchEvent(new Event(NAVIGATED));
}

interface LinkProps {
  href: string;
  class?: string;
  'aria-current'?: 'page';
  children: ComponentChildren;
}

/** A control character: a browser drops a tab or newline from a URL, so `/\t/host` becomes `//host`. */
const CONTROL = /[\u0000-\u001f\u007f]/;

/** A path of this app: starts with one slash, not `//host` or `/\host`, and has no control character. */
function isInternal(href: string): boolean {
  return href.startsWith('/') && href[1] !== '/' && href[1] !== '\\' && !CONTROL.test(href);
}

/** An in-app link: a real `<a href>`, so a new tab works; a plain left click does not reload the page. */
export function Link({ href, class: className, children, ...rest }: LinkProps) {
  // Only a path of this app becomes a link; anything else (`javascript:`, `//host`) is plain text.
  if (!isInternal(href)) return <span class={className}>{children}</span>;
  function onClick(e: MouseEvent) {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    navigate(href);
  }
  return (
    <a href={href} class={className} onClick={onClick} {...rest}>
      {children}
    </a>
  );
}

function segments(path: string): string[] {
  const parts = path.split('/').filter((part) => part !== '');
  return parts;
}

/** Matches `/hosts/:name` against a path; the parameters come back decoded, a malformed escape gives null. */
export function match(pattern: string, path: string): Record<string, string> | null {
  const want = segments(pattern);
  const have = segments(path);
  if (want.length !== have.length) return null;
  const params: Record<string, string> = {};
  for (let i = 0; i < want.length; i += 1) {
    const expected = want[i];
    const raw = have[i];
    if (expected === undefined || raw === undefined) return null;
    let decoded: string;
    try {
      decoded = decodeURIComponent(raw);
    } catch {
      return null;
    }
    if (expected.startsWith(':')) {
      params[expected.slice(1)] = decoded;
    } else if (expected !== decoded) {
      return null;
    }
  }
  return params;
}
