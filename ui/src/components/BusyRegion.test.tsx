import { render, screen } from '@testing-library/preact';

import { BusyRegion } from './BusyRegion';

describe('BusyRegion', () => {
  it('marks its content busy and dimmed while pending', () => {
    const view = render(
      <BusyRegion pending={true} class="hosts-result">
        <p>rows</p>
      </BusyRegion>,
    );
    const region = screen.getByText('rows').parentElement;
    expect(region).toHaveClass('busy-region', 'is-busy', 'hosts-result');
    expect(region).toHaveAttribute('aria-busy', 'true');
    view.rerender(
      <BusyRegion pending={false} class="hosts-result">
        <p>rows</p>
      </BusyRegion>,
    );
    expect(region).not.toHaveClass('is-busy');
    expect(region).toHaveAttribute('aria-busy', 'false');
  });
});
