import { useState } from 'preact/hooks';

import { api, seg } from '../api/client';
import type { Problem } from '../api/types';
import { describeError } from './describeError';
import { Dialog } from './Dialog';
import { ProblemBrief } from './ProblemBrief';

const COMMENT_MAX = 1000;

/** Both actions answer 409 for a problem that has been resolved meanwhile. */
const ALREADY_RESOLVED = { 409: 'This problem is already resolved.' };

interface Props {
  problem: Problem;
  onDone: (p: Problem) => void;
  onClose: () => void;
}

/** Acknowledges an open problem; the comment is optional. */
export function AckDialog({ problem, onDone, onClose }: Props) {
  const [comment, setComment] = useState('');
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | undefined>(undefined);

  const submit = (e: Event) => {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setFailure(undefined);
    const text = comment.trim();
    api
      .send<Problem>(
        'POST',
        `/problems/${seg(problem.id)}/ack`,
        text === '' ? {} : { comment: text },
      )
      .then(
        (answer) => {
          onDone(answer);
        },
        (error: unknown) => {
          setFailure(describeError(error, ALREADY_RESOLVED));
          setBusy(false);
        },
      );
  };

  // While the request runs the dialog cannot be dismissed, so a failure is never dropped unseen.
  return (
    <Dialog
      title="Acknowledge problem"
      closeOnBackdrop={!busy}
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <ProblemBrief problem={problem} />
      <form class="dialog-form" onSubmit={submit}>
        <label>
          Comment (optional)
          <textarea
            rows={3}
            maxLength={COMMENT_MAX}
            value={comment}
            onInput={(e) => {
              setComment(e.currentTarget.value);
            }}
          />
        </label>
        {failure !== undefined && (
          <p role="alert" class="form-error">
            {failure}
          </p>
        )}
        <div class="dialog-actions">
          <button type="button" class="btn" disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button type="submit" class="btn" disabled={busy}>
            Acknowledge
          </button>
        </div>
      </form>
    </Dialog>
  );
}
