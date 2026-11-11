"use client";

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, Loader2, X } from "lucide-react";
import type { FamilyCandidate, DatasetTypeSlug } from "@/lib/datasets/family";

type Props = {
  /** The dataset being re-keyed. */
  uuid: string;
  name: string;
  /** Its type slug, which is also the prefix its new id takes. */
  datasetType: DatasetTypeSlug;
  onCancel: () => void;
  /** Resolving closes the dialog; the page reloads its listing. */
  onJoined: () => Promise<void> | void;
};

/**
 * Pick a family for one dataset — the manual companion builder.
 *
 * A family is nothing but a shared family key in the ids (`<type>_<key>`), so
 * "joining" is re-keying this dataset's `info.json` uuid to the picked family's
 * key. Everything else about the dataset is untouched; what *is* keyed by the
 * old id (progress, review history, XP, sync state) does not follow, which is
 * what the warning is for.
 *
 * The candidate list comes from the backend rather than being computed here: a
 * family is a relation spread across all five type trees, and only the disk
 * knows which members actually resolved.
 */
export default function JoinFamilyDialog({
  uuid,
  name,
  datasetType,
  onCancel,
  onJoined,
}: Props) {
  const [candidates, setCandidates] = useState<FamilyCandidate[] | null>(null);
  const [picked, setPicked] = useState<FamilyCandidate | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let dead = false;
    invoke<FamilyCandidate[]>("dataset_family_candidates", { uuid })
      .then((list) => {
        if (!dead) setCandidates(list);
      })
      .catch((e) => {
        if (!dead) setError(String(e));
      });
    return () => {
      dead = true;
    };
  }, [uuid]);

  const join = async () => {
    if (!picked || busy) return;
    setBusy(true);
    setError(null);
    try {
      await invoke("dataset_join_family", { uuid, targetUuid: picked.uuid });
      await onJoined();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4"
      onClick={onCancel}
    >
      <div
        className="flex max-h-[85vh] w-full max-w-md flex-col gap-3 rounded-2xl border border-border-default bg-bg-card p-5 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <h2 className="text-base font-semibold text-text-primary">Join a family</h2>
          <button
            onClick={onCancel}
            className="cursor-pointer rounded p-1 text-text-tertiary hover:bg-bg-hover hover:text-text-primary"
            aria-label="Close"
          >
            <X size={18} />
          </button>
        </div>

        <p className="text-xs leading-relaxed text-text-secondary">
          Put <span className="font-medium text-text-primary">{name}</span> in the same family as
          another dataset, so the two are offered to each other as companions.
        </p>

        {error && (
          <p className="rounded-md bg-error-text/10 px-2 py-1.5 text-xs leading-tight break-words text-error-text">
            {error}
          </p>
        )}

        <div className="min-h-0 flex-1 overflow-y-auto rounded-lg border border-border-default">
          {candidates === null ? (
            <p className="flex items-center gap-2 px-3 py-4 text-xs text-text-tertiary">
              <Loader2 size={12} className="animate-spin" />
              Looking for families…
            </p>
          ) : candidates.length === 0 ? (
            <p className="px-3 py-4 text-xs leading-relaxed text-text-tertiary">
              Nothing to join yet: a family holds one dataset of each type, so this needs another
              dataset whose family has no {datasetType} member.
            </p>
          ) : (
            <ul className="divide-y divide-border-default">
              {candidates.map((c) => (
                <li key={c.family}>
                  <button
                    type="button"
                    onClick={() => setPicked(c)}
                    className={`flex w-full cursor-pointer flex-col gap-0.5 px-3 py-2 text-left hover:bg-bg-hover ${
                      picked?.family === c.family ? "bg-accent-bg/10" : ""
                    }`}
                  >
                    <span className="truncate text-sm text-text-primary">{c.name}</span>
                    <span className="truncate text-[10px] text-text-tertiary">
                      {c.member_types.join(" · ")} — id {c.dataset_type}_{c.family}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>

        {/* The part the user must read before confirming: the id is an address.
            Content and practice state are keyed by other things (card uuids, the
            acting user), so they survive; what is addressed *by* the id does not. */}
        <div className="flex gap-2 rounded-lg bg-amber-500/10 p-2.5">
          <AlertTriangle size={14} className="mt-0.5 shrink-0 text-amber-600" />
          <div className="flex flex-col gap-1 text-[11px] leading-relaxed text-text-secondary">
            <span>
              This renames this dataset&apos;s id
              {picked && (
                <>
                  {" "}
                  to{" "}
                  <span className="font-mono text-text-primary">
                    {datasetType}_{picked.family}
                  </span>
                </>
              )}
              . Its files, cards and your practice history stay exactly as they are.
            </span>
            <span>
              But the id is how everything <em>else</em> addresses it: another device&apos;s copy, a
              saved MCP call, its sync state. Those still know the old id and will treat this as a
              new dataset that has never synced.
            </span>
          </div>
        </div>

        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            disabled={busy}
            className="cursor-pointer rounded-md px-3 py-2 text-sm text-text-secondary hover:bg-bg-hover disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={join}
            disabled={!picked || busy}
            className="inline-flex cursor-pointer items-center gap-1.5 rounded-md bg-accent-bg px-4 py-2 text-sm text-white hover:bg-accent-bg-hover disabled:cursor-not-allowed disabled:opacity-60"
          >
            {busy && <Loader2 size={13} className="animate-spin" />}
            {busy ? "Joining…" : "Join family"}
          </button>
        </div>
      </div>
    </div>
  );
}
