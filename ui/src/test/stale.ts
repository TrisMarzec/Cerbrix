import { screen } from '@testing-library/preact';

/** The "Not updated since" notes on screen. */
export function staleNotes(): HTMLElement[] {
  return screen.queryAllByText(/^Not updated since/);
}

/**
 * Asserts that a "Not updated since <time>" note stands directly before `region` (the note is the
 * nearest preceding one, and nothing of the region comes before it).
 */
export function expectStaleAbove(region: HTMLElement, time: string): void {
  const before = staleNotes().filter(
    (note) => (note.compareDocumentPosition(region) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0,
  );
  const note = before.at(-1);
  expect(note, 'no stale note above the region').toBeDefined();
  expect(note).toHaveTextContent(`Not updated since ${time}`);
}
