import { render } from '@testing-library/preact';
import { vi } from 'vitest';

import type { ValuePanelData } from '../../api/types';
import { NOW, valuePanelFixture } from '../../test/fixtures';
import { installMeasure, releaseMeasure, type Measure } from '../test/measure';
import { ValuePanel } from './ValuePanel';

/** The boxes of the hidden samples, one per size step: the value with the age under it, or beside it. */
const STEPS: Record<string, { width: number; height: number }> = {
  'is-l': { width: 300, height: 175 },
  'is-m': { width: 230, height: 140 },
  'is-s': { width: 160, height: 105 },
  'is-text': { width: 110, height: 78 },
  'is-inline': { width: 220, height: 46 },
};
const NO_DATA = 46;

let measure: Measure;

function body(width: number, height: number) {
  measure = installMeasure({ bodyWidth: width, bodyHeight: height });
}

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains('wall-sample')) {
      if (this.classList.contains('wall-nodata')) return new DOMRect(0, 0, 200, NO_DATA);
      const step = Object.keys(STEPS).find((c) => this.classList.contains(c));
      const size = step === undefined ? undefined : STEPS[step];
      if (this.classList.contains('wall-value') && size !== undefined) {
        return new DOMRect(0, 0, size.width, size.height);
      }
    }
    return Element.prototype.getBoundingClientRect.call(this);
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  releaseMeasure();
});

function show(overrides: Partial<ValuePanelData> = {}, now = NOW) {
  return render(<ValuePanel panel={valuePanelFixture(overrides)} now={now} />);
}

/** The shown value block, without the hidden samples. */
function shown(container: Element): Element | null {
  return container.querySelector('.wall-value:not(.wall-sample)');
}

function step(container: Element): string | undefined {
  const el = shown(container);
  return el === null ? undefined : Object.keys(STEPS).find((c) => el.classList.contains(c));
}

function ago(minutes: number): string {
  return new Date(NOW.getTime() - minutes * 60_000).toISOString();
}

