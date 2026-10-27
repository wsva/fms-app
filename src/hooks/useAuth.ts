/**
 * Shared auth / XP hook.
 *
 * Extracted from Sidebar so both the desktop sidebar and the mobile bottom-nav
 * "More" sheet can render the login/user/XP controls without duplicating the
 * Tauri event wiring. Auth is per-workspace: on `workspace-selected` the old
 * user is cleared and reloaded from the newly selected workspace.
 */

import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "@/lib/tauri";

export interface AuthUser {
  name: string;
  email: string;
}

export interface XpUser {
  user_id: string;
  lifetime_xp: number;
  level: number;
  updated_at: string;
  // §3.6: XP earned on this follower but not yet confirmed by the hub (still
  // queued). Already folded into `lifetime_xp`; shown only as a syncing hint.
  pending_xp?: number;
}

export interface XpAwardResult {
  xp_awarded: number;
  lifetime_xp: number;
  level: number;
  is_new: boolean;
}

export function useAuth() {
  const [authUser, setAuthUser] = useState<AuthUser | null>(null);
  const [xpUser, setXpUser] = useState<XpUser | null>(null);
  const [xpFlash, setXpFlash] = useState<number | null>(null);
  // True while `auth_get_user` is in flight (startup, workspace switch, manual
  // re-check). The user-info area renders "Checking…" instead of the Login
  // button so a pending network validation never reads as signed out.
  const [checking, setChecking] = useState(true);
  // In-app login modal visibility. Login is verified against the website's
  // signin API (see LoginModal + auth_login_password), not via a browser
  // deep-link round trip, so it works reliably on Android.
  const [loginOpen, setLoginOpen] = useState(false);

  const checkAuth = useCallback(async () => {
    if (!isTauri()) {
      setChecking(false);
      return;
    }
    setChecking(true);
    try {
      const user = await invoke<AuthUser | null>("auth_get_user");
      setAuthUser(user);
      if (user) {
        const xp = await invoke<XpUser | null>("xp_get_user");
        setXpUser(xp);
      } else {
        setXpUser(null);
      }
    } catch {
      /* ignore */
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    checkAuth();
    if (!isTauri()) return;
    const unlistenLogin = listen<AuthUser>("auth-login-success", (event) => {
      setAuthUser(event.payload);
      setChecking(false);
      // Fetch XP after login.
      invoke<XpUser | null>("xp_get_user").then(setXpUser).catch(() => {});
    });
    const unlistenLogout = listen("auth-logout", () => {
      setAuthUser(null);
      setXpUser(null);
      setChecking(false);
    });
    // Auth is per-workspace: on workspace switch, clear the old user and
    // reload auth/XP from the newly selected workspace's auth.json.
    const unlistenWorkspace = listen("workspace-selected", () => {
      setAuthUser(null);
      setXpUser(null);
      checkAuth();
    });
    const unlistenXp = listen<XpAwardResult>("xp-earned", (event) => {
      setXpUser((prev) =>
        prev
          ? { ...prev, lifetime_xp: event.payload.lifetime_xp, level: event.payload.level }
          : { user_id: "", lifetime_xp: event.payload.lifetime_xp, level: event.payload.level, updated_at: new Date().toISOString() }
      );
      // Flash the XP gain.
      setXpFlash(event.payload.xp_awarded);
      setTimeout(() => setXpFlash(null), 2000);
    });
    return () => {
      unlistenLogin.then((fn) => fn());
      unlistenLogout.then((fn) => fn());
      unlistenWorkspace.then((fn) => fn());
      unlistenXp.then((fn) => fn());
    };
  }, [checkAuth]);

  const login = useCallback(() => {
    setLoginOpen(true);
  }, []);

  const closeLogin = useCallback(() => {
    setLoginOpen(false);
  }, []);

  const logout = useCallback(async () => {
    if (!isTauri()) return;
    try {
      await invoke("auth_logout");
      // Clear the XP alongside the user: the `auth-logout` event is the other
      // path, but this one always runs, so no stale level can outlive a
      // signed-out state.
      setAuthUser(null);
      setXpUser(null);
      setXpFlash(null);
    } catch {
      /* ignore */
    }
  }, []);

  return { authUser, xpUser, xpFlash, checking, login, logout, loginOpen, closeLogin };
}
