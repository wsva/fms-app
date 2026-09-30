import { invoke } from "@tauri-apps/api/core";

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * Log an error message to the app's logging system
 */
export function logError(message: string, module: string = "frontend"): void {
  if (!isTauri()) {
    console.error(`[${module}] ${message}`);
    return;
  }
  invoke("log_frontend_message", {
    message,
    level: "ERROR",
    module,
  }).catch((err) => {
    console.error("Failed to log error:", err);
  });
}

/**
 * Log an info message to the app's logging system
 */
export function logInfo(message: string, module: string = "frontend"): void {
  if (!isTauri()) {
    console.info(`[${module}] ${message}`);
    return;
  }
  invoke("log_frontend_message", {
    message,
    level: "INFO",
    module,
  }).catch((err) => {
    console.error("Failed to log info:", err);
  });
}

/**
 * Log a warning message to the app's logging system
 */
export function logWarning(message: string, module: string = "frontend"): void {
  if (!isTauri()) {
    console.warn(`[${module}] ${message}`);
    return;
  }
  invoke("log_frontend_message", {
    message,
    level: "WARN",
    module,
  }).catch((err) => {
    console.error("Failed to log warning:", err);
  });
}
