import './shell/shell.css';
import { match, useRoute } from './lib/router';
import { SessionProvider } from './session';
import { Shell } from './shell/Shell';
import { DisplayPage } from './wall/DisplayPage';
import { WallPage } from './wall/WallPage';

/**
 * A wall monitor (`/display/…`) has no session; everything else needs one: a dashboard in wall
 * mode (`/wall/…`) outside the shell, every other path inside it. Each wall is keyed by its id, so
 * going from one to another starts fresh.
 */
export function App() {
  const { path } = useRoute();
  const display = match('/display/:id', path);
  if (display?.id !== undefined) return <DisplayPage key={display.id} id={display.id} />;
  const wall = match('/wall/:id', path);
  return (
    <SessionProvider>
      {wall?.id !== undefined ? <WallPage key={wall.id} id={wall.id} /> : <Shell />}
    </SessionProvider>
  );
}
