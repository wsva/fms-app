# Online Dataset Market — website API contract & app-side design

Status: **app side implemented; website side implemented 2026-10-19** (`learning/fms` now serves `GET /api/v1/market`, `POST|DELETE /api/v1/market/{uuid}/subscribe`, `PATCH /api/v1/market/{uuid}`, all backed by the shared `src/lib/datasets/market.ts` core, and advertises `"market"` in `WEBSITE_FEATURES` — rich mode activates on the next deploy).

The market lets *any* workspace — including one running as `role = "hub"` — browse, subscribe to, and download datasets shared on the website. It is not a new sync zone: the Datasets Sync **Pull tab** renders the market whenever the effective origin is the cloud (`pc_url` pointing at the website, or no `pc_url` with a signed-in account — exactly the backend's `sync::client::auth::hub_base_or_account` order). The authority-only paths (inbox drain, handover, journal append/prune, mirror push) stay `role=="hub"` gated; market *consumption* is role-agnostic.

## 1. Website REST API (spec — to implement in the website repo)

All endpoints are Bearer-auth (`Authorization: Bearer <access_token>`), gated by the same `resolveDatasetUser` the existing `/api/v1/datasets` routes use. JSON in/out. Errors carry a plain-text message the app surfaces verbatim.

### 1.1 `GET /api/v1/market`

Query params (all optional): `type=<t>` (repeatable — one per dataset type: `dictation|card|book|read_aloud|wiki`), `tag=<uuid>`, `q=<text>`.

Returns an array of rows:

```json
{
  "uuid": "...", "name": "...", "description": "...",
  "dataset_type": "book", "visibility": "shared", "language": "en",
  "is_owner": false, "is_subscribed": true,
  "owner": "a@example.com",
  "tags": [{ "uuid": "...", "name": "German" }],
  "media_count": 3, "card_count": null,
  "total_bytes": 123456, "file_count": 17,
  "updated": "<descriptorStamp codec — same as the lite list, no text comparison across shapes>",
  "published_version": 4
}
```

Semantics:

- Backed by the existing `/dataset/market` surface (`listMarketDatasets`): registry reads of each dataset's published version + `dataset_subscription` state for the calling principal.
- `published_version` = `dataset.current_version` (the live pin; null when never published).
- `visibility='private'` rows are listed **only to their owner** — this is what makes "shared by this user" a pure client-side `is_owner` partition with no second call.
- `is_owner` / `is_subscribed` are computed on the server's own principal. (The app falls back to an email match against `owner`/`user_id` when an older host omits the flags.)

### 1.2 Subscribe / unsubscribe

- `POST /api/v1/market/{uuid}/subscribe` — insert the caller's `dataset_subscription` row (mirror the existing server actions `subscribeDataset` / `unsubscribeDataset`).
- `DELETE /api/v1/market/{uuid}/subscribe` — remove it.
- Both return `{ "ok": true, "subscribed": true|false }` and **404** on an unknown uuid or one whose visibility makes it invisible to the caller.
- Subscribing moves no bytes; unsubscribing never deletes anything the app already adopted (evict/forget are separate explicit app-side actions).

### 1.3 `PATCH /api/v1/market/{uuid}` — owner visibility toggle

Body `{ "visibility": "private" | "shared" | "public" }`. Owner-only (403 otherwise, 400 on an unknown value). The response **echoes the catalog row** (in the §1.1 shape, including `current_version`) so the app can refresh its stored publish pin through the existing publish-echo seam (`sync/upload.rs::record_publish_echo`).

### 1.4 Capability advertisement

`GET /api/v1/cluster` gains `"market"` in its capability list (and `"registry_reads"` if not yet advertised). The app defaults to **degraded mode** when `market` is absent (INV-20 default-deny — never treat an unadvertised route as a success path).

## 2. App-side implementation (this repo)

- `src-tauri/src/sync/client/market.rs` — commands `market_list`, `market_subscribe`, `market_unsubscribe`, `market_set_visibility`, `market_whoami` (registered in `lib.rs` for desktop **and** the Android thin client; MCP twins in `mcp/sync.rs`). Compiles on all platforms (reqwest + serde_json + rusqlite only).
- `market_list` returns `{ datasets, downloads, published, origin, lite, market_available, can_subscribe, signed_in, degraded_reason }`: catalog rows enriched with the local sync bucket (`downloaded` / `needs_resync` / `not_downloaded`, plus `withdrawn`), this device's registry adoptions, and its upload history. Rows keep `is_owner`/`is_subscribed` unfiltered — the frontend partitions the Pull tab's three market sections (Browse / My Downloads / My Published).
- **Degradation (today's website):** without the `market` capability the list falls back to `GET /api/v1/datasets?lite=1` (public/self only, filtering already applied server-side), rows marked `lite: true`, `is_owner` decided by an email match on the stored token. No description/tags/subscribe/visibility controls in this mode — downloading shared/public datasets still works end-to-end.
- **Download is not reimplemented:** adoption rides the existing `dataset_sync_snapshot` (registry manifest/snapshot when `registry_reads` is advertised, else `/api/v1/datasets/{uuid}/snapshot`), installing read-only under `download/<type>/<uuid>` with `upstream_kind='registry'`. Prune rides `dataset_evict`.
- **owned-locally guard** (`sync/client/snapshot.rs::owned_locally_conflict`, checked in `dataset_sync_snapshot` before a website adoption): a uuid that resolves to a writable copy *outside* the download root is served by this workspace — market adoption is refused with `"owned-locally: dataset {uuid} is served by this workspace (writable copy at {path}) — market adoption refused; export it to the cloud via Publish instead"`. Mirrors the website upload route's `owned-by-other` 409 rule and protects a hub's served data.

## 3. Conformance vectors (add to the suite once the routes exist)

- `GET /api/v1/market` unauthenticated → 401; with Bearer → array as §1.1.
- Private row of another user → absent; own private row → present with `is_owner: true`.
- `POST …/subscribe` on a `shared` dataset → `{ok:true,subscribed:true}`; repeat → idempotent; on an invisible/unknown uuid → 404.
- `DELETE …/subscribe` → `{ok:true,subscribed:false}`.
- `PATCH …/{uuid}` non-owner → 403; `"Public"` → 400; valid → echo row.
- `/api/v1/cluster` capabilities contain `"market"` after deploy.

## 4. Open notes

- The website's own `/dataset/market` UI and these REST routes must stay a single source (`listMarketDatasets` backing both) — do not fork the visibility/subscription semantics.
- Until deploy, no app-side conformance vectors can run against the rich routes; the degraded lite path is covered by unit tests in `sync/client/market.rs` (row mapping, query encoding, sync-state bucket).