describe('ValuePanel', () => {
  it('is labelled with the host, the item and the parameters in key order', () => {
    body(600, 300);
    const { container } = show({ params: { mount: '/', fs: 'ext4' } });
    expect(container.querySelector('.wall-panel-label')).toHaveTextContent('db-01 · load.1m · fs=ext4 · mount=/');
  });

  it('shows the value through the item format and its age below it', () => {
    body(600, 300);
    const { container } = show({ item: 'disk.used_percent', value: 41.25 });
    expect(shown(container)?.querySelector('.wall-value-number')).toHaveTextContent('41.3 %');
    const age = shown(container)?.querySelector('.wall-value-age');
    expect(age).toHaveTextContent('30 s ago');
    expect(age).not.toHaveClass('is-old');
  });

  it('marks a value older than ten minutes as old, in words', () => {
    body(600, 300);
    const { container } = show({ at: ago(12) });
    const age = shown(container)?.querySelector('.wall-value-age');
    expect(age).toHaveTextContent('12 min ago · old');
    expect(age).toHaveClass('is-old');
  });

  it('does not mark a value of exactly ten minutes as old', () => {
    body(600, 300);
    const { container } = show({ at: ago(10) });
    expect(shown(container)?.querySelector('.wall-value-age')).toHaveTextContent(/^10 min ago$/);
  });

  it('says "No data" without an age for a null value', () => {
    body(600, 300);
    const { container } = show({ value: null, at: null });
    expect(container.querySelector('.wall-nodata:not(.wall-sample)')).toHaveTextContent('No data');
    expect(shown(container)).toBeNull();
    const ages = [...container.querySelectorAll('.wall-value-age')].filter((el) => el.closest('.wall-sample') === null);
    expect(ages).toHaveLength(0);
  });

  it('leaves the age out for a value without a time', () => {
    body(600, 300);
    const { container } = show({ at: null });
    expect(shown(container)?.querySelector('.wall-value-number')).toHaveTextContent('0.42');
    expect(shown(container)?.querySelector('.wall-value-age')).toBeNull();
  });

  it.each([
    ['a 3-row panel', 300, 'is-l'],
    ['a 2-row panel', 150, 'is-m'],
    ['a low 2-row panel', 110, 'is-s'],
    ['a very low panel', 80, 'is-text'],
    ['a 1-row panel', 50, 'is-inline'],
  ])('takes the largest size that fits the height of %s', (_name, height, expected) => {
    body(600, height);
    const { container } = show();
    expect(step(container)).toBe(expected);
  });

  it('takes a smaller size when the value is wider than the panel', () => {
    body(200, 300);
    const { container } = show();
    expect(step(container)).toBe('is-s');
  });

  it('writes "…" for a value too wide at every size, with the whole value for a screen reader', () => {
    body(100, 300);
    const { container, getByRole } = show({ item: 'disk.used', value: 1_294_967_296 });
    expect(step(container)).toBe('is-inline');
    expect(shown(container)?.querySelector('.wall-value-number')).toHaveTextContent(/^…$/);
    expect(getByRole('img', { name: 'db-01 · disk.used: 1.2 GiB, 30 s ago' })).toBeInTheDocument();
    // Cut, the block may still be wider than the body: the age ends in an ellipsis on its one line.
    expect(shown(container)?.querySelector('.wall-value-age')).toHaveClass('wall-ellipsis');
  });

  it('names a cut old value as old for a screen reader', () => {
    body(100, 300);
    const { getByRole } = show({ at: ago(12) });
    expect(getByRole('img', { name: 'db-01 · load.1m: 0.42, 12 min ago, old' })).toBeInTheDocument();
  });

  it('gives an uncut value no image role', () => {
    body(600, 300);
    const { container } = show();
    expect(shown(container)).not.toHaveAttribute('role');
    expect(shown(container)?.querySelector('.wall-value-age')).not.toHaveClass('wall-ellipsis');
  });

  it('shows a value measured in the future as 0 s ago, not old', () => {
    body(600, 300);
    const { container } = show({ at: new Date(NOW.getTime() + 90_000).toISOString() });
    const age = shown(container)?.querySelector('.wall-value-age');
    expect(age).toHaveTextContent(/^0 s ago$/);
    expect(age).not.toHaveClass('is-old');
  });

  it('shows only the label when not even one line fits', () => {
    body(600, 40);
    const { container } = show();
    expect(shown(container)).toBeNull();
    expect(container.querySelector('.wall-panel-label')).toHaveTextContent('db-01 · load.1m');
  });

  it('says that the metrics are not available instead of "No data" when the store is down', () => {
    body(600, 300);
    const { container } = render(
      <ValuePanel panel={valuePanelFixture({ value: null, at: null })} now={NOW} metricsAvailable={false} />,
    );
    expect(container.querySelector('.wall-nodata:not(.wall-sample)')).toHaveTextContent(
      /^Metrics are not available right now$/,
    );
    expect(shown(container)).toBeNull();
  });

  it('shows only the label when "No data" does not fit', () => {
    body(600, 40);
    const { container } = show({ value: null, at: null });
    expect(container.querySelector('.wall-nodata:not(.wall-sample)')).toBeNull();
  });

  it('measures again when a value arrives in a panel that had none', () => {
    body(600, 300);
    const view = show({ value: null, at: null });
    expect(shown(view.container)).toBeNull();
    view.rerender(<ValuePanel panel={valuePanelFixture()} now={NOW} />);
    expect(step(view.container)).toBe('is-l');
  });

  it('changes the size when the panel is resized', () => {
    body(600, 50);
    const { container } = show();
    expect(step(container)).toBe('is-inline');
    measure.set({ bodyHeight: 300 });
    measure.resize();
    expect(step(container)).toBe('is-l');
  });
});
