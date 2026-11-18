import './HostsPage.css';

import { api, seg } from '../api/client';
import type { HostRow, HostsAnswer } from '../api/types';
import { Banner } from '../components/Banner';
import { BusyRegion } from '../components/BusyRegion';
import { Empty } from '../components/Empty';
import { Pager } from '../components/Pager';
import { SearchField } from '../components/SearchField';
import { StaleNote } from '../components/StaleNote';
import { StateText } from '../components/StateText';
import { age, DASH, value } from '../lib/format';
import { usePaging } from '../lib/paging';
import { ONCE, usePolled } from '../lib/poll';
import { useQueryState } from '../lib/query';
import { Link } from '../lib/router';
import { useNow } from '../shell/context';

const LIMIT = 50;
const SEARCH_MAX = 200;
const GROUP_MAX = 63;

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

function rowClass(h: HostRow): string | undefined {
  const classes: string[] = [];
  if (h.state !== null) {
    classes.push('is-problem', `sev-${h.state.severity}`);
    if (h.state.acknowledged) classes.push('is-acknowledged');
  }
  return classes.length === 0 ? undefined : classes.join(' ');
}

function HostTable({ rows, now }: { rows: HostRow[]; now: Date }) {
  return (
    <div class="table-scroll">
      <table class="hosts-table" aria-label="Hosts">
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
              <td class="host-groups">{h.groups.length === 0 ? DASH : h.groups.join(', ')}</td>
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
    </div>
  );
}

export function HostsPage() {
  const [query, setQuery] = useQueryState();
  const now = useNow();

  const search = (query.get('search') ?? '').slice(0, SEARCH_MAX);
  const rawGroup = query.get('group') ?? '';
  // A group name has at most 63 characters; the server refuses a longer one, so none is sent.
  const groupInvalid = rawGroup.length > GROUP_MAX;
  const group = groupInvalid ? '' : rawGroup;
  const withoutGroup = new URLSearchParams(query);
  withoutGroup.delete('group');
  withoutGroup.delete('offset');
  const withoutGroupText = withoutGroup.toString();
  const state = parseState(query.get('state'));
  const paging = usePaging('offset', LIMIT);
  const offset = paging.offset;

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
    // Loaded once per mount; "Try again" and a tab that becomes visible after a failure load it again.
    ONCE,
  );

  const answer = hosts.data;
  // Fewer hosts than the offset: the last page that exists replaces it (once the answer is in).
  const { beyond, shownOffset } = paging.correct(answer?.total, hosts.pending);

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
          label="Search"
          placeholder="Name or address"
          class="hosts-search"
          maxLength={SEARCH_MAX}
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

      {groupInvalid && (
        <Banner kind="warning">
          The group in the address is not valid.{' '}
          <Link href={withoutGroupText === '' ? '/hosts' : '/hosts?' + withoutGroupText}>
            Show all groups
          </Link>
        </Banner>
      )}
      {failed && (
        <Banner kind="warning">
          Could not load the hosts.{' '}
          <button
            type="button"
            class="btn"
            onClick={() => {
              hosts.reload();
              if (groups.data === undefined) groups.reload();
            }}
          >
            Try again
          </button>
        </Banner>
      )}
      {answer !== undefined && (
        // Everything below comes from the last answer: while the answer for a new query is
        // outstanding it is dimmed and marked busy (the empty sentence is not shown at all).
        <BusyRegion pending={hosts.pending} class="hosts-result">
          <StaleNote of={hosts} />
          {!answer.metrics_available && (
            <Banner kind="warning">Current values are not available right now.</Banner>
          )}
          {answer.total === 0 ? (
            !hosts.pending && <Empty>{filtered ? 'No host matches.' : 'No hosts yet.'}</Empty>
          ) : beyond ? null : (
            <>
              {answer.items.length > 0 ? (
                <HostTable rows={answer.items} now={now} />
              ) : (
                !hosts.pending && <Empty>Nothing on this page.</Empty>
              )}
              <p class="hosts-total num">
                {answer.total === 1 ? '1 host' : answer.total + ' hosts'}
              </p>
              <Pager
                total={answer.total}
                limit={LIMIT}
                offset={shownOffset}
                disabled={hosts.pending}
                onChange={paging.set}
              />
            </>
          )}
        </BusyRegion>
      )}
    </>
  );
}
