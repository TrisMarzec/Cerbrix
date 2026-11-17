import type { ComponentChildren } from 'preact';
import { createContext } from 'preact';
import { useCallback, useContext, useEffect, useMemo, useState } from 'preact/hooks';

import { api, onUnauthorized } from './api/client';
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

  useEffect(() => {
    onUnauthorized(() => {
      setState((current) => (current.kind === 'in' ? { kind: 'out', expired: true } : current));
    });
    check();
    return () => {
      onUnauthorized(() => {});
    };
  }, [check]);

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
          setState({ kind: 'in', user: loggedIn });
        }}
      />
    );
  }
  return <SessionContext.Provider value={session}>{children}</SessionContext.Provider>;
}
