import { render } from '@testing-library/preact';
import { vi } from 'vitest';

import type { Problem, ProblemsPanelData } from '../../api/types';
import { NOW, acknowledgedProblem, criticalProblem, problemsPanelFixture } from '../../test/fixtures';
import { installMeasure, releaseMeasure, type Sizes } from '../test/measure';
import { ProblemsPanel } from './ProblemsPanel';

const ROW = 100;
const LINE = 60;

/** A wide body with room for `n` rows of the sample height and half a row more. */
function room(n: number, extra: Partial<Sizes> = {}): Partial<Sizes> {
  return {
    bodyWidth: 1000,
    bodyHeight: n * ROW + ROW / 2,
    rowHeight: ROW,
    rowWidth: 400,
    summaryHeight: LINE,
    emptyWidth: 500,
    emptyHeight: LINE,
    ...extra,
  };
}

function problems(n: number, base: Problem = criticalProblem): Problem[] {
  return Array.from({ length: n }, (_, i) => ({ ...base, id: `p-${i}`, host: `host-${i}` }));
}

function label(container: Element): Element | null {
  return container.querySelector('.wall-panel-label');
}

function empty(container: Element): Element | null {
  return container.querySelector('.wall-problems-empty:not(.wall-sample)');
}

function summary(container: Element): Element | null {
  return container.querySelector('.wall-problems-more:not(.wall-sample)');
}

function show(panel: Partial<ProblemsPanelData>, causes: string[] = []) {
  return render(<ProblemsPanel panel={problemsPanelFixture(panel)} now={NOW} causes={causes} />);
}

/** The shown rows, without the hidden sample. */
function rows(container: Element): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>('.wall-problem:not(.wall-sample)')];
}

function part(row: HTMLElement, cls: string): string {
  return row.querySelector(`.${cls}`)?.textContent ?? '';
}

afterEach(() => {
  releaseMeasure();
});

