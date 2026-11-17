import { useNow, useStatus } from '../shell/context';
import { TimeText } from './TimeText';

interface Props {
  /** The polled data of the region below: `usePolled`'s answer. */
  of: { data: unknown; staleSince: Date | undefined };
}

/**
 * "Not updated since 14:20" above a region whose own poll fails while the status poll of the shell
 * works. When the shell's banner already says that the server cannot be reached, it says nothing;
 * without data the page shows its failure banner instead.
 */
export function StaleNote({ of }: Props) {
  const shell = useStatus();
  const now = useNow();
  if (of.data === undefined || of.staleSince === undefined || shell.staleSince !== undefined) return null;
  return (
    <p class="stale-note">
      Not updated since <TimeText iso={of.staleSince.toISOString()} now={now} />
    </p>
  );
}
