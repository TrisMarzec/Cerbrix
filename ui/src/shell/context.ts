import { createContext } from 'preact';
import { useContext } from 'preact/hooks';

import type { Status } from '../api/types';

export interface StatusValue {
  /** The last `GET /status` that succeeded; undefined until the first answer. */
  status: Status | undefined;
  /** Since when the status poll fails; undefined while it works. */
  staleSince: Date | undefined;
}

export const StatusContext = createContext<StatusValue>({ status: undefined, staleSince: undefined });

/** The one polled `GET /status` of the shell, for pages that need `gitops` or the sync state. */
export function useStatus(): StatusValue {
  return useContext(StatusContext);
}

export const NowContext = createContext<Date>(new Date());

/** The shell's clock, updated every 10 seconds: one clock for all pages, and tests can fix it. */
export function useNow(): Date {
  return useContext(NowContext);
}
