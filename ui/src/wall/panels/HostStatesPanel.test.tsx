import { render, screen } from '@testing-library/preact';

import type { DashboardHost, HostStatesPanelData } from '../../api/types';
import { criticalState, hostStatesPanelFixture, warningAcknowledgedState } from '../../test/fixtures';
import { installMeasure, releaseMeasure, type Sizes } from '../test/measure';
import { HostStatesPanel } from './HostStatesPanel';

const LINE = 40;
const SQUARE = 10;
const GAP = 2;

/** A body with the sentence and room for `columns` by `rows` squares below it. */
function room(columns: number, rows: number): Partial<Sizes> {
  return {
    bodyWidth: columns * (SQUARE + GAP) - GAP,
    bodyHeight: LINE + GAP + rows * (SQUARE + GAP) - GAP,
    lineHeight: LINE,
    square: SQUARE,
    gap: GAP,
    charWidth: 2,
  };
}

function ok(n: number, from = 0): DashboardHost[] {
  return Array.from({ length: n }, (_, i) => ({ name: `ok-${from + i}`, state: null }));
}

const mixed: DashboardHost[] = [
  { name: 'db-01', state: criticalState },
  { name: 'db-02', state: { ...criticalState, instance: 'mount=/var' } },
  { name: 'app-03', state: { ...warningAcknowledgedState, acknowledged: false } },
  { name: 'web-02', state: warningAcknowledgedState },
  ...ok(40),
];

function show(panel: Partial<HostStatesPanelData>) {
  return render(<HostStatesPanel panel={hostStatesPanelFixture(panel)} />);
}

function squares(container: Element): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>('.wall-square:not(.wall-sample)')];
}

function more(container: Element): HTMLElement | null {
  return container.querySelector<HTMLElement>('.wall-squares-more:not(.wall-sample)');
}

afterEach(() => {
  releaseMeasure();
});

