"use client";

import { type DatasetDetail as DatasetDetailType } from "@/lib/datasets/types";

interface Props {
  detail: DatasetDetailType;
  datasetPath: string;
}

export default function DatasetDetailPanel({ detail, datasetPath }: Props) {
  return (
    <div className="px-4 pb-4 border-t border-border-default">
      <p className="text-xs text-text-tertiary font-mono mb-2">{datasetPath}</p>
      {detail.info.description && (
        <p className="text-sm text-text-secondary">{detail.info.description}</p>
      )}
    </div>
  );
}