describe('ProblemsPanel', () => {
  it('is labelled "Problems"', () => {
    installMeasure(room(5));
    const { container } = show({});
    expect(container.querySelector('.wall-panel-label')).toHaveTextContent('Problems');
  });

  it('shows one row per problem in the server order with host, text, severity word and duration', () => {
    installMeasure(room(5));
    const { container } = show({ problems: [criticalProblem, acknowledgedProblem] });
    const [first, second] = rows(container);
    if (first === undefined || second === undefined) throw new Error('two rows expected');
    expect(rows(container)).toHaveLength(2);

    expect(part(first, 'wall-problem-host')).toBe('db-01');
    expect(part(first, 'wall-problem-text')).toBe('Disk almost full mount=/ — disk.used_percent > 90: value 94.2');
    // The instance appears once, in the first line.
    expect(part(first, 'wall-problem-meta')).toBe('Critical');
    expect(part(first, 'wall-problem-duration')).toBe('2 h 14 min');
    expect(first).toHaveClass('sev-critical');
    expect(first).not.toHaveClass('is-acknowledged');

    expect(part(second, 'wall-problem-host')).toBe('web-02');
    expect(part(second, 'wall-problem-text')).toBe('Memory high — memory.used_percent >= 80: value 85.1');
    expect(part(second, 'wall-problem-meta')).toBe('Warning · acknowledged');
    expect(part(second, 'wall-problem-duration')).toBe('1 d 5 h');
    expect(second).toHaveClass('sev-warning', 'is-acknowledged');
  });

  it('says "acknowledged" without the name or comment, even when the data has them', () => {
    installMeasure(room(5));
    const { container } = show({ problems: [acknowledgedProblem] });
    const row = rows(container)[0];
    expect(row).toHaveTextContent('acknowledged');
    expect(row?.textContent).not.toContain('anna');
    expect(row?.textContent).not.toContain('watching');
  });

  it('leaves out a summary that only repeats the trigger', () => {
    installMeasure(room(5));
    const { container } = show({ problems: [{ ...criticalProblem, instance: '', summary: 'Disk almost full' }] });
    expect(part(rows(container)[0] ?? document.body, 'wall-problem-text')).toBe('Disk almost full');
  });

  it('highlights exactly the rows whose ids are in causes', () => {
    installMeasure(room(5));
    const list = problems(3);
    const { container } = show({ problems: list }, ['p-1', 'not-shown']);
    expect(rows(container).map((r) => r.classList.contains('is-cause'))).toEqual([false, true, false]);
  });

  it('replaces the rows that do not fit with "and N more"', () => {
    installMeasure(room(4));
    const { container } = show({ problems: problems(10) });
    expect(rows(container).map((r) => part(r, 'wall-problem-host'))).toEqual(['host-0', 'host-1', 'host-2']);
    expect(summary(container)).toHaveTextContent('and 7 more');
  });

  it('counts the problems the server left out of the list', () => {
    installMeasure(room(4));
    const { container } = show({ problems: problems(10), more: 5 });
    expect(rows(container)).toHaveLength(3);
    expect(summary(container)).toHaveTextContent('and 12 more');
  });

  it('gives "and N more" a row of its own when every listed problem fits but the server left some out', () => {
    installMeasure(room(10));
    const { container } = show({ problems: problems(2), more: 3 });
    expect(rows(container)).toHaveLength(2);
    expect(summary(container)).toHaveTextContent('and 3 more');
  });

  it('shows every problem and no summary when all fit exactly', () => {
    installMeasure(room(4));
    const { container } = show({ problems: problems(4) });
    expect(rows(container)).toHaveLength(4);
    expect(summary(container)).toBeNull();
  });

  it('keeps "and N more" under shown rows dim, without a severity colour', () => {
    installMeasure(room(4));
    const { container } = show({ problems: problems(10) });
    expect(summary(container)?.className).not.toMatch(/sev-/);
  });

  it('shows the count with the severity of the most urgent problem when only the summary line fits', () => {
    installMeasure(room(0, { bodyHeight: ROW - 1 }));
    const { container } = show({ problems: problems(10), more: 2 });
    expect(rows(container)).toHaveLength(0);
    expect(summary(container)).toHaveTextContent('12 open problems · Critical');
    expect(summary(container)).toHaveClass('is-alone', 'sev-critical');
    expect(summary(container)).not.toHaveClass('is-acknowledged');
    expect(label(container)).toHaveTextContent(/^Problems$/);
  });

  it('says "1 open problem · Critical" for a single problem that does not fit as a row', () => {
    installMeasure(room(0, { bodyHeight: LINE }));
    const { container } = show({ problems: problems(1) });
    expect(summary(container)).toHaveTextContent('1 open problem · Critical');
  });

  it('shows an acknowledged-only summary at half strength with the severity word', () => {
    installMeasure(room(0, { bodyHeight: ROW - 1 }));
    const { container } = show({ problems: problems(2, acknowledgedProblem) });
    expect(summary(container)).toHaveTextContent('2 open problems · Warning');
    expect(summary(container)).toHaveClass('sev-warning', 'is-acknowledged');
  });

  it('puts the count and severity into the label when not even the summary line fits', () => {
    installMeasure(room(0, { bodyHeight: LINE - 1 }));
    const { container } = show({ problems: problems(10), more: 2 });
    expect(rows(container)).toHaveLength(0);
    expect(summary(container)).toBeNull();
    expect(label(container)).toHaveTextContent('Problems · 12 open · Critical');
    expect(label(container)).toHaveClass('is-count', 'sev-critical');
  });

  it('puts an acknowledged-only count into the label at half strength with the severity word', () => {
    installMeasure(room(0, { bodyHeight: LINE - 1 }));
    const { container } = show({ problems: problems(3, acknowledgedProblem) });
    expect(label(container)).toHaveTextContent('Problems · 3 open · Warning');
    expect(label(container)).toHaveClass('is-count', 'sev-warning', 'is-acknowledged');
  });

  it('keeps the plain label without problems when nothing fits', () => {
    installMeasure(room(0, { bodyHeight: LINE - 1 }));
    const { container } = show({ problems: [], more: 0 });
    expect(label(container)).toHaveTextContent(/^Problems$/);
    expect(label(container)).not.toHaveClass('is-count');
    expect(empty(container)).toBeNull();
  });

  it('says "No open problems" on one line in the healthy colour without problems', () => {
    installMeasure(room(5));
    const { container } = show({ problems: [], more: 0 });
    const shown = empty(container);
    expect(shown).toHaveTextContent('No open problems');
    expect(shown).toHaveClass('wall-problems-empty', 'wall-ellipsis');
    expect(shown).not.toHaveClass('is-small');
  });

  it('uses the smaller size for "No open problems" when the large one is wider than the body', () => {
    installMeasure(room(5, { bodyWidth: 499 }));
    const { container } = show({ problems: [], more: 0 });
    expect(empty(container)).toHaveClass('is-small');
  });

  it('shows the duration when host and duration fit side by side', () => {
    installMeasure(room(5));
    const { container } = show({ problems: [criticalProblem] });
    expect(container.querySelector('.wall-problems')).not.toHaveClass('is-narrow');
    expect(part(rows(container)[0] ?? document.body, 'wall-problem-duration')).toBe('2 h 14 min');
  });

  it('leaves out the duration in a narrow panel and keeps the host and the severity word', () => {
    installMeasure(room(5, { bodyWidth: 399 }));
    const { container } = show({ problems: [criticalProblem] });
    const row = rows(container)[0] ?? document.body;
    expect(container.querySelector('.wall-problems')).toHaveClass('is-narrow');
    expect(row.querySelector('.wall-problem-duration')).toBeNull();
    expect(part(row, 'wall-problem-host')).toBe('db-01');
    expect(part(row, 'wall-problem-meta')).toBe('Critical');
  });

  it('cuts a long host name with an ellipsis and keeps the full name in a title', () => {
    installMeasure(room(5));
    const host = 'h'.repeat(200);
    const { container } = show({ problems: [{ ...criticalProblem, host }] });
    const el = rows(container)[0]?.querySelector('.wall-problem-host');
    expect(el).toHaveClass('wall-ellipsis');
    expect(el).toHaveAttribute('title', host);
    expect(rows(container)[0]?.querySelector('.wall-problem-text')).toHaveAttribute(
      'title',
      'Disk almost full mount=/ — disk.used_percent > 90: value 94.2',
    );
  });

  it('shows a hostile summary as text', () => {
    installMeasure(room(5));
    const summary = '<img src=x onerror="alert(1)">';
    const { container } = show({ problems: [{ ...criticalProblem, summary }] });
    expect(container.querySelector('img')).toBeNull();
    expect(part(rows(container)[0] ?? document.body, 'wall-problem-text')).toContain(summary);
  });

  it('measures again when the body changes size', () => {
    const measure = installMeasure(room(2));
    const { container } = show({ problems: problems(10) });
    expect(rows(container)).toHaveLength(1);
    measure.set(room(6));
    measure.resize();
    expect(rows(container)).toHaveLength(5);
    expect(summary(container)).toHaveTextContent('and 5 more');
  });

  it('observes its body and disconnects the observer on unmount', () => {
    const measure = installMeasure(room(5));
    const { container, unmount } = show({});
    const [observer] = measure.observers();
    expect(measure.observers()).toHaveLength(1);
    expect([...(observer?.targets ?? [])]).toEqual([container.querySelector('.wall-panel-body')]);
    unmount();
    expect(observer?.disconnected).toBe(true);
  });

  it('measures on window resize without ResizeObserver and stops listening on unmount', () => {
    const measure = installMeasure(room(2), { resizeObserver: false });
    const add = vi.spyOn(window, 'addEventListener');
    const remove = vi.spyOn(window, 'removeEventListener');
    const { container, unmount } = show({ problems: problems(10) });
    expect(rows(container)).toHaveLength(1);
    measure.set(room(4));
    measure.resize();
    expect(rows(container)).toHaveLength(3);
    const added = add.mock.calls.filter(([type]) => type === 'resize').map(([, listener]) => listener);
    expect(added).toHaveLength(1);
    unmount();
    expect(remove).toHaveBeenCalledWith('resize', added[0]);
    add.mockRestore();
    remove.mockRestore();
  });

  it('has nothing focusable', () => {
    installMeasure(room(5));
    const { container } = show({ problems: problems(3) });
    expect(container.querySelectorAll('a, button, input, [tabindex]')).toHaveLength(0);
  });
});
