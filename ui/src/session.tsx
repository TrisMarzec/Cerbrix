import type { ComponentChildren } from 'preact';
import { createContext } from 'preact';
import { useCallback, useContext, useEffect, useMemo, useRef, useState } from 'preact/hooks';

import {
  api,
  ApiError,
  currentSessionGeneration,
  newSessionGeneration,
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
  logout: () => Promise<void>;
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

/**
 * Asks who is logged in and renders `children` for a user, the login page otherwise. The login page
 * takes the place of the page in the same URL, so after a login the user is where they were.
 */
export function SessionProvider({ children }: { children: ComponentChildren }) {
  const [state, setState] = useState<State>({ kind: 'loading' });

  const check = useCallback(() => {
    setState({ kind: 'loading' });
    api.get<SessionUser>('/session').then(
      (user) => {
        setState({ kind: 'in', user });
      },
      (e: unknown) => {
        const unauthorized = typeof e === 'object' && e !== null && 'status' in e && e.status === 401;
        setState(unauthorized ? { kind: 'out', expired: false } : { kind: 'unreachable' });
      },
    );
  }, []);

  // A 401 may only mean that the cookie was replaced while a request was in flight (a password
  // change). So the session asks once more who is logged in; concurrent 401s share that one check.
  const rechecking = useRef(false);
  const recheck = useCallback(() => {
    if (rechecking.current) return;
    rechecking.current = true;
    const generation = currentSessionGeneration();
    api.probe<SessionUser>('/session').then(
      (fresh) => {
        rechecking.current = false;
        if (generation !== currentSessionGeneration()) return;
        setState((current) => {
          if (current.kind !== 'in') return current;
          const same = current.user.name === fresh.name && current.user.role === fresh.role;
          return same ? current : { kind: 'in', user: fresh };
        });
      },
      (e: unknown) => {
        rechecking.current = false;
        if (generation !== currentSessionGeneration()) return;
        // Only a 401 here ends the session; a network failure keeps the user in (the polls show it).
        if (e instanceof ApiError && e.status === 401) {
          setState((current) => (current.kind === 'in' ? { kind: 'out', expired: true } : current));
        }
      },
    );
  }, []);

  useEffect(() => {
    onUnauthorized(recheck);
    check();
    return () => {
      onUnauthorized(() => {});
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
        } catch {
          // The session is gone on this side either way.
        }
        // The next person must not land on the page of the one who left.
        navigate('/', { replace: true });
        newSessionGeneration();
        setState({ kind: 'out', expired: false });
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
  return <SessionContext.Provider value={session}>{children}</SessionContext.Provider>;
}
