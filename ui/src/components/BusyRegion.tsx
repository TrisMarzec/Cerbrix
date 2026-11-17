import type { ComponentChildren } from 'preact';

interface Props {
  /** True while the answer for a new query is outstanding: the content is the previous answer. */
  pending: boolean;
  /** A class of the page for its own layout. */
  class?: string;
  children: ComponentChildren;
}

/** Everything that comes from the last answer: dimmed and marked busy while the next is outstanding. */
export function BusyRegion({ pending, class: className, children }: Props) {
  const classes = ['busy-region', pending ? 'is-busy' : '', className ?? ''].filter((c) => c !== '');
  return (
    <div class={classes.join(' ')} aria-busy={pending}>
      {children}
    </div>
  );
}
