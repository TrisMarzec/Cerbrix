import { Link } from '../lib/router';

export function NotFoundPage() {
  return (
    <>
      <h1>Page not found</h1>
      <p class="dim">There is no page at this address.</p>
      <p>
        <Link href="/dashboards">Go to the dashboards</Link>
      </p>
    </>
  );
}
