"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow, type Window } from "@tauri-apps/api/window";
import { isTauri } from "@/lib/platform";
import { logError } from "@/lib/logger";

/**
 * The window's own controls — minimize / maximize-restore / close / fullscreen —
 * for the surfaces that draw them (the custom title bar).
 *
 * The window is created with `decorations: false`, so the app draws this chrome
 * itself and nothing here is decorative: `maximized` decides which glyph the
 * middle button shows, and `fullscreen` which row of the View menu is checked.
 * Both are *read back* from the native window rather than tracked from the
 * buttons, because the OS changes them too — Win+Up/Down, a drag of a maximized
 * window back down, F11 from a keyboard that never touched this component.
 *
 * Everything no-ops outside Tauri: the same static export runs in a browser,
 * where there is no window to command and `getCurrentWindow()` would throw.
 */
export function useWindowControls() {
  const [available, setAvailable] = useState(false);
  const [maximized, setMaximized] = useState(false);
  // The state setter is `setFullscreenState`, not `setFullscreen`: the name
  // `setFullscreen` belongs to the command that changes the *window*, which is
  // what a caller of this hook wants to call.
  const [fullscreen, setFullscreenState] = useState(false);
  const winRef = useRef<Window | null>(null);

  // Read both states off the native window. A rejected getter (a platform that
  // has no answer, a window already gone) simply leaves the previous value.
  const sync = useCallback((win: Window) => {
    win.isMaximized().then(setMaximized).catch(() => {});
    win.isFullscreen().then(setFullscreenState).catch(() => {});
  }, []);

  useEffect(() => {
    if (!isTauri()) return;
    const win = getCurrentWindow();
    winRef.current = win;
    setAvailable(true);
    sync(win);

    // Resizing is the only native signal that a maximize or a fullscreen toggle
    // leaves behind; there is no `tauri://maximized-changed` event.
    const unlisten = win.onResized(() => sync(win));

    return () => {
      void unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [sync]);

  /** Run one window command, then re-read the states it changed, reporting a
   *  failure the app's way instead of silently doing nothing under the user's
   *  cursor. The promise settles once the command is done, which the click
   *  handlers ignore and an agent action awaits before reading the window back. */
  const run = useCallback(
    (label: string, action: (win: Window) => Promise<void>) => {
      const win = winRef.current;
      if (!win) return Promise.resolve();
      return action(win)
        .then(() => sync(win))
        .catch((e) => logError(`${label} failed: ${String(e)}`, "titlebar"));
    },
    [sync],
  );

  /** The window's state read off the *native* window rather than off this
   *  render. A click has no use for it — React flips the glyph from `sync` — but
   *  an agent invoking a command through `ui_invoke_action` gets this back as the
   *  action's detail, and an answer built from hook state could report the state
   *  from before the click. */
  const readState = useCallback(async () => {
    const win = winRef.current;
    if (!win) return { maximized, fullscreen };
    const [isMax, isFull] = await Promise.all([
      win.isMaximized().catch(() => maximized),
      win.isFullscreen().catch(() => fullscreen),
    ]);
    return { maximized: isMax, fullscreen: isFull };
  }, [maximized, fullscreen]);

  const minimize = useCallback(() => run("Minimize", (w) => w.minimize()), [run]);
  const toggleMaximize = useCallback(
    () => run("Toggle maximize", (w) => w.toggleMaximize()),
    [run],
  );
  // Closing the main window is what quits the app in this build: the shell owns
  // the only window, and Tauri exits once the last one closes.
  const close = useCallback(() => run("Close", (w) => w.close()), [run]);
  // The show state to hand back when fullscreen ends. Kept in a ref, not state:
  // nothing renders from it, and the value has to survive the whole fullscreen
  // stretch, during which the window is deliberately *not* maximized.
  const maximizedBeforeFullscreen = useRef(false);

  /** Enter or leave native fullscreen.
   *
   *  The maximize sandwich is a workaround for a tao interaction that only bites
   *  an *undecorated* window (`decorations: false`, which this app is). Its
   *  `WM_NCCALCSIZE` handler shrinks the client area to the monitor's **work
   *  area** for as long as `GetWindowPlacement` still reports `SW_MAXIMIZE` —
   *  correct for a maximized window, whose frame is deliberately oversized — but
   *  tao never clears that show state when it enters borderless fullscreen, it
   *  only moves the frame. So the frame covers the taskbar and the shell hides
   *  it, while the webview keeps its maximized height: a dead strip exactly where
   *  the taskbar was. Unmaximizing first takes the handler's other branch, which
   *  applies no inset at all while fullscreen, and the content fills the monitor.
   *
   *  Both commands are queued to the same event-loop pass, so the intermediate
   *  restored size is not a frame the user sees. */
  const setFullscreen = useCallback(
    (on: boolean) =>
      run("Fullscreen", async (w) => {
        if (on) {
          maximizedBeforeFullscreen.current = await w.isMaximized().catch(() => false);
          if (maximizedBeforeFullscreen.current) await w.unmaximize();
          await w.setFullscreen(true);
          return;
        }
        await w.setFullscreen(false);
        if (maximizedBeforeFullscreen.current) {
          maximizedBeforeFullscreen.current = false;
          await w.maximize();
        }
      }),
    [run],
  );

  const toggleFullscreen = useCallback(
    () => setFullscreen(!fullscreen),
    [setFullscreen, fullscreen],
  );

  return {
    available,
    maximized,
    fullscreen,
    minimize,
    toggleMaximize,
    close,
    setFullscreen,
    toggleFullscreen,
    readState,
  };
}
