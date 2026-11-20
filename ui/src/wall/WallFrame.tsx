import './wall.css';

import type { ComponentChildren } from 'preact';

import type { Severity, SyncIssue, SyncState } from '../api/types';
import { TimeText } from '../components/TimeText';
import { clock, severityWord } from '../lib/format';
import { Link } from '../lib/router';
import { WallRoot } from './WallRoot';

/** Why the display shows the dashboard it shows: an alarm (concept section 6, rule 4). */
export interface WallAlarm {
  /** The name of the dashboard shown because of the alarm. */
  showing: string;
  /** The name of the dashboard the display came from, when it switched. */
  switchedFrom?: string;
  /** How many more dashboards have an alarm. */
  waiting: number;
  /** The most urgent alarm of the dashboard shown, and how many more it has (#99). */
  problem?: { severity: Severity; host: string; summary: string; more: number };
}

interface Props {
  /** The name of the dashboard. */
  title: string;
  /** The sync state of the configuration; without it the frame says nothing about it. */
  sync?: SyncState;
  now: Date;
  alarm?: WallAlarm;
  /** Since when no answer came from the server: the header says so and the content dims. */
  lostSince?: Date;
  /** With `lostSince`: the server answered, but with an error, so the header says that instead. */
  serverError?: boolean;
  /** A display that runs on a login instead of a display token: the header says so (R16). */
  onLogin?: boolean;
  /** Where "Back to the desk" leads; a display has none. */
  back?: string;
  children: ComponentChildren;
}

function waitingSentence(waiting: number): string | null {
  if (waiting === 1) return '1 more dashboard waiting.';
  if (waiting >= 2) return `${waiting} more dashboards waiting.`;
  return null;
}

/** The names of the band: what is shown and where the display came from. */
function alarmText(alarm: WallAlarm): string {
  const sentences = [`Alarm: showing ${alarm.showing}.`];
  if (alarm.switchedFrom !== undefined) sentences.push(`Switched from ${alarm.switchedFrom}.`);
  return sentences.join(' ');
}

/** `file:line:column — reason`; unknown parts are left out. */
function issueText(issue: SyncIssue): string {
  const position = [issue.file, issue.line, issue.column].filter((p) => p !== null).join(':');
  return position === '' ? issue.reason : `${position} — ${issue.reason}`;
}

function SyncText({ sync }: { sync: SyncState | undefined }) {
  switch (sync?.status) {
    case 'in_sync':
      return <span class="wall-sync is-in-sync">In sync</span>;
    case 'out_of_sync':
      return <span class="wall-sync is-out-of-sync">Out of sync</span>;
    case 'never':
      return <span class="wall-sync is-never">Not synchronised yet</span>;
    default:
      return null;
  }
}

/**
 * The switch band (D10) and, on its second line, the most urgent problem (D6): whether or not the
 * dashboard has a problems panel, it says which host has which problem. The names take the
 * ellipsis; the counts after them never shrink.
 */
function AlarmBand({ alarm }: { alarm: WallAlarm }) {
  const waiting = waitingSentence(alarm.waiting);
  const problem = alarm.problem;
  return (
    <div class="wall-band is-alarm" role="status">
      <p class="wall-band-line">
        <span class="wall-band-text">{alarmText(alarm)}</span>
        {waiting !== null && (
          <>
            {' '}
            <span class="wall-band-more">{waiting}</span>
          </>
        )}
      </p>
      {problem !== undefined && (
        <p class="wall-band-line wall-band-detail">
          <span class="wall-band-text">{`${severityWord(problem.severity)}: ${problem.host} – ${problem.summary}`}</span>
          {problem.more > 0 && <span class="wall-band-more">+{problem.more} more</span>}
        </p>
      )}
    </div>
  );
}

function OutOfSyncBand({ sync, now }: { sync: SyncState; now: Date }) {
  const [first, ...others] = sync.issues;
  return (
    <div class="wall-band is-out-of-sync" role="status">
      <p class="wall-band-line">
        <span class="wall-band-text">
          Out of sync
          {sync.last_success_at !== null && (
            <>
              {' since '}
              <TimeText iso={sync.last_success_at} now={now} />
            </>
          )}
          {sync.active_commit !== null
            ? `, showing the configuration from commit ${sync.active_commit.slice(0, 7)}`
            : ', no configuration is active'}
        </span>
      </p>
      {first !== undefined && (
        <p class="wall-band-line wall-band-detail">
          <span class="wall-band-text">{issueText(first)}</span>
          {others.length > 0 && <span class="wall-band-more">and {others.length} more</span>}
        </p>
      )}
    </div>
  );
}

/**
 * The frame of a wall dashboard (concept sections 7 and 8.1): the header with the name, the sync
 * state and the clock, the switch band and the Out Of Sync band, then the content, filling the
 * screen exactly. It takes no input; only "Back to the desk" can be focused, and only with `back`.
 */
export function WallFrame({
  title,
  sync,
  now,
  alarm,
  lostSince,
  serverError = false,
  onLogin = false,
  back,
  children,
}: Props) {
  const nowIso = now.toISOString();
  return (
    <WallRoot>
      <header class="wall-header">
        <h1 class="wall-title" title={title}>
          {title}
        </h1>
        <div class="wall-meta">
          {/* The only live region of the header: the title and the clock are outside it, so the
              clock does not make a screen reader read the header every minute. */}
          <span class="wall-state" role="status">
            {lostSince !== undefined ? (
              <span class="wall-sync is-lost">
                {serverError ? 'The server reported an error since ' : 'No connection to the server since '}
                <TimeText iso={lostSince.toISOString()} now={now} />
              </span>
            ) : (
              <SyncText sync={sync} />
            )}
            {onLogin && <span class="wall-login">Running on your login, not on a display token</span>}
          </span>
          <time class="wall-clock num" dateTime={nowIso}>
            {clock(nowIso, now)}
          </time>
          {back !== undefined && (
            <Link href={back} class="wall-back">
              Back to the desk
            </Link>
          )}
        </div>
      </header>
      {/* Without connection the bands dim with the content: they are as old as it is. */}
      <div class={lostSince !== undefined ? 'wall-body is-lost' : 'wall-body'}>
        {alarm !== undefined && <AlarmBand alarm={alarm} />}
        {sync?.status === 'out_of_sync' && <OutOfSyncBand sync={sync} now={now} />}
        <div class="wall-content">{children}</div>
      </div>
    </WallRoot>
  );
}
