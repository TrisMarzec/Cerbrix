interface Props {
  total: number;
  limit: number;
  offset: number;
  onChange: (offset: number) => void;
}

export function Pager({ total, limit, offset, onChange }: Props) {
  if (total === 0) return null;
  const beyond = offset >= total;
  const lastStart = Math.floor((total - 1) / limit) * limit;
  const from = beyond ? lastStart : offset;
  const last = Math.min(from + limit, total);
  return (
    <nav class="pager" aria-label="Pages">
      <button
        type="button"
        class="btn"
        disabled={from <= 0}
        onClick={() => onChange(beyond ? lastStart : Math.max(0, offset - limit))}
      >
        Previous
      </button>
      <span class="num">{`${from + 1}–${last} of ${total}`}</span>
      <button
        type="button"
        class="btn"
        disabled={from + limit >= total}
        onClick={() => onChange(from + limit)}
      >
        Next
      </button>
    </nav>
  );
}
