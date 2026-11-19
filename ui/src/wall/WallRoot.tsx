import './wall.css';

import type { ComponentChildren } from 'preact';
import { useEffect, useState } from 'preact/hooks';

/** The cursor (and the back link of a wall page) disappear after this long without input. */
export const IDLE_MS = 3_000;

/** Events that wake the wall: the mouse, a pointer (touch included) and the keyboard. */
const WAKE_EVENTS = ['mousemove', 'pointermove', 'pointerdown', 'keydown'] as const;

/**
 * Whether nobody has used the mouse, a pointer or the keyboard for `IDLE_MS`; the timer runs from
 * mount. A move to the position of the last one is no movement (remote desktops and KVM switches
 * send such moves) and does not wake the wall. Typing wakes it, so a form being filled in keeps
 * its cursor.
 */
export function useIdle(): boolean {
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
 * The wall's dark root, which hides the cursor while idle: for the frame of a dashboard and for
 * every state of a wall path outside it (a notice, the session gate, the token form).
 */
export function WallRoot({ children }: { children?: ComponentChildren }) {
  const idle = useIdle();
  return <div class={idle ? 'wall is-idle' : 'wall'}>{children}</div>;
}
