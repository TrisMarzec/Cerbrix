import { render, screen, waitFor } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';
import { useState } from 'preact/hooks';

import { Dialog } from './Dialog';

function Host({ onClose }: { onClose?: () => void }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open
      </button>
      {open && (
        <Dialog
          title="Acknowledge"
          onClose={() => {
            onClose?.();
            setOpen(false);
          }}
        >
          <input aria-label="Comment" />
          <button type="button">Save</button>
        </Dialog>
      )}
    </>
  );
}

function Nested({ innerButtons = 1 }: { innerButtons?: number }) {
  const [confirm, setConfirm] = useState(false);
  return (
    <Dialog title="Outer" onClose={() => {}}>
      <button type="button" onClick={() => setConfirm(true)}>
        Delete
      </button>
      <button type="button">Other</button>
      {confirm && (
        <Dialog title="Confirm" onClose={() => setConfirm(false)}>
          <button type="button">Yes</button>
          {innerButtons > 1 && <button type="button">No</button>}
        </Dialog>
      )}
    </Dialog>
  );
}

describe('nested Dialog', () => {
  it('closes only the inner dialog on Escape and returns focus to its opener', async () => {
    const user = userEvent.setup();
    render(<Nested />);
    await user.click(screen.getByRole('button', { name: 'Delete' }));
    expect(screen.getAllByRole('dialog')).toHaveLength(2);
    await user.keyboard('{Escape}');
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
    expect(screen.getByRole('dialog', { name: 'Outer' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Delete' })).toHaveFocus();
  });

  it('keeps Tab and Shift+Tab in an inner dialog with one focusable element', async () => {
    const user = userEvent.setup();
    render(<Nested />);
    await user.click(screen.getByRole('button', { name: 'Delete' }));
    const yes = screen.getByRole('button', { name: 'Yes' });
    expect(yes).toHaveFocus();
    await user.tab();
    expect(yes).toHaveFocus();
    await user.tab({ shift: true });
    expect(yes).toHaveFocus();
  });

  it('cycles inside an inner dialog with two focusable elements', async () => {
    const user = userEvent.setup();
    render(<Nested innerButtons={2} />);
    await user.click(screen.getByRole('button', { name: 'Delete' }));
    await user.tab();
    expect(screen.getByRole('button', { name: 'No' })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole('button', { name: 'Yes' })).toHaveFocus();
    await user.tab({ shift: true });
    expect(screen.getByRole('button', { name: 'No' })).toHaveFocus();
  });

  it('lets the outer dialog work again after the inner one closed', async () => {
    const user = userEvent.setup();
    render(<Nested />);
    await user.click(screen.getByRole('button', { name: 'Delete' }));
    await user.keyboard('{Escape}');
    await user.tab();
    expect(screen.getByRole('button', { name: 'Other' })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole('button', { name: 'Delete' })).toHaveFocus();
  });
});

describe('Dialog focus', () => {
  it('keeps focus inside when the focused element disappears', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    function Form() {
      const [sent, setSent] = useState(false);
      return (
        <Dialog title="Form" onClose={onClose}>
          {!sent && (
            <button type="button" onClick={() => setSent(true)}>
              Send
            </button>
          )}
          <button type="button">Close</button>
        </Dialog>
      );
    }
    render(<Form />);
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalledTimes(1);
    await user.tab();
    expect(screen.getByRole('dialog')).toContainElement(document.activeElement as HTMLElement);
  });

  it('focuses main when the opener is gone at close', async () => {
    const main = document.createElement('main');
    document.body.append(main);
    const opener = document.createElement('button');
    main.append(opener);
    opener.focus();
    const { unmount } = render(
      <Dialog title="T" onClose={() => {}}>
        <button type="button">In</button>
      </Dialog>,
    );
    opener.remove();
    unmount();
    await waitFor(() => expect(main).toHaveFocus());
    main.remove();
  });
});

describe('Dialog mounted together with a nested one', () => {
  function Both({ outer = true, inner = true }: { outer?: boolean; inner?: boolean }) {
    return (
      <>
        {outer && (
          <Dialog title="Outer" onClose={() => {}}>
            <button type="button">Outer button</button>
            {inner && (
              <Dialog title="Inner" onClose={() => {}}>
                <button type="button">Inner button</button>
              </Dialog>
            )}
          </Dialog>
        )}
      </>
    );
  }

  it('puts the nested dialog on top: it has the focus and handles Escape', async () => {
    const onOuter = vi.fn();
    const onInner = vi.fn();
    const user = userEvent.setup();
    render(
      <Dialog title="Outer" onClose={onOuter}>
        <button type="button">Outer button</button>
        <Dialog title="Inner" onClose={onInner}>
          <button type="button">Inner button</button>
        </Dialog>
      </Dialog>,
    );
    expect(screen.getByRole('button', { name: 'Inner button' })).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(onInner).toHaveBeenCalledTimes(1);
    expect(onOuter).not.toHaveBeenCalled();
    await user.tab();
    expect(screen.getByRole('button', { name: 'Inner button' })).toHaveFocus();
  });

  it('leaves the focus on main, not on the body, when the outer one goes with the inner one open', async () => {
    const main = document.createElement('main');
    document.body.append(main);
    const view = render(<Both />, { container: main });
    expect(screen.getByRole('button', { name: 'Inner button' })).toHaveFocus();
    view.rerender(<Both outer={false} />);
    await waitFor(() => expect(main).toHaveFocus());
    expect(document.activeElement).not.toBe(document.body);
    main.remove();
  });

  it('gives the focus back to the opener of the outer one when it is still there', async () => {
    const main = document.createElement('main');
    document.body.append(main);
    const opener = document.createElement('button');
    opener.textContent = 'Opener';
    document.body.append(opener);
    opener.focus();
    const view = render(<Both />, { container: main });
    view.rerender(<Both outer={false} />);
    await waitFor(() => expect(opener).toHaveFocus());
    opener.remove();
    main.remove();
  });
});

describe('Dialog', () => {
  it('is modal and labelled by its title', async () => {
    const user = userEvent.setup();
    render(<Host />);
    await user.click(screen.getByRole('button', { name: 'Open' }));
    const dialog = screen.getByRole('dialog', { name: 'Acknowledge' });
    expect(dialog).toHaveAttribute('aria-modal', 'true');
  });

  it('puts focus on the first focusable element', async () => {
    const user = userEvent.setup();
    render(<Host />);
    await user.click(screen.getByRole('button', { name: 'Open' }));
    expect(screen.getByLabelText('Comment')).toHaveFocus();
  });

  it('keeps Tab and Shift+Tab inside', async () => {
    const user = userEvent.setup();
    render(<Host />);
    await user.click(screen.getByRole('button', { name: 'Open' }));
    await user.tab();
    expect(screen.getByRole('button', { name: 'Save' })).toHaveFocus();
    await user.tab();
    expect(screen.getByLabelText('Comment')).toHaveFocus();
    await user.tab({ shift: true });
    expect(screen.getByRole('button', { name: 'Save' })).toHaveFocus();
  });

  it('closes on Escape and returns focus to the opener', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<Host onClose={onClose} />);
    const opener = screen.getByRole('button', { name: 'Open' });
    await user.click(opener);
    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(opener).toHaveFocus();
  });

  it('closes on a click on the backdrop but not inside', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const { container } = render(
      <Dialog title="T" onClose={onClose}>
        <button type="button">Inside</button>
      </Dialog>,
    );
    await user.click(screen.getByRole('button', { name: 'Inside' }));
    expect(onClose).not.toHaveBeenCalled();
    const backdrop = container.querySelector('.dialog-backdrop');
    expect(backdrop).not.toBeNull();
    await user.click(backdrop as Element);
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
