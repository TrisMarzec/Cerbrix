import { act, render } from '@testing-library/preact';
import { vi } from 'vitest';

import { useWallUnit } from './unit';

function Probe() {
  return <span data-testid="u">{useWallUnit()}</span>;
}

const original = Object.getOwnPropertyDescriptor(window, 'innerWidth');

function setWidth(width: number) {
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: width });
}

afterEach(() => {
  if (original === undefined) Reflect.deleteProperty(window, 'innerWidth');
  else Object.defineProperty(window, 'innerWidth', original);
  vi.restoreAllMocks();
});

describe('useWallUnit', () => {
  it('is a 1920th of the window width and follows a resize', () => {
    setWidth(3840);
    const { getByTestId } = render(<Probe />);
    expect(getByTestId('u')).toHaveTextContent('2');
    setWidth(960);
    act(() => {
      window.dispatchEvent(new Event('resize'));
    });
    expect(getByTestId('u')).toHaveTextContent('0.5');
  });

  it('stops listening on unmount', () => {
    const remove = vi.spyOn(window, 'removeEventListener');
    const view = render(<Probe />);
    view.unmount();
    expect(remove).toHaveBeenCalledWith('resize', expect.any(Function));
  });
});
