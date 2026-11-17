import { clock } from '../lib/format';

export function TimeText({ iso, now }: { iso: string; now: Date }) {
  return (
    <time class="num" dateTime={iso} title={iso}>
      {clock(iso, now)}
    </time>
  );
}
