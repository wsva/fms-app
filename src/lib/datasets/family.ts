// ---------------------------------------------------------------------------
// The `<type>_<family key>` dataset id relation — types, and nothing that
// decides what an id means.
//
// The grammar lives once, in `src-tauri/src/datasets/ids.rs`. Nothing here
// parses an id: the backend answers `dataset_family` with the members it
// actually found on disk, so the frontend never has to agree with Rust about
// what a key looks like, and a legacy id (a random uuid, `de_a1_by_system`, the
// reserved `dictation-favorites`) needs no special case beyond `family === null`.
//
// A family is a relation, not an entity — no registry file, no table, no
// `info.json` field — which is why every field below is derived per call and
// cannot go stale the way a stored family would.
// ---------------------------------------------------------------------------

/** The directory slug a dataset type lives under: `DatasetType::as_str()`. */
export type DatasetTypeSlug = "dictation" | "card" | "book" | "read_aloud" | "wiki";

/** One dataset of a family. Mirrors `datasets::registry::FamilyMember`. */
export interface FamilyMember {
  uuid: string;
  /** The type the dataset was *found* under, which is authoritative. The prefix
   *  in `uuid` is only a claim, and a re-typed dataset keeps a stale one. */
  dataset_type: DatasetTypeSlug;
  name: string;
  path: string;
  /** True when it resolves under the reserved read-only download root. */
  is_read_only?: boolean;
}

/** The whole family picture in one read. Mirrors `datasets::registry::FamilyReport`. */
export interface FamilyReport {
  /** `null` for a dataset whose id predates the grammar. `members` then holds
   *  that one dataset: an unaffiliated dataset is a family of one, so callers
   *  never need the special case. */
  family: string | null;
  members: FamilyMember[];
  /** The book member's uuid, when the family has one — what a derived page's
   *  breadcrumb links back to. */
  anchor: string | null;
  /** The book member's name, else the alphabetically-first member's. A family
   *  owns no name of its own, so this is always borrowed. */
  display_name: string | null;
}

/** The member of `report` whose type is `slug`, when the family has one. */
export function familyMember(
  report: FamilyReport | null | undefined,
  slug: DatasetTypeSlug
): FamilyMember | undefined {
  return report?.members.find((m) => m.dataset_type === slug);
}
