import { act, render, screen } from '@testing-library/preact';

import { ApiError } from '../api/client';
import { advance } from '../test/timers';
import { usePolled } from './poll';

let hidden = false;
function setHidden(value: boolean) {
  hidden = value;
  document.dispatchEvent(new Event('visibilitychange'));
}

beforeEach(() => {
  hidden = false;
  Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden });
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 10, 17, 14, 30, 0));
});

afterEach(() => {
  vi.useRealTimers();
});

type Loader = (signal: AbortSignal) => Promise<string>;

function Probe({ load, dep = 0, every }: { load: Loader; dep?: number; every?: number | 'once' }) {
  const { data, error, staleSince, pending, reload } = usePolled(load, [dep], every);
  return (
    <>
      <p>data={data ?? '-'}</p>
      <p>error={error?.code ?? '-'}</p>
      <p>pending={String(pending)}</p>
      <p>stale={staleSince === undefined ? '-' : staleSince.toISOString()}</p>
      <button onClick={reload}>reload</button>
    </>
  );
}

describe('usePolled', () => {
  it('loads at once and again after 10 s', async () => {
    const load = vi.fn<Loader>().mockResolvedValueOnce('one').mockResolvedValue('two');
    render(<Probe load={load} />);
    await advance(0);
    expect(load).toHaveBeenCalledTimes(1);
    expect(screen.getByText('data=one')).toBeInTheDocument();
    await advance(9999);
    expect(load).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(load).toHaveBeenCalledTimes(2);
    expect(screen.getByText('data=two')).toBeInTheDocument();
  });

  it('measures the interval from the arrival of the answer', async () => {
    let finish: (value: string) => void = () => {};
    const load = vi.fn<Loader>(
      () =>
        new Promise<string>((resolve) => {
          finish = resolve;
        }),
    );
    render(<Probe load={load} />);
    await advance(4000);
    await act(async () => {
      finish('slow');
      await Promise.resolve();
    });
    await advance(9999);
    expect(load).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(load).toHaveBeenCalledTimes(2);
  });

  it('does not load while the page is hidden and loads once when it becomes visible', async () => {
    const load = vi.fn<Loader>().mockResolvedValue('x');
    render(<Probe load={load} />);
    await advance(0);
    expect(load).toHaveBeenCalledTimes(1);
    await act(async () => {
      setHidden(true);
    });
    await advance(60000);
    expect(load).toHaveBeenCalledTimes(1);
    await act(async () => {
      setHidden(false);
    });
    await advance(0);
    expect(load).toHaveBeenCalledTimes(2);
  });

  it('does not start loading when it is mounted hidden', async () => {
    hidden = true;
    const load = vi.fn<Loader>().mockResolvedValue('x');
    render(<Probe load={load} />);
    await advance(30000);
    expect(load).not.toHaveBeenCalled();
    await act(async () => {
      setHidden(false);
    });
    await advance(0);
    expect(load).toHaveBeenCalledTimes(1);
  });

  it('keeps the data on an error, sets staleSince to the first failure, and clears it on success', async () => {
    const failure = new ApiError(0, 'network', 'down');
    const load = vi
      .fn<Loader>()
      .mockResolvedValueOnce('good')
      .mockRejectedValueOnce(failure)
      .mockRejectedValueOnce(failure)
      .mockResolvedValue('again');
    render(<Probe load={load} />);
    await advance(0);
    expect(screen.getByText('stale=-')).toBeInTheDocument();
    await advance(10000);
    const first = new Date(2026, 10, 17, 14, 30, 10).toISOString();
    expect(screen.getByText('data=good')).toBeInTheDocument();
    expect(screen.getByText('error=network')).toBeInTheDocument();
    expect(screen.getByText(`stale=${first}`)).toBeInTheDocument();
    await advance(10000);
    expect(screen.getByText(`stale=${first}`)).toBeInTheDocument();
    await advance(10000);
    expect(screen.getByText('data=again')).toBeInTheDocument();
    expect(screen.getByText('error=-')).toBeInTheDocument();
    expect(screen.getByText('stale=-')).toBeInTheDocument();
  });

  it('drops the data on a 403, also on a later poll', async () => {
    const forbidden = new ApiError(403, 'forbidden', 'no');
    const load = vi.fn<Loader>().mockResolvedValueOnce('secret').mockRejectedValue(forbidden);
    render(<Probe load={load} />);
    await advance(0);
    expect(screen.getByText('data=secret')).toBeInTheDocument();
    await advance(10000);
    expect(screen.getByText('data=-')).toBeInTheDocument();
    expect(screen.getByText('error=forbidden')).toBeInTheDocument();
    // It keeps asking: the role may be given back.
    await advance(10000);
    expect(load).toHaveBeenCalledTimes(3);
    expect(screen.getByText('data=-')).toBeInTheDocument();
  });

  it('does not put a 401 into error', async () => {
    const load = vi.fn<Loader>().mockRejectedValue(new ApiError(401, 'unauthorized', 'no'));
    render(<Probe load={load} />);
    await advance(0);
    expect(screen.getByText('error=-')).toBeInTheDocument();
    expect(screen.getByText('stale=-')).toBeInTheDocument();
  });

  it('never throws on an error that is not an ApiError', async () => {
    const load = vi.fn<Loader>().mockRejectedValue(new Error('boom'));
    render(<Probe load={load} />);
    await advance(0);
    expect(screen.getByText('error=internal')).toBeInTheDocument();
  });

  it('aborts the request and stops the timer on unmount', async () => {
    let seen: AbortSignal | undefined;
    const load = vi.fn<Loader>((signal) => {
      seen = signal;
      return new Promise<string>(() => {});
    });
    const view = render(<Probe load={load} />);
    await advance(0);
    expect(seen?.aborted).toBe(false);
    await act(async () => {
      view.unmount();
    });
    expect(seen?.aborted).toBe(true);
    await advance(60000);
    expect(load).toHaveBeenCalledTimes(1);
  });

  it('drops a late answer of the old request when the deps change', async () => {
    const answers: Array<(value: string) => void> = [];
    const load = vi.fn<Loader>(
      () =>
        new Promise<string>((resolve) => {
          answers.push(resolve);
        }),
    );
    const view = render(<Probe load={load} dep={1} />);
    await advance(0);
    view.rerender(<Probe load={load} dep={2} />);
    await advance(0);
    expect(load).toHaveBeenCalledTimes(2);
    await act(async () => {
      answers[1]?.('new');
      await Promise.resolve();
    });
    await act(async () => {
      answers[0]?.('old');
      await Promise.resolve();
    });
    expect(screen.getByText('data=new')).toBeInTheDocument();
  });

  it('reload loads at once and restarts the interval', async () => {
    const load = vi.fn<Loader>().mockResolvedValue('x');
    render(<Probe load={load} />);
    await advance(6000);
    expect(load).toHaveBeenCalledTimes(1);
    await act(async () => {
      screen.getByText('reload').click();
    });
    await advance(0);
    expect(load).toHaveBeenCalledTimes(2);
    await advance(9999);
    expect(load).toHaveBeenCalledTimes(2);
    await advance(1);
    expect(load).toHaveBeenCalledTimes(3);
  });

  it('honours another interval', async () => {
    const load = vi.fn<Loader>().mockResolvedValue('x');
    render(<Probe load={load} every={2000} />);
    await advance(0);
    await advance(2000);
    expect(load).toHaveBeenCalledTimes(2);
  });

  it('turns an exception thrown by load itself into an error and keeps polling', async () => {
    const load = vi
      .fn<Loader>()
      .mockImplementationOnce(() => {
        throw new Error('sync boom');
      })
      .mockResolvedValue('fine');
    render(<Probe load={load} />);
    await advance(0);
    expect(screen.getByText('error=internal')).toBeInTheDocument();
    await advance(10000);
    expect(load).toHaveBeenCalledTimes(2);
    expect(screen.getByText('data=fine')).toBeInTheDocument();
    expect(screen.getByText('error=-')).toBeInTheDocument();
  });
});

