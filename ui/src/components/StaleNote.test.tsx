import { render, screen } from '@testing-library/preact';

import { NowContext, StatusContext } from '../shell/context';
import { NOW, statusFixture } from '../test/fixtures';
import { StaleNote } from './StaleNote';

const SINCE = new Date(2026, 10, 17, 14, 20, 0);

function show(of: { data: unknown; staleSince: Date | undefined }, shellStale?: Date) {
  return render(
    <StatusContext.Provider value={{ status: statusFixture(), staleSince: shellStale, reloadStatus: () => {} }}>
      <NowContext.Provider value={NOW}>
        <StaleNote of={of} />
      </NowContext.Provider>
    </StatusContext.Provider>,
  );
}

describe('StaleNote', () => {
  it('says since when the data of the page is old', () => {
    show({ data: ['x'], staleSince: SINCE });
    const note = screen.getByText(/^Not updated since/);
    expect(note).toHaveTextContent('Not updated since 14:20');
    expect(note.querySelector('time')).toHaveAttribute('datetime', SINCE.toISOString());
  });

  it('is absent while the poll works', () => {
    const { container } = show({ data: ['x'], staleSince: undefined });
    expect(container).toBeEmptyDOMElement();
  });

  it('is absent when the shell already says that the server cannot be reached', () => {
    const { container } = show({ data: ['x'], staleSince: SINCE }, new Date(2026, 10, 17, 14, 19, 0));
    expect(container).toBeEmptyDOMElement();
  });

  it('is absent without data: the page shows its failure instead', () => {
    const { container } = show({ data: undefined, staleSince: SINCE });
    expect(container).toBeEmptyDOMElement();
  });
});
