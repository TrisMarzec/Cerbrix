import { useState } from 'preact/hooks';

import { api, seg } from '../api/client';
import type { Problem } from '../api/types';
import { describeActionError } from './actionError';
import { Dialog } from './Dialog';
import { ProblemBrief } from './ProblemBrief';

const REASON_MAX = 1000;

interface Props {
  problem: Problem;
  onDone: (p: Problem) => void;
  onClose: () => void;
}

/** Closes an open problem by hand; the reason is required. */
export function CloseDialog({ problem, onDone, onClose }: Props) {
  const [reason, setReason] = useState('');
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const submit = (e: Event) => {
    e.preventDefault();
    if (busy) return;
    const text = reason.trim();
    if (text === '') {
      setFailure('A reason is required.');
      return;
    }
    setBusy(true);
    setFailure(undefined);
    api.send<Problem>('POST', `/problems/${seg(problem.id)}/close`, { comment: text }).then(
      (answer) => {
        onDone(answer);
      },
      (error: unknown) => {
        setFailure(describeActionError(error));
        setBusy(false);
      },
    );
  };

  return (
    <Dialog title="Close problem" onClose={onClose}>
      <ProblemBrief problem={problem} />
      <p>
        Closing ends this problem by hand. Use it when the thing it is about no longer exists, for
        example a removed disk. If the condition still holds, a new problem opens with the next
        value.
      </p>
      <form class="dialog-form" noValidate onSubmit={submit}>
        <label>
          Reason
          <textarea
            rows={3}
            maxLength={REASON_MAX}
            required
            value={reason}
            onInput={(e) => {
              setReason(e.currentTarget.value);
            }}
          />
        </label>
        {failure !== undefined && (
          <p role="alert" class="dialog-error">
            {failure}
          </p>
        )}
        <div class="dialog-actions">
          <button type="button" class="btn" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" class="btn" disabled={busy}>
            Close problem
          </button>
        </div>
      </form>
    </Dialog>
  );
}
