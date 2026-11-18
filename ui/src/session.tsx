import type { ComponentChildren } from 'preact';
import { createContext, Fragment } from 'preact';
import { useCallback, useContext, useEffect, useMemo, useRef, useState } from 'preact/hooks';

import {
  api,
  ApiError,
  currentSessionGeneration,
  newSessionGeneration,
  onSessionDoubt,
  onUnauthorized,
} from './api/client';
import type { Role, SessionUser } from './api/types';
import { navigate } from './lib/router';
import { LoginPage } from './shell/LoginPage';

export type { Role };

const RANK: Record<Role, number> = { viewer: 0, operator: 1, admin: 2 };

interface Session {
  user: SessionUser;
  /** True when the user's role is `role` or higher. For hiding buttons only: the server decides. */
  can: (role: Role) => boolean;
  /**
   * Ends the session. Resolves true when the server confirmed it (`204`, or `401`: it was gone
   * already) and the login page is shown; false when the server did not answer, and the user is
   * still logged in.
   */
  logout: () => Promise<boolean>;
}

const SessionContext = createContext<Session | undefined>(undefined);

export function useSession(): Session {
  const session = useContext(SessionContext);
  if (session === undefined) throw new Error('useSession needs a logged-in session');
  return session;
}

type State =
  | { kind: 'loading' }
  | { kind: 'unreachable' }
  | { kind: 'out'; expired: boolean }
  | { kind: 'in'; user: SessionUser };

function isStatus(e: unknown, status: number): boolean {
  return e instanceof ApiError && e.status === status;
}

/**
 * Asks who is logged in and renders `children` for a user, the login page otherwise. The login page
 * takes the place of the page in the same URL, so after a login the user is where they were.
 * Everything below is keyed by the user's name and role: when the cookie turns out to belong to
 * someone else, nothing of the previous user survives.
 */
export function SessionProvider({ children }: { children: ComponentChildren }) {
  const [state, setState] = useState<State>({ kind: 'loading' });
  const stateRef = useRef(state);
  stateRef.current = state;

  // The first check is a probe: its 401 means "not logged in", not "the session ended".
  const check = useCallback(() => {
    setState({ kind: 'loading' });
    api.probe<SessionUser>('/session').then(
      (user) => {
        setState({ kind: 'in', user });
      },
      (e: unknown) => {
        setState(isStatus(e, 401) ? { kind: 'out', expired: false } : { kind: 'unreachable' });
      },
    );
  }, []);

  // A 401 may only mean that the cookie was replaced while a request was in flight (a password
  // change); a 403 or a tab that becomes visible or a window that gets focus may mean that another user has logged in in
  // another tab. So the session asks once more who is logged in; concurrent triggers share it.
  const rechecking = useRef(false);
  const recheck = useCallback(() => {
    if (rechecking.current || stateRef.current.kind !== 'in') return;
    rechecking.current = true;
    const generation = currentSessionGeneration();
    api.probe<SessionUser>('/session').then(
      (fresh) => {
        rechecking.current = false;
        if (generation !== currentSessionGeneration()) return;
        const current = stateRef.current;
        if (current.kind !== 'in') return;
        if (current.user.name === fresh.name && current.user.role === fresh.role) return;
        // Requests of the previous user say nothing about this one.
        newSessionGeneration();
        setState({ kind: 'in', user: fresh });
      },
      (e: unknown) => {
        rechecking.current = false;
        if (generation !== currentSessionGeneration()) return;
        // Only a 401 here ends the session; a network failure keeps the user in (the polls show it).
        if (isStatus(e, 401)) {
          setState((current) => (current.kind === 'in' ? { kind: 'out', expired: true } : current));
        }
      },
    );
  }, []);

  useEffect(() => {
    onUnauthorized(recheck);
    onSessionDoubt(recheck);
    const onVisibility = () => {
      if (!document.hidden) recheck();
    };
    document.addEventListener('visibilitychange', onVisibility);
    // Two visible windows side by side get no visibility change; focus tells that the user came back.
    window.addEventListener('focus', recheck);
    check();
    return () => {
      onUnauthorized(() => {});
      onSessionDoubt(() => {});
      document.removeEventListener('visibilitychange', onVisibility);
      window.removeEventListener('focus', recheck);
    };
  }, [check, recheck]);

  const user = state.kind === 'in' ? state.user : undefined;
  const session = useMemo<Session | undefined>(() => {
    if (user === undefined) return undefined;
    return {
      user,
      can: (role) => RANK[user.role] >= RANK[role],
      logout: async () => {
        try {
          await api.send('DELETE', '/session');
        } catch (e) {
          // A 401 says the session is gone already; anything else leaves it alive on the server.
          if (!isStatus(e, 401)) return false;
        }
        // The next person must not land on the page of the one who left.
        navigate('/', { replace: true });
        newSessionGeneration();
        setState({ kind: 'out', expired: false });
        return true;
      },
    };
  }, [user]);

  if (state.kind === 'loading') return null;
  if (state.kind === 'unreachable') {
    return (
      <main class="gate">
        <p role="alert">Could not reach the server</p>
        <button type="button" class="btn" onClick={check}>
          Try again
        </button>
      </main>
    );
  }
  if (session === undefined) {
    return (
      <LoginPage
        expired={state.kind === 'out' && state.expired}
        onLoggedIn={(loggedIn) => {
          newSessionGeneration();
          setState({ kind: 'in', user: loggedIn });
        }}
      />
    );
  }
  return (
    <SessionContext.Provider value={session}>
      <Fragment key={`${session.user.role}:${session.user.name}`}>{children}</Fragment>
    </SessionContext.Provider>
  );
}
