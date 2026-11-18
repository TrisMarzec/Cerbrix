import { act, fireEvent, render, screen } from '@testing-library/preact';

import { advance } from '../test/timers';
import { SearchField } from './SearchField';

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

function field(urlValue: string, onCommit: (text: string) => void) {
  return (
    <SearchField
      urlValue={urlValue}
      onCommit={onCommit}
      label="Search"
      placeholder="Name"
      class="x-search"
      maxLength={20}
    />
  );
}

function show(urlValue: string) {
  const onCommit = vi.fn();
  const view = render(field(urlValue, onCommit));
  return { onCommit, view };
}

describe('SearchField', () => {
  it('has a label, a placeholder, the class and the length limit', () => {
    show('');
    const input = screen.getByRole('searchbox', { name: 'Search' });
    expect(input).toHaveAttribute('placeholder', 'Name');
    expect(input).toHaveAttribute('maxlength', '20');
    expect(screen.getByRole('search')).toHaveClass('search-field', 'x-search');
  });

  it('commits 300 ms after the last key stroke', async () => {
    const { onCommit } = show('');
    fireEvent.input(screen.getByRole('searchbox'), { target: { value: 'db' } });
    await advance(299);
    expect(onCommit).not.toHaveBeenCalled();
    await advance(1);
    expect(onCommit).toHaveBeenCalledWith('db');
  });

  it('commits at once on Enter', () => {
    const { onCommit } = show('');
    fireEvent.input(screen.getByRole('searchbox'), { target: { value: 'db' } });
    fireEvent.submit(screen.getByRole('search'));
    expect(onCommit).toHaveBeenCalledWith('db');
  });

  it('does not commit text equal to the URL', async () => {
    const { onCommit } = show('db');
    fireEvent.input(screen.getByRole('searchbox'), { target: { value: 'db' } });
    await advance(300);
    fireEvent.submit(screen.getByRole('search'));
    expect(onCommit).not.toHaveBeenCalled();
  });

  it('clears, commits the empty text and keeps the focus in the field', async () => {
    const { onCommit } = show('db');
    await act(async () => {
      screen.getByRole('button', { name: 'Clear search' }).click();
    });
    expect(onCommit).toHaveBeenCalledWith('');
    expect(screen.getByRole('searchbox')).toHaveFocus();
  });

  it('follows a change of the URL', () => {
    const { view } = show('db');
    view.rerender(field('web', () => {}));
    expect(screen.getByRole('searchbox')).toHaveValue('web');
  });
});
