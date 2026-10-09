"use client";

import { memo } from "react";
import {
  ReactFlow,
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  Position,
  type Node,
  type Edge as RfEdge,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";

import {
  type StepDef,
  type StepStatus,
  type PositionedStep,
  type Edge as WfEdge,
  statusLabel,
} from "@/lib/workflow/steps";

// Status → theme-aware palette (CSS vars from globals.css @theme block).
function statusColors(status: StepStatus): {
  bg: string;
  fg: string;
  border: string;
  dashed?: boolean;
} {
  switch (status) {
    case "completed":
      return { bg: "var(--success-bg)", fg: "var(--success-text)", border: "var(--success-text)" };
    case "ready":
      return { bg: "var(--accent-bg)", fg: "#ffffff", border: "var(--accent)" };
    case "running":
      return { bg: "var(--info-bg)", fg: "var(--info-text)", border: "var(--info-text)" };
    case "failed":
      return { bg: "var(--error-bg)", fg: "var(--error-text)", border: "var(--error-text)" };
    case "blocked":
      return { bg: "var(--error-bg)", fg: "var(--error-text)", border: "var(--error-text)", dashed: true };
    case "skipped":
      return { bg: "var(--bg-muted)", fg: "var(--text-tertiary)", border: "var(--border-light)", dashed: true };
    default: // pending
      return { bg: "var(--bg-card)", fg: "var(--text-secondary)", border: "var(--border-default)" };
  }
}

interface StepNodeData {
  step: StepDef;
  status: StepStatus;
  selected: boolean;
  [key: string]: unknown;
}

const StepNode = memo(({ data }: { data: StepNodeData }) => {
  const { step, status, selected } = data;
  const c = statusColors(status);
  return (
    <div
      className="rounded-lg px-3 py-2 min-w-[190px] max-w-[220px] shadow-sm transition-[box-shadow,outline]"
      style={{
        background: c.bg,
        color: c.fg,
        border: `1.5px ${c.dashed ? "dashed" : "solid"} ${c.border}`,
        outline: selected ? "2px solid var(--accent)" : "none",
        outlineOffset: 2,
      }}
    >
      {/* Top→down flow: edges enter the top edge of a node and leave its bottom. */}
      <Handle type="target" position={Position.Top} className="!bg-[var(--border-default)]" />
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs font-semibold truncate" title={step.title}>
          {step.title}
        </span>
        <span
          className="text-[10px] px-1.5 py-[1px] rounded-full font-medium shrink-0"
          style={{ background: "rgba(0,0,0,0.08)" }}
        >
          {statusLabel(status)}
        </span>
      </div>
      <div className="mt-0.5 font-mono text-[10px] opacity-80 truncate" title={step.action}>
        {step.action}
      </div>
      <Handle type="source" position={Position.Bottom} className="!bg-[var(--border-default)]" />
    </div>
  );
});
StepNode.displayName = "StepNode";

const nodeTypes = { step: StepNode };

export default function WorkflowGraph({
  positioned,
  edges,
  view,
  selectedId,
  onSelect,
}: {
  positioned: PositionedStep[];
  edges: WfEdge[];
  view: Record<string, StepStatus>;
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  const nodes: Node<StepNodeData>[] = positioned.map((p) => ({
    id: p.step.id,
    type: "step",
    position: { x: p.x, y: p.y },
    data: { step: p.step, status: view[p.step.id] ?? "pending", selected: p.step.id === selectedId },
  }));

  const rfEdges: RfEdge[] = edges.map((e) => ({
    id: e.id,
    source: e.source,
    target: e.target,
    type: "smoothstep",
    animated: (view[e.target] ?? "pending") === "running",
    style: { stroke: "var(--border-light)", strokeWidth: 1.5 },
  }));

  return (
    <div className="w-full h-full">
      <ReactFlow
        nodes={nodes}
        edges={rfEdges}
        nodeTypes={nodeTypes}
        onNodeClick={(_, node) => onSelect(node.id)}
        onPaneClick={() => onSelect("")}
        fitView
        // A vertical pipeline is taller than the pane, so fit only down to a
        // readable zoom and let the rest be scrolled instead of shrunk away.
        fitViewOptions={{ minZoom: 0.6, maxZoom: 1, padding: 0.15 }}
        minZoom={0.4}
        zoomOnScroll={false}
        panOnScroll
        proOptions={{ hideAttribution: true }}
      >
        <Background variant={BackgroundVariant.Dots} gap={16} size={1} color="var(--border-default)" />
        <Controls showInteractive={false} />
      </ReactFlow>
    </div>
  );
}
