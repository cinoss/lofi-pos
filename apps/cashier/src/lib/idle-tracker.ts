import { useEffect, useRef } from "react";

const ACTIVITY_EVENTS = [
  "mousedown",
  "keydown",
  "touchstart",
  "wheel",
  "mousemove",
] as const;

/**
 * Calls onIdle when no user activity has occurred for `idleMs`.
 * Resets the timer on any mouse/keyboard/touch event.
 */
export function useIdleTimer(idleMs: number, onIdle: () => void): void {
  const onIdleRef = useRef(onIdle);
  onIdleRef.current = onIdle;

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;

    const reset = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => onIdleRef.current(), idleMs);
    };
    reset();

    for (const ev of ACTIVITY_EVENTS)
      window.addEventListener(ev, reset, { passive: true });

    return () => {
      if (timer) clearTimeout(timer);
      for (const ev of ACTIVITY_EVENTS) window.removeEventListener(ev, reset);
    };
  }, [idleMs]);
}
