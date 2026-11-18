import { render, screen } from '@testing-library/preact';
import userEvent from '@testing-library/user-event';
import { useState } from 'preact/hooks';

import { SecretDialog } from './SecretDialog';

function Host() {
  const [open, setOpen] = useState(true);
  return open ? (
    <SecretDialog
      title="Enrolment key"
      secret="s3cr3t-value"
      note="Use it once."
      onClose={() => setOpen(false)}
    />
  ) : (
    <p>closed</p>
  );
}

function setClipboard(value: unknown) {
  Object.defineProperty(navigator, 'clipboard', { value, configurable: true });
}

describe('SecretDialog', () => {
  const original = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
  afterEach(() => {
    if (original === undefined) {
      Reflect.deleteProperty(navigator, 'clipboard');
    } else {
      Object.defineProperty(navigator, 'clipboard', original);
    }
  });

  it('is not closed by a click on the backdrop but by Escape', async () => {
    const user = userEvent.setup();
    const { container } = render(<Host />);
    const backdrop = container.querySelector('.dialog-backdrop');
    await user.click(backdrop as Element);
    expect(screen.getByDisplayValue('s3cr3t-value')).toBeInTheDocument();
    await user.keyboard('{Escape}');
    expect(screen.getByText('closed')).toBeInTheDocument();
  });

  it('switches off autocomplete and spellcheck', () => {
    render(<Host />);
    const field = screen.getByDisplayValue('s3cr3t-value');
    expect(field).toHaveAttribute('autocomplete', 'off');
    expect(field).toHaveAttribute('spellcheck', 'false');
  });

  it('shows the secret in a read-only field with the note', () => {
    render(<Host />);
    const field = screen.getByDisplayValue('s3cr3t-value');
    expect(field).toHaveAttribute('readonly');
    expect(screen.getByText('This is shown once.')).toBeInTheDocument();
    expect(screen.getByText('Use it once.')).toBeInTheDocument();
  });

  it('copies to the clipboard and says so', async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    setClipboard({ writeText });
    render(<Host />);
    await user.click(screen.getByRole('button', { name: 'Copy' }));
    expect(writeText).toHaveBeenCalledWith('s3cr3t-value');
    expect(await screen.findByText('Copied')).toBeInTheDocument();
  });

  it('falls back to selecting the field when the clipboard refuses', async () => {
    const user = userEvent.setup();
    setClipboard({ writeText: vi.fn().mockRejectedValue(new Error('denied')) });
    render(<Host />);
    await user.click(screen.getByRole('button', { name: 'Copy' }));
    expect(await screen.findByText('Press Ctrl+C to copy')).toBeInTheDocument();
    expect(screen.getByDisplayValue('s3cr3t-value')).toHaveFocus();
  });

  it('falls back when there is no clipboard', async () => {
    const user = userEvent.setup();
    setClipboard(undefined);
    render(<Host />);
    await user.click(screen.getByRole('button', { name: 'Copy' }));
    expect(await screen.findByText('Press Ctrl+C to copy')).toBeInTheDocument();
  });

  it('is gone from the DOM after close', async () => {
    const user = userEvent.setup();
    const { container } = render(<Host />);
    await user.click(screen.getByRole('button', { name: 'Close' }));
    expect(screen.getByText('closed')).toBeInTheDocument();
    expect(screen.queryByDisplayValue('s3cr3t-value')).toBeNull();
    expect(container.textContent).not.toContain('s3cr3t-value');
  });
});
