import { screen } from '@testing-library/preact';

import type { Problem } from '../api/types';
import { acknowledgedProblem, criticalProblem } from '../test/fixtures';
import { renderAt } from '../test/render';
import { ProblemMeta, problemName, ProblemRow, resolveText } from './ProblemRow';

describe('ProblemRow', () => {
  it('shows host, trigger, summary, severity word and duration', () => {
    const { container } = renderAt((now) => <ProblemRow problem={criticalProblem} now={now} showHost />);
    expect(screen.getByText('db-01')).toBeInTheDocument();
    expect(screen.getByText(/Disk almost full/)).toBeInTheDocument();
    expect(screen.getByText(/mount=\//)).toBeInTheDocument();
    expect(screen.getByText(/disk\.used_percent > 90/)).toBeInTheDocument();
    expect(screen.getByText('Critical')).toBeInTheDocument();
    expect(screen.getByText('2 h 14 min')).toBeInTheDocument();
    expect(container.querySelector('.problem-row')).toHaveClass('sev-critical');
    expect(container.querySelector('.problem-row')).not.toHaveClass('is-acknowledged');
  });

  it('leaves out the host unless asked', () => {
    renderAt((now) => <ProblemRow problem={criticalProblem} now={now} />);
    expect(screen.queryByText('db-01')).toBeNull();
  });

  it('shows who acknowledged, when, and the comment', () => {
    const { container } = renderAt((now) => (
      <ProblemRow problem={acknowledgedProblem} now={now} showHost />
    ));
    expect(container.querySelector('.problem-row')).toHaveClass('is-acknowledged');
    expect(container.querySelector('.problem-row')).toHaveClass('sev-warning');
    expect(screen.getByText(/acknowledged by anna, 14:05/)).toBeInTheDocument();
    expect(screen.getByText(/watching/)).toBeInTheDocument();
  });

  it('renders text from the API as text', () => {
    const problem = {
      ...acknowledgedProblem,
      host: '<img src=x onerror=alert(1)>',
      ack_comment: '<b>x</b> & "y"',
    };
    const { container } = renderAt((now) => <ProblemRow problem={problem} now={now} showHost />);
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('b')).toBeNull();
    expect(screen.getByText('<img src=x onerror=alert(1)>')).toBeInTheDocument();
    expect(screen.getByText(/<b>x<\/b> & "y"/)).toBeInTheDocument();
  });

  it('gives a long host name a title with the full name', () => {
    const host = 'h'.repeat(200);
    renderAt((now) => <ProblemRow problem={{ ...criticalProblem, host }} now={now} showHost />);
    expect(screen.getByTitle(host)).toBeInTheDocument();
  });

  it('puts long unbroken text in elements that wrap', () => {
    const long = 'x'.repeat(200);
    const problem = { ...criticalProblem, trigger: long, instance: `path=/${long}`, summary: long };
    const { container } = renderAt((now) => <ProblemRow problem={problem} now={now} />);
    expect(container.querySelector('.problem-name')).toHaveTextContent(long);
    expect(container.querySelector('.problem-name')).toHaveTextContent(`path=/${long}`);
    expect(container.querySelector('.problem-summary')).toHaveTextContent(long);
  });

  it('renders actions at the end', () => {
    renderAt((now) => (
      <ProblemRow
        problem={criticalProblem}
        now={now}
        actions={<button type="button">Acknowledge</button>}
      />
    ));
    expect(screen.getByRole('button', { name: 'Acknowledge' })).toBeInTheDocument();
  });

  it('uses the end of a resolved problem for the duration', () => {
    const resolved = { ...criticalProblem, resolved_at: new Date(2026, 10, 17, 12, 46).toISOString() };
    renderAt((now) => <ProblemRow problem={resolved} now={now} />);
    expect(screen.getByText('30 min')).toBeInTheDocument();
  });
});

describe('problem texts', () => {
  it('names a problem by trigger and instance', () => {
    expect(problemName(criticalProblem)).toBe('Disk almost full mount=/');
    expect(problemName(acknowledgedProblem)).toBe('Memory high');
  });

  it('says acknowledged without a name when the user is unknown', () => {
    const problem = { ...acknowledgedProblem, acknowledged_by: null, ack_comment: null };
    const { container } = renderAt((now) => <ProblemMeta problem={problem} now={now} />);
    expect(container).toHaveTextContent(/^Warning · acknowledged, 14:05$/);
    expect(container).not.toHaveTextContent('unknown');
  });

  it('writes the severity, who acknowledged and the comment', () => {
    const { container } = renderAt((now) => <ProblemMeta problem={acknowledgedProblem} now={now} />);
    expect(container).toHaveTextContent('Warning · acknowledged by anna, 14:05 · watching');
  });

  it('says how a problem was resolved', () => {
    const base = { ...criticalProblem, resolved_at: criticalProblem.started_at };
    expect(resolveText(criticalProblem)).toBeUndefined();
    expect(resolveText({ ...base, resolve_reason: 'condition_ended' })).toBe('ended by itself');
    expect(resolveText({ ...base, resolve_reason: 'host_removed' })).toBe('host removed');
    expect(resolveText({ ...base, resolve_reason: 'trigger_removed' })).toBe('trigger removed');
    expect(resolveText({ ...base, resolve_reason: 'closed_by_operator', closed_by: 'anna' })).toBe('closed by anna');
    expect(resolveText({ ...base, resolve_reason: 'closed_by_operator', closed_by: null })).toBe('closed by hand');
    // A reason a later server version adds is shown as it was sent.
    const unknown = 'merged' as unknown as Problem['resolve_reason'];
    expect(resolveText({ ...base, resolve_reason: unknown })).toBe('merged');
  });
});
