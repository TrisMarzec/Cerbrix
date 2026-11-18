import type { SyncIssue, SyncState } from '../api/types';
import { Banner } from '../components/Banner';
import { TimeText } from '../components/TimeText';
import { age } from '../lib/format';
import { Link } from '../lib/router';
import { useNow, useStatus } from './context';

function shortCommit(commit: string | null): string | undefined {
  return commit === null ? undefined : commit.slice(0, 7);
}

function place(issue: SyncIssue): string {
  if (issue.file === null) return '';
  let text = issue.file;
  if (issue.line !== null) {
    text += `:${issue.line}`;
    if (issue.column !== null) text += `:${issue.column}`;
  }
  return text;
}

function issueText(issue: SyncIssue): string {
  const where = place(issue);
  return where === '' ? issue.reason : `${where} — ${issue.reason}`;
}

function IssueLine({ sync }: { sync: SyncState }) {
  const first = sync.issues[0];
  const more = Math.max(0, sync.issues.length - 1);
  return (
    <p>
      {first !== undefined && <span>{issueText(first)} </span>}
      <Link href="/configuration">{more > 0 ? `and ${more} more` : 'Details'}</Link>
    </p>
  );
}

/** The bar on top of every page, and the banners under it that say the data cannot be trusted. */
export function TopBar() {
  const { status, staleSince, error } = useStatus();
  const now = useNow();
  const sync = status?.gitops === true ? status.sync : undefined;
  const commit = shortCommit(sync?.active_commit ?? null);

  let left: string | undefined;
  let warn = false;
  if (status?.gitops === false) {
    left = 'Configuration is managed on the server';
  } else if (sync?.status === 'never') {
    left = 'The configuration has not been synchronised yet';
    warn = true;
  } else if (sync !== undefined) {
    const parts = [
      sync.status === 'in_sync' ? 'In sync' : 'Out of sync',
      commit,
      sync.last_success_at === null ? undefined : age(sync.last_success_at, now),
    ];
    left = parts.filter((part) => part !== undefined).join(' · ');
    warn = sync.status === 'out_of_sync';
  }

  return (
    <>
      <header class="bar">
        {/* Without GitOps there is no sync state: the sentence is written in the plain bar colour. */}
        <span class={status?.gitops === false ? undefined : warn ? 'sync is-warning' : 'sync'}>{left}</span>
        {status?.gitops === true && (
          <span class="git-note">Hosts, templates and dashboards are managed by Git</span>
        )}
      </header>
      {sync?.status === 'out_of_sync' && (
        <Banner kind="warning">
          <p>
            {sync.last_success_at === null ? (
              'Out of sync'
            ) : (
              <>
                Out of sync since <TimeText iso={sync.last_success_at} now={now} />
              </>
            )}
            {commit === undefined
              ? ', no configuration is active'
              : `, showing the configuration from commit ${commit}`}
          </p>
          <IssueLine sync={sync} />
        </Banner>
      )}
      {sync?.status === 'never' && sync.issues.length > 0 && (
        <Banner kind="warning">
          <IssueLine sync={sync} />
        </Banner>
      )}
      {staleSince !== undefined && (
        <Banner kind="warning">
          {error === undefined || error.code === 'network'
            ? 'No connection to the server since '
            : 'The server reported an error since '}
          <TimeText iso={staleSince.toISOString()} now={now} />
        </Banner>
      )}
    </>
  );
}
