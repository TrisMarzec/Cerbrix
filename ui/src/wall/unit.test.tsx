import { act, render } from '@testing-library/preact';
import { vi } from 'vitest';

import { advance } from '../test/timers';
import { useWallUnit } from './unit';

let renders = 0;

function Probe() {
  renders += 1;
  return <span data-testid="u">{useWallUnit()}</span>;
}

const original = Object.getOwnPropertyDescriptor(window, 'innerWidth');

function setWidth(width: number) {
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: width });
}

function resize(width: number) {
  setWidth(width);
  act(() => {
    window.dispatchEvent(new Event('resize'));
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  renders = 0;
});

afterEach(() => {
  vi.useRealTimers();
  if (original === undefined) Reflect.deleteProperty(window, 'innerWidth');
  else Object.defineProperty(window, 'innerWidth', original);
  vi.restoreAllMocks();
});

describe('useWallUnit', () => {
  it('is a 1920th of the window width', () => {
    setWidth(3840);
    const { getByTestId } = render(<Probe />);
    expect(getByTestId('u')).toHaveTextContent('2');
  });

  it('takes a new width 150 ms after the last resize, once for a whole drag', async () => {
    setWidth(1920);
    const { getByTestId } = render(<Probe />);
    const before = renders;
    resize(1000);
    await advance(100);
    resize(960);
    await advance(149);
    expect(getByTestId('u')).toHaveTextContent('1');
    expect(renders).toBe(before);
    await advance(1);
    expect(getByTestId('u')).toHaveTextContent('0.5');
    expect(renders).toBe(before + 1);
  });

  it('stops listening and waiting on unmount', async () => {
    const remove = vi.spyOn(window, 'removeEventListener');
    setWidth(1920);
    const view = render(<Probe />);
    const before = renders;
    resize(960);
    view.unmount();
    expect(remove).toHaveBeenCalledWith('resize', expect.any(Function));
    await advance(200);
    expect(renders).toBe(before);
  });
});
