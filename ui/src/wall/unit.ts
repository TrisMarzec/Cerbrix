import { useEffect, useState } from 'preact/hooks';

/** The wall's scale unit in pixels, as `--u` in wall.css: a 1920th of the viewport width. */
function wallUnit(): number {
  return window.innerWidth / 1920;
}

/** `--u` in pixels, for what is drawn on a canvas; read again on every window resize. */
export function useWallUnit(): number {
  const [u, setU] = useState(wallUnit);
  useEffect(() => {
    const update = () => setU(wallUnit());
    update();
    window.addEventListener('resize', update);
    return () => window.removeEventListener('resize', update);
  }, []);
  return u;
}
