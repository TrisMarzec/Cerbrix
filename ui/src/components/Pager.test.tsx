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

  it('renders nothing for an empty list', () => {
    const { container } = render(<Pager total={0} limit={50} offset={0} onChange={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });
});
