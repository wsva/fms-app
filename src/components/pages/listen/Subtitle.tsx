"use client";

import { invoke } from "@tauri-apps/api/core";
import { Button } from "@heroui/react";
import type { ListenSubtitle } from "@/lib/types";
import React, { useState } from "react";

type Props = {
  item: ListenSubtitle;
  datasetUuid: string;
  isActive: boolean;
  onSelect: () => void;
};

export default function SubtitleItem({ item, datasetUuid, isActive, onSelect }: Props) {
  const [stateSaving, setStateSaving] = useState<boolean>(false);

  return (
    <div className="flex flex-col items-center justify-start w-full my-2">
      <div className="flex flex-row items-center justify-between w-full gap-2">
        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant={isActive ? "primary" : "secondary"}
            onPress={onSelect}
          >
            {item.name || "Subtitle"}
          </Button>
        </div>
        <div className="flex flex-row items-center gap-2">
          <Button
            variant="danger"
            size="sm"
            isDisabled={stateSaving}
            onPress={async () => {
              setStateSaving(true);
              // Delete subtitle and all its cues
              try {
                // Delete all cues first
                const cues = await invoke<
                  Array<{ uuid: string }>
                >("listen_get_cues", {
                  datasetUuid,
                  subtitleUuid: item.uuid,
                });
                for (const cue of cues) {
                  await invoke("listen_delete_cue", {
                    datasetUuid,
                    cueUuid: cue.uuid,
                  });
                }
              } catch (e) {
                console.error(e);
              }
              setStateSaving(false);
            }}
          >
            Delete
          </Button>
        </div>
      </div>
      <div className="flex flex-col items-start justify-start w-full gap-0.5">
        <div className="text-sm text-text-secondary">
          UUID: {item.uuid}
        </div>
        {item.note && (
          <div className="text-sm text-text-tertiary">{item.note}</div>
        )}
      </div>
    </div>
  );
}
