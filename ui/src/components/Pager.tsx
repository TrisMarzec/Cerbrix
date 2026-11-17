interface Props {
  total: number;
  limit: number;
  offset: number;
  onChange: (offset: number) => void;
}

export function Pager({ total, limit, offset, onChange }: Props) {
  if (total === 0) return null;
  const last = Math.min(offset + limit, total);
  return (
    <nav class="pager" aria-label="Pages">
      <button
        type="button"
        class="btn"
        disabled={offset <= 0}
        onClick={() => onChange(Math.max(0, offset - limit))}
      >
        Previous
      </button>
      <span class="num">{`${offset + 1}–${last} of ${total}`}</span>
      <button
        type="button"
        class="btn"
        disabled={offset + limit >= total}
        onClick={() => onChange(offset + limit)}
      >
        Next
      </button>
    </nav>
  );
}
