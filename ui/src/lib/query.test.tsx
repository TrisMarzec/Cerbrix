import { fireEvent, render, screen } from '@testing-library/preact';

import { useQueryState } from './query';

function Probe({ changes }: { changes: Record<string, string | undefined> }) {
  const [query, setQuery] = useQueryState();
  return (
    <>
      <p>state={query.get('state') ?? '-'}</p>
      <button onClick={() => setQuery(changes)}>apply</button>
    </>
  );
}

beforeEach(() => {
  window.history.replaceState(null, '', '/problems?state=open&offset=100&q=disk');
});

describe('useQueryState', () => {
  it('removes offset when a filter changes', async () => {
    render(<Probe changes={{ state: 'resolved' }} />);
    fireEvent.click(screen.getByText('apply'));
    expect(window.location.search).toBe('?state=resolved&q=disk');
    expect(await screen.findByText('state=resolved')).toBeInTheDocument();
  });

  it('keeps an offset that is set on purpose', () => {
    render(<Probe changes={{ offset: '150' }} />);
    fireEvent.click(screen.getByText('apply'));
    expect(window.location.search).toBe('?state=open&offset=150&q=disk');
  });

  it('removes a key set to undefined and leaves no question mark when empty', () => {
    render(<Probe changes={{ state: undefined, q: undefined }} />);
    fireEvent.click(screen.getByText('apply'));
    expect(window.location.search).toBe('');
  });

  it('replaces the history entry instead of adding one', () => {
    const push = vi.spyOn(window.history, 'pushState');
    const replace = vi.spyOn(window.history, 'replaceState');
    render(<Probe changes={{ state: 'resolved' }} />);
    fireEvent.click(screen.getByText('apply'));
    expect(replace).toHaveBeenCalled();
    expect(push).not.toHaveBeenCalled();
    push.mockRestore();
    replace.mockRestore();
  });
});
