"use client";

/**
 * In-app login modal.
 *
 * Verifies credentials directly against the website's first-party signin API
 * via the `auth_login_password` Tauri command (no browser round trip). This is
 * the primary login path on Android, where the `fms-app://` deep-link callback
 * is blocked by Chrome (a server-issued custom-scheme redirect carries no
 * transient user activation). On success the backend emits `auth-login-success`,
 * which `useAuth` listens for, so the caller's avatar/XP updates automatically.
 *
 * Visual style deliberately mirrors the website's `/oauth2/login` page
 * (gradient badge, white card, purple focus rings) for a consistent look.
 */

import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { isTauri } from "@/lib/tauri";
import { logInfo, logError } from "@/lib/logger";

const SITE_BASE = "https://lusworkshop.site";
const GRADIENT = "linear-gradient(135deg, #667eea, #764ba2)";

export default function LoginModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const [identity, setIdentity] = useState("");
  const [password, setPassword] = useState("");
  const [showPw, setShowPw] = useState(false);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  // Reset transient state each time the modal is opened.
  useEffect(() => {
    if (open) {
      setError("");
      setLoading(false);
      setPassword("");
    }
  }, [open]);

  if (!open) return null;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!isTauri() || loading) return;
    setLoading(true);
    setError("");
    logInfo(`Login attempt for "${identity}"`, "auth");
    try {
      await invoke("auth_login_password", { email: identity, password });
      logInfo(`Login succeeded for "${identity}"`, "auth");
      setPassword("");
      onClose();
    } catch (err) {
      const msg = typeof err === "string" ? err : "Sign in failed. Please try again.";
      // The backend already returns its full error source chain in `err`, so
      // this surfaces the underlying TLS/DNS/network cause in the Logs page.
      logError(`Login failed for "${identity}": ${msg}`, "auth");
      setError(msg);
    } finally {
      setLoading(false);
    }
  }

  function openSite(path: string) {
    if (!isTauri()) return;
    openUrl(`${SITE_BASE}${path}`).catch(() => {});
  }

  const inputClass =
    "w-full pl-10 pr-4 py-2.5 text-sm border-[1.5px] border-gray-200 rounded-xl bg-gray-50 text-gray-900 outline-none transition-all focus:border-[#667eea] focus:bg-white focus:shadow-[0_0_0_3px_rgba(102,126,234,0.15)]";

  return (
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center bg-black/50 p-4"
      onClick={onClose}
    >
      <div
        className="relative w-full max-w-md rounded-2xl p-8 sm:p-10 shadow-2xl"
        style={{ background: "rgba(255,255,255,0.97)" }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Close */}
        <button
          type="button"
          onClick={onClose}
          aria-label="Close"
          className="absolute right-4 top-4 text-gray-400 hover:text-gray-600 transition-colors cursor-pointer"
        >
          <svg className="w-5 h-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <line x1="18" y1="6" x2="6" y2="18" />
            <line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>

        {/* Logo */}
        <div
          className="w-14 h-14 rounded-2xl flex items-center justify-center mx-auto mb-6 shadow-lg"
          style={{ background: GRADIENT }}
        >
          <svg className="w-7 h-7 fill-white" viewBox="0 0 24 24">
            <path d="M12 1C8.676 1 6 3.676 6 7v1H4v15h16V8h-2V7c0-3.324-2.676-6-6-6zm0 2c2.276 0 4 1.724 4 4v1H8V7c0-2.276 1.724-4 4-4zm0 9a2 2 0 1 1 0 4 2 2 0 0 1 0-4z" />
          </svg>
        </div>

        <h1 className="text-center text-2xl font-bold text-gray-900 mb-1 tracking-tight">
          Welcome
        </h1>
        <p className="text-center text-sm text-gray-500 mb-8">Sign in to your account</p>

        <form onSubmit={handleSubmit} autoComplete="on">
          {/* Email field */}
          <div className="mb-5">
            <label className="block text-xs font-semibold text-gray-700 mb-1.5 tracking-wide">
              Email
            </label>
            <div className="relative">
              <input
                type="text"
                value={identity}
                onChange={(e) => setIdentity(e.target.value)}
                placeholder="you@example.com"
                autoComplete="username"
                required
                className={inputClass}
              />
              <svg
                className="absolute left-3.5 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400"
                viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"
                strokeLinecap="round" strokeLinejoin="round"
              >
                <path d="M4 4h16c1.1 0 2 .9 2 2v12c0 1.1-.9 2-2 2H4c-1.1 0-2-.9-2-2V6c0-1.1.9-2 2-2z" />
                <polyline points="22,6 12,13 2,6" />
              </svg>
            </div>
          </div>

          {/* Password field */}
          <div className="mb-6">
            <label className="block text-xs font-semibold text-gray-700 mb-1.5 tracking-wide">
              Password
            </label>
            <div className="relative">
              <input
                type={showPw ? "text" : "password"}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="••••••••"
                autoComplete="current-password"
                required
                className={`${inputClass} pr-10`}
              />
              <svg
                className="absolute left-3.5 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400"
                viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"
                strokeLinecap="round" strokeLinejoin="round"
              >
                <rect x="3" y="11" width="18" height="11" rx="2" ry="2" />
                <path d="M7 11V7a5 5 0 0 1 10 0v4" />
              </svg>
              <button
                type="button"
                onClick={() => setShowPw(!showPw)}
                tabIndex={-1}
                aria-label={showPw ? "Hide password" : "Show password"}
                className="absolute right-3 top-1/2 -translate-y-1/2 text-gray-400 hover:text-[#667eea] transition-colors cursor-pointer"
              >
                {showPw ? (
                  <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24" />
                    <line x1="1" y1="1" x2="23" y2="23" />
                  </svg>
                ) : (
                  <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                    <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
                    <circle cx="12" cy="12" r="3" />
                  </svg>
                )}
              </button>
            </div>
          </div>

          <button
            type="submit"
            disabled={loading}
            className="w-full py-3 text-sm font-semibold text-white rounded-xl transition-all disabled:opacity-60 hover:opacity-90 active:scale-[0.98] shadow-lg shadow-purple-500/40 cursor-pointer"
            style={{ background: GRADIENT }}
          >
            {loading ? "Signing in…" : "Sign In"}
          </button>
        </form>

        <div className="flex justify-between items-center mt-5">
          <button
            type="button"
            onClick={() => openSite("/oauth2/reset-password")}
            className="text-sm text-gray-400 hover:text-[#667eea] transition-colors cursor-pointer"
          >
            Forgot password?
          </button>
          <button
            type="button"
            onClick={() => openSite("/oauth2/register")}
            className="text-sm text-gray-500 cursor-pointer"
          >
            Don&apos;t have an account?{" "}
            <span className="text-[#667eea] font-semibold underline underline-offset-2">
              Create one
            </span>
          </button>
        </div>

        {error && (
          <div className="mt-4 p-3 rounded-xl text-sm bg-red-50 text-red-600 border border-red-200">
            {error}
          </div>
        )}
      </div>
    </div>
  );
}
