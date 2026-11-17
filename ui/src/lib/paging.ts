import { useCallback, useEffect, useRef } from 'preact/hooks';

import { useQueryState } from './query';

/** An offset from the URL: a whole number, or 0 for anything else (garbage, negative, too large). */
export function parseOffset(text: string | null): number {
  if (text === null || !/^\d+$/.test(text)) return 0;
  const n = Number(text);
  return Number.isSafeInteger(n) ? n : 0;
}

/** Start of the last page that exists for `total` > 0 entries. */
export function lastPageStart(total: number, limit: number): number {
  return Math.floor((total - 1) / limit) * limit;
}

function offsetParam(offset: number): string | undefined {
  return offset === 0 ? undefined : String(offset);
}

interface Correction {
  /** The offset is at or past the total of the answer: the page is being replaced by the last one. */
  beyond: boolean;
  /** The offset the rows on screen belong to: the previous one while the next page is pending. */
  shownOffset: number;
}

interface Paging {
  /** The offset of the URL. */
  offset: number;
  /** Goes to another offset (the first page leaves the parameter out). */
  set: (offset: number) => void;
  /**
   * Called on every render with the total of the last answer and `usePolled`'s `pending`: when the
   * offset is beyond the total, the URL is replaced with the last page that exists once the answer
   * for this offset is in (so the next answer is in range).
   */
  correct: (total: number | undefined, pending: boolean) => Correction;
}

/** The offset of a paged list in the URL parameter `param`. */
export function usePaging(param: string, limit: number): Paging {
  const [query, setQuery] = useQueryState();
  const offset = parseOffset(query.get(param));
  const wanted = useRef<number | undefined>(undefined);
  const shown = useRef(offset);
  wanted.current = undefined;

  // Runs after every render: `correct` of this render has said whether a correction is due.
  useEffect(() => {
    if (wanted.current !== undefined) setQuery({ [param]: offsetParam(wanted.current) });
  });

  const set = useCallback(
    (next: number) => {
      setQuery({ [param]: offsetParam(next) });
    },
    [param, setQuery],
  );

  const correct = (total: number | undefined, pending: boolean): Correction => {
    if (!pending) shown.current = offset;
    const beyond = total !== undefined && total > 0 && offset >= total;
    if (beyond && !pending) wanted.current = lastPageStart(total, limit);
    return { beyond, shownOffset: shown.current };
  };

  return { offset, set, correct };
}
