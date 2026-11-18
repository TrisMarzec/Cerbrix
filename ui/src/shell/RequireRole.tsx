import type { ComponentChildren } from 'preact';

import { useSession, type Role } from '../session';

/** Shows `children` to a user of `role` or higher, and a plain refusal to anyone else. */
export function RequireRole({ role, children }: { role: Role; children: ComponentChildren }) {
  const { can } = useSession();
  if (!can(role)) return <h1>Not allowed for your role</h1>;
  return <>{children}</>;
}
