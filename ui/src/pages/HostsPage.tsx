import './HostsPage.css';

import { useEffect, useRef, useState } from 'preact/hooks';

import { api, seg } from '../api/client';
import type { HostRow, HostsAnswer } from '../api/types';
import { Banner } from '../components/Banner';
import { Empty } from '../components/Empty';
import { Pager } from '../components/Pager';
import { StateText } from '../components/StateText';
import { age, value } from '../lib/format';
import { usePolled } from '../lib/poll';
import { useQueryState } from '../lib/query';
import { Link } from '../lib/router';
import { useNow } from '../shell/context';

const LIMIT = 50;
const SEARCH_MAX = 200;
const GROUP_MAX = 63;
const SEARCH_DELAY_MS = 300;

type StateFilter = 'problems' | 'no_data' | 'ok';

const STATE_OPTIONS: { value: StateFilter | ''; label: string }[] = [
  { value: '', label: 'All' },
  { value: 'problems', label: 'With problems' },
  { value: 'no_data', label: 'No data' },
  { value: 'ok', label: 'Without problems' },
];

function parseState(text: string | null): StateFilter | undefined {
  return text === 'problems' || text === 'no_data' || text === 'ok' ? text : undefined;
}

function parseOffset(text: string | null): number {
  if (text === null || !/^\d+$/.test(text)) return 0;
  const n = Number(text);
  return Number.isSafeInteger(n) ? n : 0;
}

/** Start of the last page that exists for `total` > 0 hosts. */
function lastPageStart(total: number): number {
  return Math.floor((total - 1) / LIMIT) * LIMIT;
}

function offsetParam(offset: number): string | undefined {
  return offset === 0 ? undefined : String(offset);
}

interface SearchProps {
  /** The search text of the URL. */
  urlValue: string;
  onCommit: (text: string) => void;
}

/** The search field: follows the URL, writes to it 300 ms after the last key stroke or at once on Enter. */
function SearchField({ urlValue, onCommit }: SearchProps) {
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

  const commit = (next: string) => {
    cancel();
    onCommit(next);
  };

  return (
    <form
      class="hosts-search"
      role="search"
      onSubmit={(e) => {
        e.preventDefault();
        commit(text);
      }}
    >
      <label>
        Search
        <input
          type="search"
          value={text}
          maxLength={SEARCH_MAX}
          placeholder="Name or address"
          onInput={(e) => {
            const next = e.currentTarget.value;
            setText(next);
            cancel();
            timer.current = setTimeout(() => {
              timer.current = undefined;
              onCommit(next);
            }, SEARCH_DELAY_MS);
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
          }}
        >
          Clear search
        </button>
      )}
    </form>
  );
}

function rowClass(h: HostRow): string | undefined {
  const classes: string[] = [];
  if (h.state !== null) {
    classes.push('is-problem', `sev-${h.state.severity}`);
    if (h.state.acknowledged) classes.push('is-acknowledged');
  }
  return classes.length === 0 ? undefined : classes.join(' ');
}

function HostTable({ rows, busy, now }: { rows: HostRow[]; busy: boolean; now: Date }) {
  return (
    <table class={`hosts-table${busy ? ' is-busy' : ''}`} aria-label="Hosts" aria-busy={busy}>
      <thead>
        <tr>
          <th scope="col">
            <span class="visually-hidden">Severity</span>
          </th>
          <th scope="col">Host</th>
          <th scope="col">Groups</th>
          <th scope="col">State</th>
          <th scope="col" class="right">
            CPU
          </th>
          <th scope="col" class="right">
            Memory
          </th>
          <th scope="col" class="right">
            Last value
          </th>
        </tr>
      </thead>
      <tbody>
        {rows.map((h) => (
          <tr key={h.name} class={rowClass(h)}>
            <td class="cell-sev" aria-hidden="true" />
            <td>
              <Link class="host-name" href={`/hosts/${seg(h.name)}`}>
                {h.name}
              </Link>
              {h.address !== null && <small class="host-address">{h.address}</small>}
            </td>
            <td class="host-groups">{h.groups.length === 0 ? '–' : h.groups.join(', ')}</td>
            <td>
              <StateText state={h.state} />
            </td>
            <td class="right num">{value('cpu.usage', h.cpu)}</td>
            <td class="right num">{value('memory.used_percent', h.memory)}</td>
            <td class="right num dim">{age(h.last_seen_at, now)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function HostsPage() {
  const [query, setQuery] = useQueryState();
  const now = useNow();

  const search = (query.get('search') ?? '').slice(0, SEARCH_MAX);
  const rawGroup = query.get('group') ?? '';
  // A group name has at most 63 characters; a longer one cannot exist and is ignored.
  const group = rawGroup.length > GROUP_MAX ? '' : rawGroup;
  const state = parseState(query.get('state'));
  const offset = parseOffset(query.get('offset'));

  const hosts = usePolled(
    (signal) =>
      api.get<HostsAnswer>(
        '/hosts',
        {
          search: search === '' ? undefined : search,
          group: group === '' ? undefined : group,
          state,
          limit: LIMIT,
          offset,
        },
        signal,
      ),
    [search, group, state, offset],
  );
  const groups = usePolled(
    (signal) => api.get<{ items: string[] }>('/host-groups', undefined, signal),
    [],
  );

  const answer = hosts.data;
  const total = answer?.total ?? 0;
  const beyond = answer !== undefined && total > 0 && total <= offset;

  // Fewer hosts than the offset: go back to the last page that exists (once; the next answer is in range).
  useEffect(() => {
    if (!hosts.pending && beyond) setQuery({ offset: offsetParam(lastPageStart(total)) });
  }, [hosts.pending, beyond, total]);

  const failed = answer === undefined && hosts.error !== undefined;
  const filtered = search !== '' || group !== '' || state !== undefined;

  const groupNames = groups.data?.items ?? [];
  const groupOptions =
    group !== '' && !groupNames.includes(group) ? [...groupNames, group] : groupNames;

  return (
    <>
      <h1>Hosts</h1>
      <div class="hosts-tools">
        <SearchField
          urlValue={search}
          onCommit={(text) => setQuery({ search: text === '' ? undefined : text })}
        />
        <label>
          Group
          <select
            value={group}
            onChange={(e) => setQuery({ group: e.currentTarget.value || undefined })}
          >
            <option value="">All groups</option>
            {groupOptions.map((g) => (
              <option key={g} value={g}>
                {g}
              </option>
            ))}
          </select>
        </label>
        <label>
          State
          <select
            value={state ?? ''}
            onChange={(e) => setQuery({ state: e.currentTarget.value || undefined })}
          >
            {STATE_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
      </div>

      {failed && (
        <Banner kind="warning">
          Could not load the hosts.{' '}
          <button type="button" class="btn" onClick={hosts.reload}>
            Try again
          </button>
        </Banner>
      )}
      {answer !== undefined && (
        <>
          {!answer.metrics_available && (
            <Banner kind="warning">Current values are not available right now.</Banner>
          )}
          {answer.total === 0 ? (
            <Empty>{filtered ? 'No host matches.' : 'No hosts yet.'}</Empty>
          ) : (
            answer.items.length > 0 && (
              <>
                <HostTable rows={answer.items} busy={hosts.pending} now={now} />
                <p class="hosts-total num">
                  {answer.total === 1 ? '1 host' : `${answer.total} hosts`}
                </p>
                <Pager
                  total={answer.total}
                  limit={LIMIT}
                  offset={offset}
                  onChange={(next) => setQuery({ offset: offsetParam(next) })}
                />
              </>
            )
          )}
        </>
      )}
    </>
  );
}
