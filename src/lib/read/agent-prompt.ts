/**
 * Compose the copy/paste prompt that hands an outstanding artifact job to a
 * third-party MCP agent (WorkBuddy, goose, any MCP client). FmS is attach-only:
 * it cannot call the agent, so the learner carries the request over by hand.
 * The string names the exact job and the write that fulfils it, mirroring the
 * `book_list_artifact_jobs` / `book_save_artifact` contract.
 */
export function buildArtifactJobPrompt(opts: {
  bookUuid: string;
  scopeType: string;
  scopeUuid: string;
  kind: string;
  instructions?: string;
  /** The existing artifact's uuid when regenerating, so the agent updates it in
   *  place instead of adding a duplicate the panel would never show. */
  existingUuid?: string;
}): string {
  const { bookUuid, scopeType, scopeUuid, kind, instructions, existingUuid } = opts;
  const guidance = instructions?.trim();
  const uuid = (existingUuid ?? "").trim();
  const output =
    kind === "questions"
      ? 'a JSON array serialized as a string, e.g. [{"type":"mcq","stem":"…","choices":["…"],"correct":0,"explanation":"…"}] or [{"type":"open","stem":"…","model_answer":"…","explanation":"…"}]'
      : "Markdown";

  const lines = [
    'Using the "fms" MCP server, fulfil this FmS reading request.',
    "",
    `Book UUID: ${bookUuid}`,
    `Requested: kind=${kind}, scope_type=${scopeType}, scope_uuid=${scopeUuid}`,
  ];
  if (guidance) lines.push(`Additional guidance: ${guidance}`);
  lines.push(
    "",
    "Steps:",
    `1. Call book_list_artifact_jobs with {"book_uuid":"${bookUuid}"} and pick the job whose scope_uuid is "${scopeUuid}" and kind is "${kind}". Read its source_text (and its instructions, if present).`,
    `2. Generate the ${kind} from that source_text with your own model. Output ${output}.`,
    "3. Deliver it with book_save_artifact using this shape:",
    `   {"book_uuid":"${bookUuid}","artifact":{"uuid":"${uuid}","book_uuid":"${bookUuid}","scope_type":"${scopeType}","scope_uuid":"${scopeUuid}","kind":"${kind}","content":"<generated content>","source":"agent","model":"<your model>","created_at":"","updated_at":"","content_hash":""}}`,
    '   Keep content_hash as "" (the app computes staleness itself).',
    "4. Call book_list_artifact_jobs again and confirm the job is gone (count 0).",
  );
  if (uuid) {
    lines.push("", "This replaces an existing artifact, so keep its uuid above — that updates it in place.");
  } else {
    lines.push("", 'Set uuid to "" to create a new artifact (none exists for this scope and kind yet).');
  }
  return lines.join("\n");
}
