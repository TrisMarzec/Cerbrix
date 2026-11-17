import { fireEvent, render, screen } from '@testing-library/preact';

import { Link, match, navigate, useRoute } from './router';

beforeEach(() => {
  window.history.replaceState(null, '', '/');
});

describe('match', () => {
  it('decodes a parameter', () => {
    expect(match('/hosts/:name', '/hosts/db%2D02.example')).toEqual({ name: 'db-02.example' });
  });

  it('does not match a longer or different path', () => {
    expect(match('/hosts/:name', '/hosts/a/b')).toBeNull();
    expect(match('/hosts/:name', '/hosts')).toBeNull();
    expect(match('/hosts', '/problems')).toBeNull();
  });

  it('ignores a trailing slash and matches the root', () => {
    expect(match('/hosts', '/hosts/')).toEqual({});
    expect(match('/', '/')).toEqual({});
  });

  it('gives null for a malformed escape', () => {
    expect(match('/hosts/:name', '/hosts/%E0%A4%A')).toBeNull();
  });

  it('compares fixed segments after decoding', () => {
    expect(match('/hosts', '/%68osts')).toEqual({});
  });
});

describe('Link with a target that is not a path of this app', () => {
  it('renders plain text, not a link', () => {
    render(
      <>
        <Link href="javascript:alert(1)">script</Link>
        <Link href="//evil.example">scheme-less</Link>
        <Link href={'/\\evil.example'}>backslash</Link>
        <Link href="https://evil.example">absolute</Link>
      </>,
    );
    expect(screen.queryAllByRole('link')).toHaveLength(0);
    expect(screen.getByText('script')).toBeInTheDocument();
    expect(screen.getByText('scheme-less')).toBeInTheDocument();
    expect(screen.getByText('backslash')).toBeInTheDocument();
  });
});

describe('Link', () => {
  it('navigates on a plain click and prevents the page load', () => {
    render(<Link href="/hosts?state=problem">Hosts</Link>);
    const link = screen.getByRole('link', { name: 'Hosts' });
    expect(link).toHaveAttribute('href', '/hosts?state=problem');
    const notPrevented = fireEvent.click(link);
    expect(notPrevented).toBe(false);
    expect(window.location.pathname + window.location.search).toBe('/hosts?state=problem');
  });

  it('leaves a click with Ctrl held to the browser', () => {
    render(<Link href="/hosts">Hosts</Link>);
    // What the link left to the browser; the listener then stops jsdom from trying to load the page.
    const preventedByLink: boolean[] = [];
    const record = (e: Event) => {
      preventedByLink.push(e.defaultPrevented);
      e.preventDefault();
    };
    document.body.addEventListener('click', record);
    fireEvent.click(screen.getByRole('link'), { ctrlKey: true });
    document.body.removeEventListener('click', record);
    expect(preventedByLink).toEqual([false]);
    expect(window.location.pathname).toBe('/');
  });
});

function Show() {
  const { path, query } = useRoute();
  return (
    <p>
      {path}|{query.get('q') ?? ''}
    </p>
  );
}

describe('useRoute and navigate', () => {
  it('follows navigate and the back button', async () => {
    render(<Show />);
    expect(screen.getByText('/|')).toBeInTheDocument();
    navigate('/problems?q=a');
    expect(await screen.findByText('/problems|a')).toBeInTheDocument();
    window.history.back();
    expect(await screen.findByText('/|')).toBeInTheDocument();
  });

  it('does nothing when the URL is the current one', () => {
    const push = vi.spyOn(window.history, 'pushState');
    navigate('/');
    expect(push).not.toHaveBeenCalled();
    push.mockRestore();
  });

  it('replaces the entry when asked', () => {
    const replace = vi.spyOn(window.history, 'replaceState');
    navigate('/hosts', { replace: true });
    expect(replace).toHaveBeenCalledTimes(1);
    expect(window.location.pathname).toBe('/hosts');
    replace.mockRestore();
  });
});
