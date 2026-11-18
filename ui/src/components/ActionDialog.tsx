import type { ComponentChildren } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';

import { describeError, type ErrorTexts } from './describeError';
import { Dialog } from './Dialog';

interface Props<T> {
  title: string;
  submitLabel: string;
  submitDisabled?: boolean;
  /** The question or the form fields. */
  children: ComponentChildren;
  /** A sentence when the input is not acceptable; no request is sent then. */
  validate?: () => string | undefined;
  run: () => Promise<T>;
  /** Sentences for failures this dialog knows, by status (see `describeError`). */
  texts?: ErrorTexts;
  /** Called with every failure, for what it means beyond the sentence (a user that is gone). */
  onFailed?: (e: unknown) => void;
  /** Called after every attempt, for example to empty password fields. */
  afterAttempt?: () => void;
  onDone: (result: T) => void;
  onClose: () => void;
}

/**
 * Asks first, runs the request once, cannot be dismissed while it runs, and stays open with the
 * failure written out inside.
 */
export function ActionDialog<T>({
  title,
  submitLabel,
  submitDisabled = false,
  children,
  validate,
  run,
  texts,
  onFailed,
  afterAttempt,
  onDone,
  onClose,
}: Props<T>) {
  const mounted = useRef(true);
  useEffect(
    () => () => {
      mounted.current = false;
    },
    [],
  );
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const close = () => {
    if (!busy) onClose();
  };

  const submit = (e: Event) => {
    e.preventDefault();
    if (busy) return;
    const problem = validate?.();
    if (problem !== undefined) {
      setFailure(problem);
      return;
    }
    setBusy(true);
    setFailure(undefined);
    run().then(
      (result) => {
        if (!mounted.current) return;
        afterAttempt?.();
        onDone(result);
      },
      (error: unknown) => {
        if (!mounted.current) return;
        afterAttempt?.();
        onFailed?.(error);
        // onFailed may have closed the dialog.
        if (!mounted.current) return;
        setFailure(describeError(error, texts));
        setBusy(false);
      },
    );
  };

  return (
    <Dialog title={title} closeOnBackdrop={!busy} onClose={close}>
      <form class="dialog-form" onSubmit={submit} noValidate>
        {children}
        {failure !== undefined && (
          <p role="alert" class="form-error">
            {failure}
          </p>
        )}
        <div class="dialog-actions">
          <button type="button" class="btn" disabled={busy} onClick={close}>
            Cancel
          </button>
          <button type="submit" class="btn" disabled={busy || submitDisabled}>
            {submitLabel}
          </button>
        </div>
      </form>
    </Dialog>
  );
}
