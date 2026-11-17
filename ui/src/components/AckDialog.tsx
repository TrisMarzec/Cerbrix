import { useState } from 'preact/hooks';

import { api, seg } from '../api/client';
import type { Problem } from '../api/types';
import { describeActionError } from './actionError';
import { Dialog } from './Dialog';
import { ProblemBrief } from './ProblemBrief';

const COMMENT_MAX = 1000;

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
          setFailure(describeActionError(error));
          setBusy(false);
        },
      );
  };

  return (
    <Dialog title="Acknowledge problem" onClose={onClose}>
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
          <p role="alert" class="dialog-error">
            {failure}
          </p>
        )}
        <div class="dialog-actions">
          <button type="button" class="btn" onClick={onClose}>
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
