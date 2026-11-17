import type { ComponentChildren } from 'preact';

import { match } from '../lib/router';
import { NotFoundPage } from '../pages/NotFoundPage';
import { DashboardsPage } from '../pages/DashboardsPage';
import { HostsPage } from '../pages/HostsPage';
import { RequireRole } from './RequireRole';

function Placeholder({ heading }: { heading: string }) {
  return <h1>{heading}</h1>;
}

interface RouteDef {
  pattern: string;
  /** The navigation entry this page belongs to. */
  nav: string;
  title: (params: Record<string, string>) => string;
  element: (params: Record<string, string>) => ComponentChildren;
}

// The pages of later tasks replace the placeholders here.
const ROUTES: RouteDef[] = [
  {
    pattern: '/dashboards',
    nav: '/dashboards',
    title: () => 'Dashboards',
    element: () => <DashboardsPage />,
  },
  {
    pattern: '/hosts',
    nav: '/hosts',
    title: () => 'Hosts',
    element: () => <HostsPage />,
  },
  {
    pattern: '/hosts/:name',
    nav: '/hosts',
    title: (p) => p['name'] ?? 'Host',
    // Keyed by the name: another host is a fresh page, so no state or data carries over.
    element: (p) => <Placeholder key={p['name']} heading={p['name'] ?? 'Host'} />,
  },
  {
    pattern: '/problems',
    nav: '/problems',
    title: () => 'Problems',
    element: () => <Placeholder heading="Problems" />,
  },
  {
    pattern: '/configuration',
    nav: '/configuration',
    title: () => 'Configuration',
    element: () => <Placeholder heading="Configuration" />,
  },
  {
    pattern: '/administration',
    nav: '/administration',
    title: () => 'Administration',
    element: () => (
      <RequireRole role="admin">
        <Placeholder heading="Administration" />
      </RequireRole>
    ),
  },
];

export interface Resolved {
  title: string;
  /** The navigation entry to mark as current; undefined for the not-found page. */
  nav: string | undefined;
  element: ComponentChildren;
}

/** Finds the page for a path. `/` shows the dashboards (the shell also redirects it). */
export function resolveRoute(path: string): Resolved {
  const wanted = match('/', path) !== null ? '/dashboards' : path;
  for (const route of ROUTES) {
    const params = match(route.pattern, wanted);
    if (params !== null) {
      return { title: route.title(params), nav: route.nav, element: route.element(params) };
    }
  }
  return { title: 'Page not found', nav: undefined, element: <NotFoundPage /> };
}
