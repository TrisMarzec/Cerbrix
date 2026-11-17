import type { ComponentChildren } from 'preact';
import { useEffect, useId, useRef } from 'preact/hooks';

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

interface Props {
  title: string;
  onClose: () => void;
  /** A click on the backdrop closes the dialog unless this is false. */
  closeOnBackdrop?: boolean;
  children: ComponentChildren;
}

/** Open dialogs, innermost last. Only the innermost one handles keys and keeps focus. */
const stack: object[] = [];

function focusables(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE));
}

function restoreFocus(before: Element | null) {
  if (before instanceof HTMLElement && before.isConnected) {
    before.focus();
    return;
  }
  const main = document.querySelector<HTMLElement>('main');
  if (main !== null) {
    if (!main.hasAttribute('tabindex')) main.setAttribute('tabindex', '-1');
    main.focus();
  }
}

export function Dialog({ title, onClose, closeOnBackdrop = true, children }: Props) {
  const titleId = useId();
  const dialog = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;

  useEffect(() => {
    const token = {};
    const before = document.activeElement;
    const el = dialog.current;
    if (el === null) return undefined;
    stack.push(token);
    (focusables(el)[0] ?? el).focus();

    const isTop = () => stack[stack.length - 1] === token;

    // Bring focus back when the focused element vanished or focus left the dialog.
    const keepFocus = () => {
      if (!isTop() || el.contains(document.activeElement)) return false;
      (focusables(el)[0] ?? el).focus();
      return true;
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (!isTop()) return;
      if (e.key === 'Escape') {
        e.preventDefault();
        close.current();
        return;
      }
      if (e.key !== 'Tab') return;
      if (keepFocus()) {
        e.preventDefault();
        return;
      }
      const items = focusables(el);
      const first = items[0];
      const last = items[items.length - 1];
      if (first === undefined || last === undefined) {
        e.preventDefault();
        return;
      }
      const active = document.activeElement;
      if (e.shiftKey && (active === first || active === el)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    };

    const observer = new MutationObserver(() => {
      keepFocus();
    });
    observer.observe(el, { childList: true, subtree: true });
    document.addEventListener('keydown', onKeyDown);

    return () => {
      document.removeEventListener('keydown', onKeyDown);
      observer.disconnect();
      stack.splice(stack.indexOf(token), 1);
      restoreFocus(before);
    };
  }, []);

  return (
    <div
      class="dialog-backdrop"
      onMouseDown={(e) => {
        if (closeOnBackdrop && e.target === e.currentTarget) onClose();
      }}
    >
      <div
        class="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        ref={dialog}
      >
        <h2 id={titleId}>{title}</h2>
        {children}
      </div>
    </div>
  );
}
