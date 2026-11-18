import { useEffect, useState } from 'preact/hooks';

/** A resize is taken once the window has kept its size this long, so dragging redraws nothing. */
const SETTLE_MS = 150;

/** The wall's scale unit in pixels, as `--u` in wall.css: a 1920th of the viewport width. */
function wallUnit(): number {
  return window.innerWidth / 1920;
}

/**
 * `--u` in pixels, for what is drawn on a canvas; read again 150 ms after the last window resize.
 * The timer and the listener are released on unmount.
 */
export function useWallUnit(): number {
  const [u, setU] = useState(wallUnit);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const update = () => {
      clearTimeout(timer);
      timer = setTimeout(() => setU(wallUnit()), SETTLE_MS);
    };
    setU(wallUnit());
    window.addEventListener('resize', update);
    return () => {
      clearTimeout(timer);
      window.removeEventListener('resize', update);
    };
  }, []);
  return u;
}
