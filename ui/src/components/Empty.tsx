import type { ComponentChildren } from 'preact';

export function Empty({ children }: { children: ComponentChildren }) {
  return <p class="empty">{children}</p>;
}
