# Dataset Directory Separation

## Summary

Implemented separation of card and dictation datasets into distinct subdirectories under the main datasets directory.

## New Directory Structure

```
workspace/
└── datasets/
    ├── card/
    │   ├── meta.json          # Linked directories for card datasets
    │   └── <card-datasets>/
    └── dictation/
        ├── meta.json          # Linked directories for dictation datasets
        └── <dictation-datasets>/
```

## Backend Changes

### 1. DatasetType Enum (`src-tauri/src/dataset.rs`)
- Added `DatasetType` enum with `Card` and `Dictation` variants
- Updated `dataset_roots()` to accept `DatasetType` parameter
- Function now looks in `datasets/{type}/` subdirectory based on type
- Each type has its own `meta.json` for linked directories

### 2. Updated Callers
- **dataset.rs**: All dictation dataset functions now use `DatasetType::Dictation`
- **cards.rs**: All card dataset functions now use `DatasetType::Card`
- **mcp.rs**: Updated `dataset_list_dirs` MCP tool to accept `dataset_type` parameter

### 3. dataset_list_dirs Command
- Now requires `dataset_type` parameter ("card" or "dictation")
- Returns directories specific to that dataset type
- Each type maintains its own linked directories

## Frontend Changes

### Updated Callers
1. **CardsPage.tsx**: Passes `datasetType: "card"` to `dataset_list_dirs`
2. **useDictationData.ts**: Passes `datasetType: "dictation"` to `dataset_list_dirs`
3. **StudioPage.tsx**: Passes `datasetType: "dictation"` to `dataset_list_dirs`

## Migration

No automatic migration code was written. Users need to manually move existing datasets:

**Before:**
```
datasets/
├── <dictation-datasets>/
└── <card-datasets>/
```

**After:**
```
datasets/
├── dictation/
│   └── <dictation-datasets>/
└── card/
    └── <card-datasets>/
```

Users should:
1. Create `datasets/card/` and `datasets/dictation/` directories
2. Move card datasets to `datasets/card/`
3. Move dictation datasets to `datasets/dictation/`
4. If using linked directories, create separate `meta.json` files in each subdirectory

## Benefits

- **Clear organization**: Card and dictation datasets are clearly separated
- **Independent management**: Each type can have its own linked directories
- **Scalable**: Easy to add more dataset types in the future
- **Cleaner backups**: Can backup/migrate specific dataset types independently

## Testing

- Backend compiles successfully with `cargo check --offline`
- Frontend compiles successfully with `tsc --noEmit` (only pre-existing error in SentenceDrawer.tsx)
- All callers updated to pass dataset type parameter