describe('usePolled pending', () => {
  type Answerer = { resolve: (v: string) => void; reject: (e: unknown) => void };

  function deferred() {
    const calls: Answerer[] = [];
    const load = vi.fn<Loader>(
      () =>
        new Promise<string>((resolve, reject) => {
          calls.push({ resolve, reject });
        }),
    );
    return { calls, load };
  }

  it('is false at mount and after the first answer, and not set by polls or reload', async () => {
    const { calls, load } = deferred();
    render(<Probe load={load} />);
    await advance(0);
    expect(screen.getByText('pending=false')).toBeInTheDocument();
    await act(async () => {
      calls[0]?.resolve('a');
      await Promise.resolve();
    });
    expect(screen.getByText('pending=false')).toBeInTheDocument();
    await advance(10000);
    expect(load).toHaveBeenCalledTimes(2);
    expect(screen.getByText('pending=false')).toBeInTheDocument();
    await act(async () => {
      screen.getByText('reload').click();
    });
    await advance(0);
    expect(load).toHaveBeenCalledTimes(3);
    expect(screen.getByText('pending=false')).toBeInTheDocument();
  });

  it('is true after a deps change, clears error and staleSince at once, keeps the data', async () => {
    const { calls, load } = deferred();
    const view = render(<Probe load={load} dep={1} />);
    await advance(0);
    await act(async () => {
      calls[0]?.resolve('host-a');
      await Promise.resolve();
    });
    await advance(10000);
    await act(async () => {
      calls[1]?.reject(new ApiError(0, 'network', 'down'));
      await Promise.resolve();
    });
    expect(screen.getByText('error=network')).toBeInTheDocument();
    view.rerender(<Probe load={load} dep={2} />);
    expect(screen.getByText('pending=true')).toBeInTheDocument();
    expect(screen.getByText('error=-')).toBeInTheDocument();
    expect(screen.getByText('stale=-')).toBeInTheDocument();
    expect(screen.getByText('data=host-a')).toBeInTheDocument();
    await advance(0);
    await act(async () => {
      calls[2]?.resolve('host-b');
      await Promise.resolve();
    });
    expect(screen.getByText('pending=false')).toBeInTheDocument();
    expect(screen.getByText('data=host-b')).toBeInTheDocument();
  });

  it('drops the old data when the first answer for the new deps is a failure', async () => {
    const { calls, load } = deferred();
    const view = render(<Probe load={load} dep={1} />);
    await advance(0);
    await act(async () => {
      calls[0]?.resolve('host-a');
      await Promise.resolve();
    });
    view.rerender(<Probe load={load} dep={2} />);
    await advance(0);
    await act(async () => {
      calls[1]?.reject(new ApiError(404, 'not_found', 'no such host'));
      await Promise.resolve();
    });
    expect(screen.getByText('pending=false')).toBeInTheDocument();
    expect(screen.getByText('data=-')).toBeInTheDocument();
    expect(screen.getByText('error=not_found')).toBeInTheDocument();
  });
});

