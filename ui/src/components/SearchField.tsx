import { useEffect, useRef, useState } from 'preact/hooks';

const DELAY_MS = 300;

interface Props {
  /** The search text of the URL. */
  urlValue: string;
  onCommit: (text: string) => void;
  label: string;
  placeholder: string;
  /** A class of the page for its own layout. */
  class?: string;
  maxLength: number;
}

/**
 * A search field that follows the URL and writes to it 300 ms after the last key stroke, or at once
 * on Enter. Text equal to the URL's changes nothing; "Clear search" returns the focus to the field.
 */
export function SearchField({ urlValue, onCommit, label, placeholder, class: className, maxLength }: Props) {
  const [text, setText] = useState(urlValue);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const cancel = () => {
    clearTimeout(timer.current);
    timer.current = undefined;
  };

  // A change of the URL (the back button, or our own commit) replaces what is typed.
  useEffect(() => {
    cancel();
    setText(urlValue);
  }, [urlValue]);
  useEffect(() => cancel, []);

  const current = useRef(urlValue);
  current.current = urlValue;
  const input = useRef<HTMLInputElement>(null);

  // Text equal to what the URL already has changes nothing: no write, no offset reset, no request.
  const commit = (next: string) => {
    cancel();
    if (next !== current.current) onCommit(next);
  };

  return (
    <form
      class={className === undefined ? 'search-field' : `search-field ${className}`}
      role="search"
      onSubmit={(e) => {
        e.preventDefault();
        commit(text);
      }}
    >
      <label>
        {label}
        <input
          ref={input}
          type="search"
          value={text}
          maxLength={maxLength}
          placeholder={placeholder}
          onInput={(e) => {
            const next = e.currentTarget.value;
            setText(next);
            cancel();
            timer.current = setTimeout(() => {
              commit(next);
            }, DELAY_MS);
          }}
        />
      </label>
      {text !== '' && (
        <button
          type="button"
          class="btn"
          onClick={() => {
            setText('');
            commit('');
            input.current?.focus();
          }}
        >
          Clear search
        </button>
      )}
    </form>
  );
}
