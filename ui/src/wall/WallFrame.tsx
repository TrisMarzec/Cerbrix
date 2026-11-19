import './wall.css';

import type { ComponentChildren } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import type { SyncIssue, SyncState } from '../api/types';
import { TimeText } from '../components/TimeText';
import { clock } from '../lib/format';
import { Link } from '../lib/router';

/** The cursor and the back link disappear after this long without mouse movement. */
const IDLE_MS = 3_000;

/** Why the display shows the dashboard it shows: an alarm (concept section 6, rule 4). */
export interface WallAlarm {
  /** The name of the dashboard shown because of the alarm. */
  showing: string;
  /** The name of the dashboard the display came from, when it switched. */
  switchedFrom?: string;
  /** How many more dashboards have an alarm. */
  waiting: number;
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
  /** Where "Back to the desk" leads; a display has none. */
  back?: string;
  children: ComponentChildren;
}

function waitingSentence(waiting: number): string | null {
  if (waiting === 1) return '1 more dashboard waiting.';
  if (waiting >= 2) return `${waiting} more dashboards waiting.`;
  return null;
}

function alarmText(alarm: WallAlarm): string {
  const sentences = [`Alarm: showing ${alarm.showing}.`];
  if (alarm.switchedFrom !== undefined) sentences.push(`Switched from ${alarm.switchedFrom}.`);
  const waiting = waitingSentence(alarm.waiting);
  if (waiting !== null) sentences.push(waiting);
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

/** Events that wake the frame: the mouse, a pointer (touch included) and the keyboard. */
const WAKE_EVENTS = ['mousemove', 'pointermove', 'pointerdown', 'keydown'] as const;

/**
 * Whether nobody has used the mouse, a pointer or the keyboard for `IDLE_MS`; the timer runs from
 * mount. A move to the position of the last one is no movement (remote desktops and KVM switches
 * send such moves) and does not wake the frame.
 */
function useIdle(): boolean {
  const [idle, setIdle] = useState(false);
  useEffect(() => {
    let timer = window.setTimeout(() => setIdle(true), IDLE_MS);
    let last: { x: number; y: number } | undefined;
    const wake = (event: Event) => {
      if (event.type === 'mousemove' || event.type === 'pointermove') {
        const { screenX: x, screenY: y } = event as MouseEvent;
        if (last !== undefined && last.x === x && last.y === y) return;
        last = { x, y };
      }
      setIdle(false);
      window.clearTimeout(timer);
      timer = window.setTimeout(() => setIdle(true), IDLE_MS);
    };
    for (const type of WAKE_EVENTS) window.addEventListener(type, wake);
    return () => {
      for (const type of WAKE_EVENTS) window.removeEventListener(type, wake);
      window.clearTimeout(timer);
    };
  }, []);
  return idle;
}

/**
 * The frame of a wall dashboard (concept sections 7 and 8.1): the header with the name, the sync
 * state and the clock, the switch band and the Out Of Sync band, then the content, filling the
 * screen exactly. It takes no input; only "Back to the desk" can be focused, and only with `back`.
 */
export function WallFrame({ title, sync, now, alarm, lostSince, serverError = false, back, children }: Props) {
  const idle = useIdle();
  const nowIso = now.toISOString();
  return (
    <div class={idle ? 'wall is-idle' : 'wall'}>
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
        {alarm !== undefined && (
          <div class="wall-band is-alarm" role="status">
            <p class="wall-band-line">
              <span class="wall-band-text">{alarmText(alarm)}</span>
            </p>
          </div>
        )}
        {sync?.status === 'out_of_sync' && <OutOfSyncBand sync={sync} now={now} />}
        <div class="wall-content">{children}</div>
      </div>
    </div>
  );
}
