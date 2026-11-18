import { useEffect, useRef, useState } from 'preact/hooks';

import { Dialog } from './Dialog';

interface Props {
  title: string;
  secret: string;
  note: string;
  onClose: () => void;
}

type CopyState = 'idle' | 'copied' | 'manual';

/** Shows a secret once. It lives only in the props of this component: nothing is stored. */
export function SecretDialog({ title, secret, note, onClose }: Props) {
  const field = useRef<HTMLInputElement>(null);
  const [copy, setCopy] = useState<CopyState>('idle');

  // Set as an attribute: a false property would drop it instead of writing spellcheck="false".
  useEffect(() => {
    field.current?.setAttribute('spellcheck', 'false');
  }, []);

  async function onCopy() {
    try {
      if (navigator.clipboard === undefined) throw new Error('no clipboard');
      await navigator.clipboard.writeText(secret);
      setCopy('copied');
    } catch {
      field.current?.focus();
      field.current?.select();
      setCopy('manual');
    }
  }

  return (
    <Dialog title={title} onClose={onClose} closeOnBackdrop={false}>
      <p>{note}</p>
      <p class="dim">This is shown once.</p>
      <input
        ref={field}
        class="secret-field"
        type="text"
        readOnly
        autoComplete="off"
        value={secret}
        aria-label={title}
        onFocus={(e) => e.currentTarget.select()}
      />
      <div class="dialog-actions">
        <button type="button" class="btn" onClick={() => void onCopy()}>
          Copy
        </button>
        <span class="dim" role="status">
          {copy === 'copied' ? 'Copied' : copy === 'manual' ? 'Press Ctrl+C to copy' : ''}
        </span>
        <button type="button" class="btn" onClick={onClose}>
          Close
        </button>
      </div>
    </Dialog>
  );
}
