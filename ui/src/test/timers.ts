import { act } from '@testing-library/preact';
import { vi } from 'vitest';

/** Moves fake timers forward and lets everything that became ready (promises, renders) finish. */
export async function advance(ms: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}
