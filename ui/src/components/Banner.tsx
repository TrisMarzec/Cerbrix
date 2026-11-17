import type { ComponentChildren } from 'preact';

interface Props {
  kind: 'warning' | 'info';
  children: ComponentChildren;
}

export function Banner({ kind, children }: Props) {
  return (
    <div class={`banner banner-${kind}`} role={kind === 'warning' ? 'alert' : 'status'}>
      {children}
    </div>
  );
}
