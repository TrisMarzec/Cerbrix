import { render, type RenderResult } from '@testing-library/preact';
import type { ComponentChildren } from 'preact';

import { NOW } from './fixtures';

/** Renders a component that takes `now`; the clock is fixed so output never depends on the real time. */
export function renderAt(build: (now: Date) => ComponentChildren): RenderResult {
  return render(<>{build(NOW)}</>);
}
