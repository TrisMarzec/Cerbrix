import { render, screen } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';

import { Pager } from './Pager';

describe('Pager', () => {
  it('shows the range', () => {
    render(<Pager total={137} limit={50} offset={0} onChange={() => {}} />);
    expect(screen.getByText('1–50 of 137')).toBeInTheDocument();
  });

  it('shows a short last page', () => {
    render(<Pager total={137} limit={50} offset={100} onChange={() => {}} />);
    expect(screen.getByText('101–137 of 137')).toBeInTheDocument();
  });

  it('disables Previous on the first page and Next on the last', () => {
    const { rerender } = render(<Pager total={137} limit={50} offset={0} onChange={() => {}} />);
    expect(screen.getByRole('button', { name: 'Previous' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled();
    rerender(<Pager total={137} limit={50} offset={100} onChange={() => {}} />);
    expect(screen.getByRole('button', { name: 'Previous' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
  });

  it('reports the new offset', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Pager total={137} limit={50} offset={50} onChange={onChange} />);
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(onChange).toHaveBeenLastCalledWith(100);
    await user.click(screen.getByRole('button', { name: 'Previous' }));
    expect(onChange).toHaveBeenLastCalledWith(0);
  });

  it('does not go below zero', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Pager total={137} limit={50} offset={20} onChange={onChange} />);
    await user.click(screen.getByRole('button', { name: 'Previous' }));
    expect(onChange).toHaveBeenLastCalledWith(0);
  });

  it('shows the last existing page when the offset is beyond the total', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Pager total={137} limit={50} offset={200} onChange={onChange} />);
    expect(screen.getByText('101–137 of 137')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Previous' }));
    expect(onChange).toHaveBeenLastCalledWith(100);
  });

  it('renders nothing for an empty list', () => {
    const { container } = render(<Pager total={0} limit={50} offset={0} onChange={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });
});

describe('Pager while the next page loads', () => {
  it('marks both buttons aria-disabled but keeps them focusable', () => {
    render(<Pager total={137} limit={50} offset={50} disabled onChange={() => {}} />);
    for (const name of ['Previous', 'Next']) {
      const button = screen.getByRole('button', { name });
      expect(button).toHaveAttribute('aria-disabled', 'true');
      expect(button).not.toBeDisabled();
    }
    expect(screen.getByText('51–100 of 137')).toBeInTheDocument();
  });

  it('keeps focus on Next after it was activated and ignores a second activation', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const { rerender } = render(<Pager total={137} limit={50} offset={0} onChange={onChange} />);
    const next = screen.getByRole('button', { name: 'Next' });
    next.focus();
    await user.keyboard('{Enter}');
    expect(onChange).toHaveBeenCalledTimes(1);
    rerender(<Pager total={137} limit={50} offset={0} disabled onChange={onChange} />);
    expect(next).toHaveFocus();
    await user.keyboard('{Enter}');
    await user.click(next);
    expect(onChange).toHaveBeenCalledTimes(1);
    rerender(<Pager total={137} limit={50} offset={50} onChange={onChange} />);
    expect(next).toHaveFocus();
    expect(next).not.toHaveAttribute('aria-disabled');
  });

  it('keeps the real disabled at the first and the last page', () => {
    const { rerender } = render(<Pager total={137} limit={50} offset={0} disabled onChange={() => {}} />);
    expect(screen.getByRole('button', { name: 'Previous' })).toBeDisabled();
    rerender(<Pager total={137} limit={50} offset={100} disabled onChange={() => {}} />);
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
  });
});
