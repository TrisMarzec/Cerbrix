import { lastPageStart } from '../lib/paging';

interface Props {
  total: number;
  limit: number;
  offset: number;
  /**
   * True while the next page loads: the buttons ignore activation. They stay focusable
   * (`aria-disabled`, not `disabled`), so keyboard focus is not dropped to the page body.
   */
  disabled?: boolean;
  onChange: (offset: number) => void;
}

export function Pager({ total, limit, offset, disabled = false, onChange }: Props) {
  if (total === 0) return null;
  const beyond = offset >= total;
  const lastStart = lastPageStart(total, limit);
  const from = beyond ? lastStart : offset;
  const last = Math.min(from + limit, total);
  const atStart = from <= 0;
  const atEnd = from + limit >= total;
  return (
    <nav class="pager" aria-label="Pages">
      <button
        type="button"
        class="btn"
        disabled={atStart}
        aria-disabled={disabled && !atStart ? 'true' : undefined}
        onClick={() => {
          if (!disabled) onChange(beyond ? lastStart : Math.max(0, offset - limit));
        }}
      >
        Previous
      </button>
      <span class="num">{`${from + 1}–${last} of ${total}`}</span>
      <button
        type="button"
        class="btn"
        disabled={atEnd}
        aria-disabled={disabled && !atEnd ? 'true' : undefined}
        onClick={() => {
          if (!disabled) onChange(from + limit);
        }}
      >
        Next
      </button>
    </nav>
  );
}
