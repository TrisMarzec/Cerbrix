import { render } from '@testing-library/preact';

import { criticalState, warningAcknowledgedState } from '../test/fixtures';
import { HostSquares } from './HostSquares';

describe('HostSquares', () => {
  it('draws one square per host with a class by state', () => {
    const { container } = render(
      <HostSquares
        hosts={[
          { name: 'a', state: criticalState },
          { name: 'b', state: warningAcknowledgedState },
          { name: 'c', state: null },
        ]}
      />,
    );
    const squares = container.querySelectorAll('.square');
    expect(squares).toHaveLength(3);
    expect(squares[0]).toHaveClass('sev-critical');
    expect(squares[1]).toHaveClass('sev-warning', 'is-acknowledged');
    expect(squares[2]).toHaveClass('is-ok');
    expect(squares[0]).toHaveAttribute('title', 'a: Critical');
    expect(squares[1]).toHaveAttribute('title', 'b: Warning, acknowledged');
    expect(squares[2]).toHaveAttribute('title', 'c: No problems');
  });

  it('has a text alternative', () => {
    const { getByRole } = render(
      <HostSquares
        hosts={[
          { name: 'a', state: criticalState },
          { name: 'c', state: null },
        ]}
      />,
    );
    expect(getByRole('img')).toHaveAccessibleName('2 hosts, 1 with problems');
  });

  it('cuts the strip at max and says how many are left', () => {
    const hosts = ['a', 'b', 'c', 'd', 'e'].map((name) => ({ name, state: null }));
    const { container, getByText } = render(<HostSquares hosts={hosts} max={3} />);
    expect(container.querySelectorAll('.square')).toHaveLength(3);
    expect(getByText('and 2 more')).toBeInTheDocument();
  });
});
