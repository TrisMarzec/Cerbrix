import { render, screen } from '@testing-library/preact';
import { vi } from 'vitest';

import type { Problem, ProblemsPanelData } from '../../api/types';
import { NOW, acknowledgedProblem, criticalProblem, problemsPanelFixture } from '../../test/fixtures';
import { installMeasure, releaseMeasure, type Sizes } from '../test/measure';
import { ProblemsPanel } from './ProblemsPanel';

const ROW = 100;

/** Room for `n` rows of the sample height. */
function room(n: number): Partial<Sizes> {
  return { bodyWidth: 1000, bodyHeight: n * ROW + ROW / 2, rowHeight: ROW };
}

function problems(n: number): Problem[] {
  return Array.from({ length: n }, (_, i) => ({ ...criticalProblem, id: `p-${i}`, host: `host-${i}` }));
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
    expect(part(first, 'wall-problem-meta')).toBe('Critical · mount=/');
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
    expect(container.querySelector('.wall-problems-more')).toHaveTextContent('and 7 more');
  });

  it('counts the problems the server left out of the list', () => {
    installMeasure(room(4));
    const { container } = show({ problems: problems(10), more: 5 });
    expect(rows(container)).toHaveLength(3);
    expect(container.querySelector('.wall-problems-more')).toHaveTextContent('and 12 more');
  });

  it('gives "and N more" a row of its own when every listed problem fits but the server left some out', () => {
    installMeasure(room(10));
    const { container } = show({ problems: problems(2), more: 3 });
    expect(rows(container)).toHaveLength(2);
    expect(container.querySelector('.wall-problems-more')).toHaveTextContent('and 3 more');
  });

  it('shows every problem and no summary when all fit exactly', () => {
    installMeasure(room(4));
    const { container } = show({ problems: problems(4) });
    expect(rows(container)).toHaveLength(4);
    expect(container.querySelector('.wall-problems-more')).toBeNull();
  });

  it('shows only the number of problems when there is room for one line', () => {
    installMeasure(room(1));
    const { container } = show({ problems: problems(10), more: 2 });
    expect(rows(container)).toHaveLength(0);
    expect(container.querySelector('.wall-problems-more')).toHaveTextContent('12 open problems');
  });

  it('shows only its label when not even one row fits', () => {
    installMeasure({ bodyWidth: 1000, bodyHeight: ROW - 1, rowHeight: ROW });
    const { container } = show({ problems: problems(10) });
    expect(rows(container)).toHaveLength(0);
    expect(container.querySelector('.wall-problems-more')).toBeNull();
    expect(screen.queryByText('No open problems')).toBeNull();
    expect(container.querySelector('.wall-panel-label')).toHaveTextContent('Problems');
  });

  it('says "No open problems" in the healthy colour without problems', () => {
    installMeasure(room(5));
    show({ problems: [], more: 0 });
    expect(screen.getByText('No open problems')).toHaveClass('wall-problems-empty');
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
    expect(container.querySelector('.wall-problems-more')).toHaveTextContent('and 5 more');
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
    const remove = vi.spyOn(window, 'removeEventListener');
    const { container, unmount } = show({ problems: problems(10) });
    expect(rows(container)).toHaveLength(1);
    measure.set(room(4));
    measure.resize();
    expect(rows(container)).toHaveLength(3);
    unmount();
    expect(remove).toHaveBeenCalledWith('resize', expect.any(Function));
    remove.mockRestore();
  });

  it('has nothing focusable', () => {
    installMeasure(room(5));
    const { container } = show({ problems: problems(3) });
    expect(container.querySelectorAll('a, button, input, [tabindex]')).toHaveLength(0);
  });
});
