/**
 * The PC-side sync service and this workspace's role, for the status bar.
 *
 * Nothing on the frontend read `web_service_get_status` before: the service
 * auto-starts at launch and its configuration lives on the Devices & Hub page,
 * so the bar is the first surface that wants the *running* answer cheaply and
 * permanently. 10s is deliberate — the port does not change under the user, and
 * the bar must not become the app's busiest poller.
 */

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "@/lib/tauri";
import { useVisiblePolling } from "./useVisiblePolling";

/** Mirror of `sync::server::WebServiceStatus` (serialized field names). */
export interface SyncServiceStatus {
  running: boolean;
  port: number;
  stt: boolean;
  dataset: boolean;
  tts: boolean;
  local_url: string | null;
  lan_url: string | null;
}

/** The same payload `web_service_start` takes. `api_token` is a deprecated
 *  field the backend still deserialises (serde default), so it is sent empty. */
interface SyncServiceConfig {
  port: number;
  stt: boolean;
  dataset: boolean;
  tts: boolean;
  apiToken: string;
}

/** Fallback for a start with no status read yet — mirrors `DEFAULT_PORT` in
 *  `sync/server.rs`. A literal 0 here would ask the OS for an ephemeral port,
 *  which no phone can find without the discovery beacon. */
const DEFAULT_PORT = 35711;

export function useSyncStatus(active: boolean) {
  const [status, setStatus] = useState<SyncServiceStatus | null>(null);
  /** `"hub"` | `"follower"`; the backend treats empty as follower. */
  const [role, setRole] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const [service, settings] = await Promise.all([
        invoke<SyncServiceStatus>("web_service_get_status"),
        invoke<{ role?: string }>("settings_get"),
      ]);
      setStatus(service);
      setRole(settings.role ?? "");
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    if (active) void refresh();
  }, [active, refresh]);

  useVisiblePolling(active, () => void refresh(), 10000);

  /** Toggle the service, keeping whatever the phone-facing flags already are:
   *  a start that reset them would silently turn STT or TTS serving off. */
  const setRunning = useCallback(
    async (on: boolean) => {
      if (busy) return;
      setBusy(true);
      try {
        const next = on
          ? await invoke<SyncServiceStatus>("web_service_start", {
              config: {
                port: status?.port || DEFAULT_PORT,
                stt: status?.stt ?? true,
                dataset: status?.dataset ?? true,
                tts: status?.tts ?? true,
                apiToken: "",
              } satisfies SyncServiceConfig,
            })
          : await invoke<SyncServiceStatus>("web_service_stop");
        setStatus(next);
        setError(null);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(false);
      }
    },
    [busy, status]
  );

  return { status, role, busy, error, setRunning, refresh };
}

export default useSyncStatus;
