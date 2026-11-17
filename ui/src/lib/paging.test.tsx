import { act, render, screen } from '@testing-library/preact';

import { lastPageStart, parseOffset, usePaging } from './paging';

afterEach(() => {
  window.history.replaceState(null, '', '/');
});

describe('parseOffset', () => {
  it('reads a whole number and falls back to 0 for anything else', () => {
    expect(parseOffset('50')).toBe(50);
    expect(parseOffset(null)).toBe(0);
    expect(parseOffset('-5')).toBe(0);
    expect(parseOffset('1e3')).toBe(0);
    expect(parseOffset('abc')).toBe(0);
    expect(parseOffset('99999999999999999999')).toBe(0);
  });
});

describe('lastPageStart', () => {
  it('is the start of the last page that exists', () => {
    expect(lastPageStart(1, 50)).toBe(0);
    expect(lastPageStart(50, 50)).toBe(0);
    expect(lastPageStart(51, 50)).toBe(50);
    expect(lastPageStart(120, 20)).toBe(100);
  });
});

function Probe({ total, pending }: { total: number | undefined; pending: boolean }) {
  const paging = usePaging('page_offset', 20);
  const { beyond, shownOffset } = paging.correct(total, pending);
  return (
    <>
      <p>offset={paging.offset}</p>
      <p>shown={shownOffset}</p>
      <p>beyond={String(beyond)}</p>
      <button
        type="button"
        onClick={() => {
          paging.set(40);
        }}
      >
        next
      </button>
      <button
        type="button"
        onClick={() => {
          paging.set(0);
        }}
      >
        first
      </button>
    </>
  );
}

describe('usePaging', () => {
  it('reads and writes its parameter, leaving it out for the first page', async () => {
    window.history.replaceState(null, '', '/x?state=open');
    render(<Probe total={100} pending={false} />);
    expect(screen.getByText('offset=0')).toBeInTheDocument();
    await act(async () => {
      screen.getByText('next').click();
    });
    expect(window.location.search).toBe('?state=open&page_offset=40');
    expect(screen.getByText('offset=40')).toBeInTheDocument();
    await act(async () => {
      screen.getByText('first').click();
    });
    expect(window.location.search).toBe('?state=open');
  });

  it('goes to the last page that exists when the offset is beyond the total, once the answer is in', async () => {
    window.history.replaceState(null, '', '/x?page_offset=200');
    const view = render(<Probe total={45} pending={true} />);
    // While the answer for the offset is outstanding nothing is corrected.
    expect(window.location.search).toBe('?page_offset=200');
    await act(async () => {
      view.rerender(<Probe total={45} pending={false} />);
    });
    expect(window.location.search).toBe('?page_offset=40');
  });

  it('says beyond for an offset past the total and nothing for an empty list', () => {
    window.history.replaceState(null, '', '/x?page_offset=60');
    const view = render(<Probe total={60} pending={true} />);
    expect(screen.getByText('beyond=true')).toBeInTheDocument();
    view.rerender(<Probe total={0} pending={true} />);
    expect(screen.getByText('beyond=false')).toBeInTheDocument();
    view.rerender(<Probe total={undefined} pending={true} />);
    expect(screen.getByText('beyond=false')).toBeInTheDocument();
  });

  it('keeps the offset of the rows on screen while the next page is pending', async () => {
    window.history.replaceState(null, '', '/x?page_offset=20');
    const view = render(<Probe total={100} pending={false} />);
    expect(screen.getByText('shown=20')).toBeInTheDocument();
    // usePolled reports pending from the render in which the offset changes on.
    view.rerender(<Probe total={100} pending={true} />);
    await act(async () => {
      screen.getByText('next').click();
    });
    expect(screen.getByText('offset=40')).toBeInTheDocument();
    expect(screen.getByText('shown=20')).toBeInTheDocument();
    view.rerender(<Probe total={100} pending={false} />);
    expect(screen.getByText('shown=40')).toBeInTheDocument();
  });
});
