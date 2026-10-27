"use client";

import type { ReactNode } from "react";

export type ConfirmRequest = {
  title?: string;
  message: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  /** When false the confirm button uses the accent color instead of red. */
  danger?: boolean;
  /** If omitted the dialog renders as an alert with a single OK button. */
  onConfirm?: () => void;
};

type Props = {
  request: ConfirmRequest | null;
  onClose: () => void;
};

export default function ConfirmDialog({ request, onClose }: Props) {
  if (!request) return null;
  const isAlert = !request.onConfirm;
  const danger = request.danger !== false;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onClick={onClose}
    >
      <div
        className="bg-bg-card rounded-2xl shadow-2xl p-6 flex flex-col gap-4 w-80 border border-border-default"
        onClick={(e) => e.stopPropagation()}
      >
        {request.title && (
          <p className="font-semibold text-base text-text-primary">{request.title}</p>
        )}
        <div className="text-sm text-text-secondary">{request.message}</div>
        <div className="flex flex-row justify-end gap-2">
          {!isAlert && (
            <button
              className="px-3 py-2 rounded-md text-sm text-text-secondary hover:bg-bg-hover cursor-pointer"
              onClick={onClose}
            >
              {request.cancelLabel ?? "Cancel"}
            </button>
          )}
          <button
            className={`px-3 py-2 rounded-md text-sm text-white hover:opacity-90 cursor-pointer ${
              danger ? "bg-red-500" : "bg-accent-bg"
            }`}
            onClick={() => {
              request.onConfirm?.();
              onClose();
            }}
          >
            {isAlert ? "OK" : request.confirmLabel ?? "Confirm"}
          </button>
        </div>
      </div>
    </div>
  );
}