describe('usePolled once', () => {
  it('loads once and never again by itself', async () => {
    const load = vi.fn<Loader>().mockResolvedValue('list');
    render(<Probe load={load} every="once" />);
    await advance(0);
    expect(screen.getByText('data=list')).toBeInTheDocument();
    await advance(3_600_000);
    expect(load).toHaveBeenCalledTimes(1);
  });

  it('loads again on reload', async () => {
    const load = vi.fn<Loader>().mockResolvedValueOnce('one').mockResolvedValue('two');
    render(<Probe load={load} every="once" />);
    await advance(0);
    await act(async () => {
      screen.getByText('reload').click();
    });
    await advance(0);
    expect(load).toHaveBeenCalledTimes(2);
    expect(screen.getByText('data=two')).toBeInTheDocument();
    await advance(60_000);
    expect(load).toHaveBeenCalledTimes(2);
  });

  it('retries once when the tab becomes visible, only after a failure', async () => {
    const load = vi
      .fn<Loader>()
      .mockRejectedValueOnce(new ApiError(0, 'network', 'down'))
      .mockResolvedValue('list');
    render(<Probe load={load} every="once" />);
    await advance(0);
    expect(screen.getByText('error=network')).toBeInTheDocument();
    await advance(60_000);
    expect(load).toHaveBeenCalledTimes(1);
    await act(async () => {
      setHidden(true);
      setHidden(false);
    });
    await advance(0);
    expect(load).toHaveBeenCalledTimes(2);
    expect(screen.getByText('data=list')).toBeInTheDocument();
    await act(async () => {
      setHidden(true);
      setHidden(false);
    });
    await advance(0);
    expect(load).toHaveBeenCalledTimes(2);
  });

  it('does not load while hidden and loads when it becomes visible', async () => {
    hidden = true;
    const load = vi.fn<Loader>().mockResolvedValue('list');
    render(<Probe load={load} every="once" />);
    await advance(0);
    expect(load).not.toHaveBeenCalled();
    await act(async () => {
      setHidden(false);
    });
    await advance(0);
    expect(load).toHaveBeenCalledTimes(1);
  });
});