describe('HostStatesPanel', () => {
  it('is labelled "Hosts"', () => {
    installMeasure(room(11, 6));
    const { container } = show({});
    expect(container.querySelector('.wall-panel-label')).toHaveTextContent('Hosts');
  });

  it('says how many of the hosts are without problems', () => {
    installMeasure(room(11, 6));
    show({ total: 44, without_problems: 40, hosts: mixed });
    const sentence = screen.getByText('40 of 44 without problems');
    expect(sentence).not.toHaveClass('is-ok');
  });

  it('says "All N without problems" in the healthy colour when none has a problem', () => {
    installMeasure(room(11, 6));
    show({ total: 44, without_problems: 44, hosts: ok(44) });
    expect(screen.getByText('All 44 without problems')).toHaveClass('is-ok');
  });

  it('says "No hosts" for an empty selection and shows no squares', () => {
    installMeasure(room(11, 6));
    const { container } = show({ total: 0, without_problems: 0, hosts: [] });
    expect(screen.getByText('No hosts')).toBeInTheDocument();
    expect(squares(container)).toHaveLength(0);
  });

  it('shows one square per host in the server order, coloured by state', () => {
    installMeasure(room(11, 6));
    const { container } = show({ total: 44, without_problems: 40, hosts: mixed });
    const shown = squares(container);
    expect(shown).toHaveLength(44);
    expect(shown[0]).toHaveClass('sev-critical');
    expect(shown[1]).toHaveClass('sev-critical');
    expect(shown[2]).toHaveClass('sev-warning');
    expect(shown[2]).not.toHaveClass('is-acknowledged');
    expect(shown[3]).toHaveClass('sev-warning', 'is-acknowledged');
    expect(shown[4]).toHaveClass('is-ok');
    expect(shown.slice(4).every((s) => s.classList.contains('is-ok'))).toBe(true);
  });

  it('describes the states in words as the text alternative', () => {
    installMeasure(room(11, 6));
    const { container } = show({ total: 44, without_problems: 40, hosts: mixed });
    expect(container.querySelector('.wall-panel-body')).toHaveAttribute('role', 'img');
    expect(screen.getByRole('img')).toHaveAccessibleName(
      '44 hosts: 2 critical, 1 warning, 1 acknowledged, 40 without problems',
    );
  });

  it('replaces the squares that do not fit with "+N" in the last cell', () => {
    installMeasure(room(12, 10));
    const hosts = [{ name: 'db-01', state: criticalState }, ...ok(499, 1)];
    const { container } = show({ total: 500, without_problems: 499, hosts });
    expect(squares(container)).toHaveLength(119);
    expect(more(container)).toHaveTextContent('+381');
    expect(screen.getByRole('img')).toHaveAccessibleName(
      '500 hosts: 1 critical, 499 without problems; 119 shown, and 381 more hosts',
    );
  });

  it('gives a short "+N" one cell', () => {
    installMeasure(room(11, 4));
    const { container } = show({ total: 46, without_problems: 46, hosts: ok(46) });
    expect(squares(container)).toHaveLength(43);
    expect(more(container)).toHaveTextContent('+3');
    expect(more(container)?.style.gridColumn).toBe('span 1');
  });

  it('reserves as many cells as a wide "+N" needs, so it is never cut', () => {
    // "+1349" (the largest N) is 50 wide: (50 + 2) / (10 + 2) rounded up is 5 cells.
    installMeasure({ ...room(12, 10), charWidth: 10 });
    const { container } = show({ total: 1349, without_problems: 1349, hosts: ok(1349) });
    expect(squares(container)).toHaveLength(115);
    expect(more(container)).toHaveTextContent('+1234');
    expect(more(container)?.style.gridColumn).toBe('span 5');
  });

  it('never shows more squares and summary cells than the room holds', () => {
    for (const [columns, rows, hostCount, charWidth] of [
      [11, 4, 46, 2],
      [12, 10, 1349, 10],
      [3, 3, 100, 4],
      [7, 1, 8, 6],
      [5, 2, 11, 2],
    ] as const) {
      installMeasure({ ...room(columns, rows), charWidth });
      const { container, unmount } = show({ total: hostCount, without_problems: 0, hosts: ok(hostCount) });
      const span = Number(/span (\d+)/.exec(more(container)?.style.gridColumn ?? '')?.[1] ?? 0);
      expect(squares(container).length + span).toBeLessThanOrEqual(columns * rows);
      unmount();
      releaseMeasure();
    }
  });

  it('counts the hosts the server did not send into "+N"', () => {
    installMeasure(room(11, 4));
    const { container } = show({ total: 15, without_problems: 15, hosts: ok(10) });
    expect(squares(container)).toHaveLength(10);
    expect(more(container)).toHaveTextContent('+5');
  });

  it('shows the sentence only when not even one square and "+N" fit', () => {
    // "+44" is 30 wide and needs 3 cells; a row has 2.
    installMeasure({ ...room(2, 6), charWidth: 10 });
    const { container } = show({ total: 44, without_problems: 40, hosts: mixed });
    expect(screen.getByText('40 of 44 without problems')).toBeInTheDocument();
    expect(squares(container)).toHaveLength(0);
    expect(more(container)).toBeNull();
  });

  it('words a single host without "All"', () => {
    installMeasure(room(11, 6));
    show({ total: 1, without_problems: 1, hosts: ok(1) });
    expect(screen.getByText('1 host without problems')).toHaveClass('is-ok');
  });

  it('says "1 host with a problem" for a single broken host', () => {
    installMeasure(room(11, 6));
    show({ total: 1, without_problems: 0, hosts: [{ name: 'db-01', state: criticalState }] });
    expect(screen.getByText('1 host with a problem')).not.toHaveClass('is-ok');
  });

  it('shows the sentence without squares when no square fits', () => {
    installMeasure({ ...room(11, 6), bodyHeight: LINE + GAP + SQUARE - 1 });
    const { container } = show({ total: 44, without_problems: 40, hosts: mixed });
    expect(screen.getByText('40 of 44 without problems')).toBeInTheDocument();
    expect(squares(container)).toHaveLength(0);
  });

  it('shows only its label when the sentence does not fit', () => {
    installMeasure({ ...room(11, 6), bodyHeight: LINE - 1 });
    const { container } = show({ total: 44, without_problems: 40, hosts: mixed });
    expect(screen.queryByText('40 of 44 without problems')).toBeNull();
    expect(squares(container)).toHaveLength(0);
    expect(container.querySelector('.wall-panel-label')).toHaveTextContent('Hosts');
  });

  it('measures again when the body changes size', () => {
    const measure = installMeasure(room(5, 2));
    const { container } = show({ total: 44, without_problems: 40, hosts: mixed });
    expect(squares(container)).toHaveLength(9);
    measure.set(room(11, 6));
    measure.resize();
    expect(squares(container)).toHaveLength(44);
  });

  it('has no focusable squares and no titles on them', () => {
    installMeasure(room(11, 6));
    const { container } = show({ total: 44, without_problems: 40, hosts: mixed });
    expect(container.querySelectorAll('a, button, input, [tabindex], [title]')).toHaveLength(0);
  });

  it('disconnects its observer on unmount', () => {
    const measure = installMeasure(room(11, 6));
    const { unmount } = show({});
    const [observer] = measure.observers();
    expect(observer?.targets.size).toBe(1);
    unmount();
    expect(observer?.disconnected).toBe(true);
  });
});
