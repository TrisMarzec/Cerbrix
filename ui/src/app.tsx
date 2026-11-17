import './shell/shell.css';
import { SessionProvider } from './session';
import { Shell } from './shell/Shell';

export function App() {
  return (
    <SessionProvider>
      <Shell />
    </SessionProvider>
  );
}
