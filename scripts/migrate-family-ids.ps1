#Requires -Version 5.1
<#
.SYNOPSIS
    Re-key fms-app datasets onto the family-keyed id scheme.

.DESCRIPTION
    Rewrites each dataset's `info.json` uuid to `<type-slug>_<family-key>`, the
    grammar owned by `src-tauri/src/datasets/ids.rs` (see
    docs/design/dataset-info.md).

    A family is a set of datasets that belong to one work. Membership is read off
    the collection, never invented, from three kinds of evidence:

      * `cards_of_book_<uuid>` lineage: a card id names the dataset it was derived
        from, so the deck joins that dataset's family.
      * the same name: the nine `DE_A1`…`DE_B2_LB` dictation sets and their
        read-aloud counterparts are the same book's audio, spelled identically
        once case, separators and punctuation are folded.
      * `$Aliases` below: the handful of works a human has to identify because the
        deck is named by an abbreviation ("GgN", "de: KTdS") and only its
        `description` names the work.

    The key is the first 12 hex chars of the SHA-256 of the family's *canonical
    name* — the name of its anchor, where the anchor is the most text-bearing
    member (book > dictation > read_aloud > card > wiki, then oldest). Deriving
    the key from the name rather than from some member's uuid means it does not
    depend on which member happens to exist, and it makes the script idempotent:
    an id that already equals `<slug>_<key>` for its family is left alone.

    Two datasets of one type in one family are the escape hatch the grammar
    reserves: the first keeps `<slug>_<key>`, the rest get `_2`, `_3`, …

    Every reference to an old uuid is rewritten with it: every text column ending
    in `_uuid` in every app database (progress, XP, review events, sync state,
    the change journal, upload state) and in each touched dataset's own database
    (a book stores its id in `book_*.book_uuid`, a read-aloud set in
    `read_text.dataset_uuid`, a card its lineage in `source_dataset_uuid`), plus
    the workspace `simple_words.json`, which names a card dataset in plain JSON.
    Matching is by value, so nothing is assumed about column naming.

    Roots are discovered per type: `<datasets_dir>/<type>` for every workspace,
    plus that type's linked directories from `<datasets_dir>/<type>/meta.json` —
    a linked directory is itself a type root, which is why a linked
    `…\fms_datasets\dictation` is not scanned as `…\dictation\dictation`.

    The app must be closed: it caches `info.json` and holds `app.sqlite3`.

.PARAMETER Root
    Datasets directories to scan instead of auto-discovery (each is treated as a
    `<datasets>/<type>/…` parent, with its per-type `meta.json` links).

.PARAMETER WorkspaceRoot
    Where workspaces live. Defaults to `%APPDATA%\fms-app\workspaces`.

.PARAMETER Apply
    Actually write. Without this the script only prints the plan.

.PARAMETER Force
    Proceed even though fms-app is running. Not recommended - the app will write
    its cached state back over the migration.

.PARAMETER Resume
    Finish an interrupted `-Apply` from its backup directory. Only the reference
    rewriting is replayed (every database and `simple_words.json`), from the
    `mapping.json` that run wrote. Descriptors are not touched: the ids they
    already carry are the truth, and re-deriving them would re-derive the family
    grouping from evidence that the first run consumed (`cards_of_book_*` ids are
    gone by then).

.EXAMPLE
    .\migrate-family-ids.ps1
    Dry run against every discovered root: prints the families and the plan.

.EXAMPLE
    .\migrate-family-ids.ps1 -Apply
    Migrates, after backing up every touched info.json and app database.
#>
[CmdletBinding()]
param(
    [string[]] $Root,
    [string]   $WorkspaceRoot = (Join-Path $env:APPDATA 'fms-app\workspaces'),
    [switch]   $Apply,
    [switch]   $Force,
    [string]   $Resume
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Dataset names are not ASCII ("Die Känguru - Offenbarung"): without this the
# console round-trip through a pipe mangles them on Windows PowerShell 5.1.
try { [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch { }

$script:ReservedId = 'dictation-favorites'
$script:TypeSlugs  = @('book', 'dictation', 'read_aloud', 'card', 'wiki')
# Which member names the family. The text-bearing types outrank the derived ones,
# so a deck called "GgN" does not name the work its read-aloud text spells out.
$script:AnchorPriority = @{ book = 0; dictation = 1; read_aloud = 2; card = 3; wiki = 4 }
$script:Formats = @{
    dictation  = 'dictation-v2'
    card       = 'card-v1'
    book       = 'book-v1'
    read_aloud = 'read_aloud-v1'
    wiki       = 'wiki-v1'
}
# Canonical name key -> the name keys that mean the same work, with the evidence.
# Only entries a rule cannot see belong here; everything else is derived.
$script:Aliases = @{
    # card GgN: description "Gut gegen Nordwind"
    'gut_gegen_nordwind'                     = @('ggn')
    # card "de: KTdS": description "Klassische Texte der Staatsphilosophie : Norbert Hoerster"
    'klassische_texte_der_staatsphilosophie' = @('de_ktds')
    # The C1 course: the Kursbuch dictation set owns the family; the read-aloud
    # texts and the two media folders that have no descriptor yet are its parts.
    'de_c1_kb'                               = @('de_c1', 'de_c1_ab', 'de_c1_extra')
}
# Media folders to turn into datasets (folder leaf name). A raw folder has no
# identity to preserve, so it is written with its final family id directly
# instead of being initialized and re-keyed. Anything not listed here is only
# reported - initializing a folder is a content decision, not a migration one.
$script:InitRaw = @('de_c1_ab', 'de_c1_extra')
$script:Stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$script:Now   = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

function Get-FileText([string] $Path) {
    # The only encoding-safe read here: Get-Content in Windows PowerShell 5.1
    # defaults to the ANSI codepage and turns "Känguru" into "KÃ¤nguru".
    [IO.File]::ReadAllText($Path)
}

function Write-FileText([string] $Path, [string] $Text) {
    # LF, no BOM - matches what the Rust side writes.
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($Path, $Text, $utf8NoBom)
}

function Get-SqlitePath {
    $cmd = Get-Command sqlite3 -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    # fms-app bundles SQLite, but the CLI is not guaranteed on PATH. The Android
    # SDK ships one on this machine; fall through to a clear error.
    $guess = Join-Path $env:LOCALAPPDATA 'Android\Sdk\platform-tools\sqlite3.exe'
    if (Test-Path $guess) { return $guess }
    throw 'sqlite3.exe not found on PATH. Install the SQLite CLI, or pass a machine that has one.'
}

function Invoke-Sqlite([string] $Db, [string] $Sql) {
    # The database is named by its bare file name from inside its own directory.
    # sqlite3.exe decodes its arguments in the ANSI codepage, so a path holding
    # "vögelfrei" arrives as "v?gelfrei" and the open fails; a relative name
    # never crosses that conversion.
    $dir = Split-Path -Parent $Db
    Push-Location -LiteralPath $dir
    try {
        & $script:Sqlite -batch -bail (Split-Path -Leaf $Db) $Sql
        if ($LASTEXITCODE -ne 0) { throw "sqlite3 failed ($LASTEXITCODE) on $Db`n$Sql" }
    }
    finally { Pop-Location }
}

function Invoke-SqliteScript([string] $Db, [string] $Sql) {
    # A full sweep is hundreds of UPDATEs; passing them as one argument would hit
    # the Windows command-line limit (~32k chars) on a library this size.
    $tmp = [IO.Path]::GetTempFileName()
    $dir = Split-Path -Parent $Db
    Push-Location -LiteralPath $dir
    try {
        Write-FileText $tmp $Sql
        & $script:Sqlite -batch -bail (Split-Path -Leaf $Db) (".read `"$tmp`"")
        if ($LASTEXITCODE -ne 0) { throw "sqlite3 failed ($LASTEXITCODE) running $tmp on $Db" }
    }
    finally {
        Pop-Location
        Remove-Item $tmp -Force -ErrorAction SilentlyContinue
    }
}

function Get-SqliteTables([string] $Db) {
    (Invoke-Sqlite $Db "select name from sqlite_master where type='table' and name not like 'sqlite_%';") |
        Where-Object { $_ } | ForEach-Object { $_.Trim() }
}

function Get-TableColumns([string] $Db, [string] $Table) {
    (Invoke-Sqlite $Db "pragma table_info('$Table');") | Where-Object { $_ } | ForEach-Object {
        $parts = $_ -split '\|'
        [pscustomobject]@{ Name = $parts[1]; Type = $parts[2] }
    }
}

function Repoint-DatasetUuids([string] $Db, [object[]] $Moves) {
    # Value-based on purpose: a dataset id can sit in a column named for its own
    # type (`book_chapter.book_uuid`), in `dataset_uuid`, or in a card's
    # `source_dataset_uuid`. Every text column ending in `_uuid` is swept and only
    # rows whose value is exactly an old id change - a row uuid never matches one.
    $sql = New-Object System.Collections.Generic.List[string]
    $sql.Add('BEGIN;')
    $touched = 0
    foreach ($table in Get-SqliteTables $Db) {
        foreach ($col in @(Get-TableColumns $Db $table | Where-Object { $_.Name -like '*_uuid' })) {
            foreach ($m in $Moves) {
                $old = ([string]$m.old_id) -replace "'", "''"
                $new = ([string]$m.new_id) -replace "'", "''"
                $sql.Add("UPDATE `"$table`" SET `"$($col.Name)`"='$new' WHERE `"$($col.Name)`"='$old';")
            }
            $touched++
        }
    }
    if ($touched -eq 0) { return 0 }
    $sql.Add('COMMIT;')
    Invoke-SqliteScript $Db ($sql -join "`n")
    $touched
}

function Update-EmbeddedUuids([string] $Db, [object[]] $Pairs) {
    # Some ids sit where no `_uuid` column can be matched:
    #
    #   * `tag.uuid` - a tag's identity is seeded from the dataset the cards came
    #     with, so a deck's tag carries that deck's uuid. `card_tag.tag_uuid` does
    #     end in `_uuid` and was rewritten, so leaving `tag.uuid` alone orphans
    #     every tagged card (PRAGMA foreign_key_check says so).
    #   * change payloads - `sync_log.payload` and the writeback queue carry the
    #     dataset uuid inside their JSON, and the follower routes by it.
    #
    # A whitelist rather than every text column: card fronts/backs and wiki prose
    # are content, and `card_fts` is maintained by hand, so editing content would
    # leave the search index stale.
    $targets = @(
        @{ Table = 'tag'; Column = 'uuid' },
        @{ Table = 'sync_log'; Column = 'payload' },
        @{ Table = 'writeback_queue'; Column = 'payload' },
        @{ Table = 'agent_mail'; Column = 'payload_json' },
        @{ Table = 'agent_mail'; Column = 'body' },
        @{ Table = 'agent_mail'; Column = 'subject' },
        @{ Table = 'agent_mail'; Column = 'resource' },
        @{ Table = 'agent_mail'; Column = 'ref_id' }
    )
    $tables = @(Get-SqliteTables $Db)
    $sql = New-Object System.Collections.Generic.List[string]
    $sql.Add('BEGIN;')
    $done = 0
    foreach ($t in $targets) {
        if (-not ($tables -contains $t.Table)) { continue }
        $cols = @(Get-TableColumns $Db $t.Table | ForEach-Object { $_.Name })
        if (-not ($cols -contains $t.Column)) { continue }
        # One statement per column with the replacement nested once per pair, so
        # the table is scanned once instead of once per id.
        $expr = "`"$($t.Column)`""
        $where = New-Object System.Collections.Generic.List[string]
        foreach ($p in $Pairs) {
            $old = ([string]$p.old_id) -replace "'", "''"
            $new = ([string]$p.new_id) -replace "'", "''"
            $expr = "replace($expr, '$old', '$new')"
            $where.Add("instr(`"$($t.Column)`", '$old')>0")
        }
        $sql.Add("UPDATE `"$($t.Table)`" SET `"$($t.Column)`" = $expr WHERE " + ($where -join ' or ') + ';')
        $done++
    }
    if ($done -eq 0) { return 0 }
    $sql.Add('COMMIT;')
    Invoke-SqliteScript $Db ($sql -join "`n")
    $done
}

function Update-References([object[]] $Pairs, [string[]] $DatasetDirs, [string] $BackupDir) {
    # Everything that writes an old uuid down, in one function so a resumed run
    # can replay exactly this and nothing else: every workspace app database,
    # every touched dataset's own database, and the workspace `simple_words.json`.
    $appDbs = @(Get-ChildItem $WorkspaceRoot -Directory |
        ForEach-Object { Join-Path $_.FullName 'app.sqlite3' } | Where-Object { Test-Path $_ })
    $datasetDbs = @($DatasetDirs | ForEach-Object { Join-Path $_ 'data.sqlite3' } | Where-Object { Test-Path $_ })

    foreach ($db in ($appDbs + $datasetDbs)) {
        $label = $db -replace '[^\w\.-]', '_'
        if ($db -like '*app.sqlite3') {
            $label = (Split-Path (Split-Path $db -Parent) -Leaf) + '--app.sqlite3'
        }
        # Never overwrite an existing backup: on -Resume the copy already taken
        # before the first write is the only pristine state left.
        $bak = Join-Path $BackupDir $label
        if (-not (Test-Path $bak)) { Copy-Item $db $bak }
        $columns = Repoint-DatasetUuids $db $Pairs
        $embedded = Update-EmbeddedUuids $db $Pairs
        Write-Host ("  db       {0}  ({1} uuid column(s), {2} payload column(s))" -f $db, $columns, $embedded)
    }

    # Plain-JSON references the databases do not cover: `simple_words.json` names
    # the card datasets that count as "known simple words" per language.
    $replacements = @{}
    foreach ($p in $Pairs) { $replacements['"' + $p.old_id + '"'] = '"' + $p.new_id + '"' }
    foreach ($ws in (Get-ChildItem $WorkspaceRoot -Directory)) {
        $sw = Join-Path $ws.FullName 'simple_words.json'
        if (-not (Test-Path $sw)) { continue }
        $text = Get-FileText $sw
        $before = $text
        foreach ($from in $replacements.Keys) { $text = $text.Replace($from, $replacements[$from]) }
        if ($text -ne $before) {
            $bak = Join-Path $BackupDir ($ws.Name + '--simple_words.json')
            if (-not (Test-Path $bak)) { Copy-Item $sw $bak }
            Write-FileText $sw $text
            Write-Host ("  json     {0}" -f $sw)
        }
    }
}

function Get-ShaKey([string] $Value) {
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $bytes = $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($Value))
        return (($bytes | ForEach-Object { $_.ToString('x2') }) -join '').Substring(0, 12)
    }
    finally { $sha.Dispose() }
}

function Get-NameKey([string] $Name) {
    # Fold the spellings the collection uses for one piece of material: `DE_B1.1_AB`
    # as a dictation and `de_b1.1_ab` as a read-aloud are the same book, and
    # `Verity(en)` is the same work as `verity_en`. Only an exact match after this
    # is a sibling, so `word_de(A1)` stays distinct from `word_de(A2)`.
    $n = $Name.ToLowerInvariant()
    $n = $n -replace '[^\p{L}\p{Nd}]+', '_'
    $n = $n -replace '_by_system$', ''
    $n = $n -replace '_cards$', ''
    $n = $n -replace '_card$', ''
    $n.Trim('_')
}

function Get-FamilyNameKey([string] $Name) {
    $k = Get-NameKey $Name
    if ($script:AliasTarget.ContainsKey($k)) { return $script:AliasTarget[$k] }
    $k
}

function Get-KeyedFamily([string] $Slug, [string] $Id) {
    # Reads the family key out of an id that already follows the grammar
    # (`card_43d880c45159`, `dictation_190c8944c5f1_2`). Legacy ids - a bare 32-hex
    # uuid, `de_a1_by_system`, `cards_of_book_<uuid>`, `dictation-favorites` -
    # return nothing, which is what makes the first run derive keys from names and
    # every run after it read them back.
    if (-not $Id) { return $null }
    $prefix = $Slug + '_'
    if (-not $Id.StartsWith($prefix)) { return $null }
    $key = ($Id.Substring($prefix.Length) -split '_')[0]
    if ($key -notmatch '^[0-9a-f]{8,32}$') { return $null }
    $key
}

function Get-LinkedRoots([string] $TypeDir) {
    $meta = Join-Path $TypeDir 'meta.json'
    if (-not (Test-Path $meta)) { return @() }
    $m = (Get-FileText $meta) | ConvertFrom-Json
    if (-not ($m.PSObject.Properties.Name -contains 'linked_dirs')) { return @() }
    @($m.linked_dirs | Where-Object { $_ -and $_.path } | ForEach-Object { [string]$_.path })
}

function Get-DatasetsDirs {
    $dirs = New-Object System.Collections.Generic.List[string]
    if ($Root) {
        foreach ($r in $Root) { $dirs.Add((Resolve-Path $r).Path) }
        return $dirs
    }
    $globalSettings = Join-Path $env:APPDATA 'fms-app\settings.json'
    if (Test-Path $globalSettings) {
        $s = (Get-FileText $globalSettings) | ConvertFrom-Json
        if (($s.PSObject.Properties.Name -contains 'datasets_dir') -and $s.datasets_dir) {
            $dirs.Add([string]$s.datasets_dir)
        }
    }
    if (Test-Path $WorkspaceRoot) {
        foreach ($ws in Get-ChildItem $WorkspaceRoot -Directory) {
            $wsSettings = Join-Path $ws.FullName 'settings.json'
            if (-not (Test-Path $wsSettings)) { continue }
            $s = (Get-FileText $wsSettings) | ConvertFrom-Json
            if (($s.PSObject.Properties.Name -contains 'datasets_dir') -and $s.datasets_dir) {
                $dirs.Add([string]$s.datasets_dir)
            }
        }
    }
    $dirs | Where-Object { $_ -and (Test-Path $_) } | ForEach-Object { (Resolve-Path $_).Path } | Sort-Object -Unique
}

function Get-TypeRoots([string[]] $DatasetsDirs) {
    $out = New-Object System.Collections.Generic.List[object]
    $seen = @{}
    foreach ($dd in $DatasetsDirs) {
        foreach ($slug in $script:TypeSlugs) {
            $typeDir = Join-Path $dd $slug
            if (Test-Path $typeDir) {
                $p = (Resolve-Path $typeDir).Path
                if (-not $seen.ContainsKey("$slug|$p")) {
                    $seen["$slug|$p"] = $true
                    $out.Add([pscustomobject]@{ Slug = $slug; Path = $p })
                }
            }
            foreach ($linked in (Get-LinkedRoots $typeDir)) {
                if (-not (Test-Path $linked)) {
                    # Common enough to be worth naming: a location that was moved
                    # away leaves its link behind, and the app silently skips it.
                    Write-Warning "stale $slug link in $(Join-Path $typeDir 'meta.json'): $linked"
                    continue
                }
                $p = (Resolve-Path $linked).Path
                if (-not $seen.ContainsKey("$slug|$p")) {
                    $seen["$slug|$p"] = $true
                    $out.Add([pscustomobject]@{ Slug = $slug; Path = $p })
                }
            }
        }
    }
    $out
}

function Get-Datasets([object[]] $TypeRoots) {
    $out = New-Object System.Collections.Generic.List[object]
    foreach ($tr in $TypeRoots) {
        foreach ($dir in (Get-ChildItem $tr.Path -Directory | Sort-Object Name)) {
            $infoPath = Join-Path $dir.FullName 'info.json'
            if (-not (Test-Path $infoPath)) {
                if (-not (Test-Path (Join-Path $dir.FullName 'media'))) { continue }
                if (-not ($script:InitRaw -contains $dir.Name)) {
                    Write-Warning "media folder with no info.json, left out (initialize it in the app): $($dir.FullName)"
                    continue
                }
                $out.Add([pscustomobject]@{
                    Kind     = 'raw'
                    Slug     = $tr.Slug
                    Name     = $dir.Name
                    Created  = $script:Now   # sorts last: it is the newest member
                    Dir      = $dir.FullName
                    DirName  = $dir.Name
                    InfoPath = $infoPath
                    OldId    = ''
                    NameKey  = Get-FamilyNameKey $dir.Name
                    KeyedKey = $null
                    Family   = $null
                    Key      = $null
                    NewId    = $null
                    Action   = $null
                    Reason   = $null
                })
                continue
            }
            $info = (Get-FileText $infoPath) | ConvertFrom-Json
            if (-not $info.uuid) {
                Write-Warning "skip (no uuid): $infoPath"
                continue
            }
            if ($info.type -and ([string]$info.type) -ne $tr.Slug) {
                # The directory is the routing authority (find_dataset_dir_typed
                # warns and skips on mismatch), so a re-key here would edit a
                # dataset the app cannot see.
                Write-Warning "skip (type '$($info.type)' in a '$($tr.Slug)' directory): $infoPath"
                continue
            }
            $out.Add([pscustomobject]@{
                Kind     = 'dataset'
                Slug     = $tr.Slug
                Name     = [string]$info.name
                Created  = [string]$info.created_at
                Dir      = $dir.FullName
                DirName  = $dir.Name
                InfoPath = $infoPath
                OldId    = [string]$info.uuid
                NameKey  = Get-FamilyNameKey ([string]$info.name)
                KeyedKey = Get-KeyedFamily $tr.Slug ([string]$info.uuid)
                Family   = $null
                Key      = $null
                NewId    = $null
                Action   = $null
                Reason   = $null
            })
        }
    }
    $out
}

function Set-InfoUuid([string] $Path, [string] $OldId, [string] $NewId) {
    # Surgical: rewrite the one line, keep key order, indentation and the
    # trailing newline exactly as they were. A round-trip through ConvertTo-Json
    # would reorder every key - a whole-file diff for a one-value edit.
    # Index arithmetic rather than a Regex MatchEvaluator: a scriptblock converted
    # to a delegate does not reliably see this function's locals.
    $text = Get-FileText $Path
    $pattern = '("uuid"\s*:\s*")' + [Regex]::Escape($OldId) + '(")'
    $m = [Regex]::Match($text, $pattern)
    if (-not $m.Success) { throw "could not find the uuid line in $Path" }
    $text = $text.Substring(0, $m.Groups[1].Index) + $m.Groups[1].Value + $NewId + $m.Groups[2].Value +
            $text.Substring($m.Groups[2].Index + $m.Groups[2].Length)

    # `updated_at` is what touch_info() bumps on any descriptor edit; leaving it
    # behind would make the migration look like a no-op to sync.
    $m = [Regex]::Match($text, '("updated_at"\s*:\s*")[^"]*(")')
    if ($m.Success) {
        $text = $text.Substring(0, $m.Groups[1].Index) + $m.Groups[1].Value + $script:Now + $m.Groups[2].Value +
                $text.Substring($m.Groups[2].Index + $m.Groups[2].Length)
    }
    Write-FileText $Path $text
}

function Write-NewInfo([string] $Path, [string] $Id, [string] $Slug, [string] $Name) {
    # What `dataset_init_dir` would write for this folder (DatasetInfo::new), with
    # the family-keyed id filled in: a raw folder has no history to preserve, so
    # there is no reason to mint a random id and immediately re-key it.
    $language = ''
    if ($Name -match '^([a-z]{2})_') { $language = $Matches[1] }
    $obj = [ordered]@{
        spec        = 2
        uuid        = $Id
        type        = $Slug
        format      = $script:Formats[$Slug]
        name        = $Name
        description = ''
        language    = $language
        created_at  = $script:Now
        updated_at  = $script:Now
        sharing     = [ordered]@{ visibility = 'private'; owner_id = ''; subscribers = @() }
    }
    $text = ($obj | ConvertTo-Json -Depth 4) -replace "`r`n", "`n"
    if (-not $text.EndsWith("`n")) { $text += "`n" }
    Write-FileText $Path $text
}

# ---------------------------------------------------------------------------
# Plan
# ---------------------------------------------------------------------------

$script:Sqlite = Get-SqlitePath
$script:AliasTarget = @{}
foreach ($canonical in $script:Aliases.Keys) {
    foreach ($alias in $script:Aliases[$canonical]) { $script:AliasTarget[$alias] = $canonical }
}

if (($Apply -or $Resume) -and -not $Force) {
    $running = Get-Process -Name fms-app, fms_app -ErrorAction SilentlyContinue
    if ($running) {
        throw "fms-app is running (PID $($running.Id -join ', ')). Close it first: it caches info.json and holds app.sqlite3, so it would write the old state back. Use -Force to override."
    }
}

if ($Resume) {
    # A run that died part-way already rewrote every descriptor and swept some
    # databases. Re-deriving ids now would regroup families from evidence the
    # first run consumed (`cards_of_book_*` ids no longer exist), so the ids in
    # mapping.json are replayed as-is instead.
    $mappingPath = Join-Path $Resume 'mapping.json'
    if (-not (Test-Path $mappingPath)) { throw "no mapping.json in $Resume" }
    # `ConvertFrom-Json` hands back a JSON array as one Object[], so it has to go
    # through a pipeline before `foreach` sees the entries rather than the array.
    $entries = @((Get-FileText $mappingPath | ConvertFrom-Json) | ForEach-Object { $_ })

    $pairs = New-Object System.Collections.Generic.List[object]
    $dirs = New-Object System.Collections.Generic.List[string]
    foreach ($e in $entries) {
        if ($e.action -ne 'rekey') { continue }
        $pairs.Add([pscustomobject]@{ old_id = [string]$e.old_id; new_id = [string]$e.new_id })
        $dir = [string]$e.dir
        if ($e.dir_renamed) { $dir = Join-Path (Split-Path -Path $dir -Parent) ([string]$e.new_id) }
        $dirs.Add($dir)
    }

    Write-Host ("Resuming {0}: {1} id(s) from {2}" -f $Resume, $pairs.Count, $mappingPath) -ForegroundColor Cyan
    if ($pairs.Count -gt 0) { Update-References $pairs.ToArray() $dirs.ToArray() $Resume }
    Write-Host ''
    Write-Host 'Done: references rewritten. Restart fms-app to pick up the new ids.' -ForegroundColor Green
    return
}

$datasetsDirs = @(Get-DatasetsDirs)
if (-not $datasetsDirs) { throw 'No dataset directories found. Pass -Root explicitly.' }
$typeRoots = @(Get-TypeRoots $datasetsDirs)

Write-Host 'Roots:' -ForegroundColor Cyan
$typeRoots | ForEach-Object { Write-Host ("  {0,-11} {1}" -f $_.Slug, $_.Path) }

$all = @(Get-Datasets $typeRoots)
if ($all.Count -eq 0) { Write-Host 'No datasets found.'; return }
Write-Host ("`nDatasets found: {0}" -f $all.Count)

# -- families --------------------------------------------------------------
# Union-find over dataset directories; two datasets are in one family when some
# piece of evidence says they are the same work.
$script:Parent = @{}
foreach ($d in $all) { $script:Parent[$d.Dir] = $d.Dir }

function Find-Root([string] $Key) {
    while ($script:Parent[$Key] -ne $Key) { $Key = $script:Parent[$Key] }
    $Key
}

function Join-Family([string] $A, [string] $B) {
    $ra = Find-Root $A
    $rb = Find-Root $B
    if ($ra -ne $rb) { $script:Parent[$rb] = $ra }
}

# 1. the same name (aliases already folded in by NameKey)
$byName = @{}
foreach ($d in $all) {
    if (-not $d.NameKey) { continue }
    if ($byName.ContainsKey($d.NameKey)) { Join-Family $byName[$d.NameKey] $d.Dir }
    else { $byName[$d.NameKey] = $d.Dir }
}

# 2. `cards_of_book_<uuid>` lineage
$byOldId = @{}
foreach ($d in $all) {
    if (-not $d.OldId) { continue }
    if ($byOldId.ContainsKey($d.OldId)) {
        Write-Warning "duplicate id '$($d.OldId)' (one dataset shadows the other): $($byOldId[$d.OldId])`n           and $($d.Dir)"
        Join-Family $byOldId[$d.OldId] $d.Dir
    }
    else { $byOldId[$d.OldId] = $d.Dir }
}
foreach ($d in $all) {
    if ($d.OldId -notlike 'cards_of_book_*') { continue }
    $source = $d.OldId.Substring('cards_of_book_'.Length)
    if ($byOldId.ContainsKey($source)) { Join-Family $byOldId[$source] $d.Dir }
    else { Write-Warning "card '$($d.Dir)' claims source '$source', which is not in any scanned root" }
}

# 3. the same family key already in the ids. This is the evidence a second run
#    has instead of lineage: the first run replaced `cards_of_book_<uuid>` with
#    `card_<key>`, so two datasets of one work are now paired by key alone.
$byFamKey = @{}
foreach ($d in $all) {
    if (-not $d.KeyedKey) { continue }
    if ($byFamKey.ContainsKey($d.KeyedKey)) { Join-Family $byFamKey[$d.KeyedKey] $d.Dir }
    else { $byFamKey[$d.KeyedKey] = $d.Dir }
}

$groups = @{}
foreach ($d in $all) {
    $r = Find-Root $d.Dir
    if (-not $groups.ContainsKey($r)) { $groups[$r] = New-Object System.Collections.Generic.List[object] }
    $groups[$r].Add($d)
}

# -- keys ------------------------------------------------------------------
$keyOwners = @{}
$families = New-Object System.Collections.Generic.List[object]
foreach ($rootKey in $groups.Keys) {
    # `.ToArray()` rather than `@(...)`: on Windows PowerShell 5.1, wrapping a
    # generic List in the array operator throws "argument types do not match"
    # under Set-StrictMode.
    $members = $groups[$rootKey].ToArray()
    # Explicit comparison rather than a multi-property Sort-Object: on Windows
    # PowerShell 5.1 that form throws "argument types do not match" when the
    # calculated properties are evaluated inside a script with Set-StrictMode.
    $anchor = $members[0]
    foreach ($m in $members) {
        $pa = [int]$script:AnchorPriority[$anchor.Slug]
        $pm = [int]$script:AnchorPriority[$m.Slug]
        $better = $false
        if ($pm -lt $pa) { $better = $true }
        elseif ($pm -eq $pa) {
            if ([string]$m.Created -lt [string]$anchor.Created) { $better = $true }
            elseif (([string]$m.Created -eq [string]$anchor.Created) -and ([string]$m.Name -lt [string]$anchor.Name)) { $better = $true }
        }
        if ($better) { $anchor = $m }
    }
    $canonical = $anchor.NameKey
    if (-not $canonical) {
        $canonical = 'family_' + (Get-ShaKey $anchor.Dir)
    }
    $key = Get-ShaKey $canonical
    # A key already carried by a member's id wins over the name-derived one: the
    # datasets may only be in this group because of it, and re-deriving would
    # rename every one of them again.
    $existing = @($members | Where-Object { $_.KeyedKey } | ForEach-Object { $_.KeyedKey } | Sort-Object -Unique)
    if ($existing.Count -gt 0) {
        if ($existing.Count -gt 1) {
            Write-Warning "family '$canonical' carries more than one key ($($existing -join ', ')); using $($existing[0])"
        }
        $key = [string]$existing[0]
    }
    if ($keyOwners.ContainsKey($key) -and $keyOwners[$key] -ne $canonical) {
        # Two works whose canonical names differ but hash into the same 12 hex
        # chars: re-derive from something that cannot repeat, and say so.
        $key = Get-ShaKey ("$canonical|$($anchor.Slug)|$($anchor.OldId)")
        Write-Warning "key collision on '$canonical' - re-derived key $key"
    }
    $keyOwners[$key] = $canonical

    # One dataset per type per family; the grammar's reserved discriminator is
    # the escape hatch for a second one, ordered oldest first so a re-run is a
    # no-op.
    foreach ($bySlug in ($members | Group-Object Slug)) {
        $ordered = @($bySlug.Group | Sort-Object { $_.Created + '|' + $_.Name })
        for ($i = 0; $i -lt $ordered.Count; $i++) {
            $m = $ordered[$i]
            $m.Family = $canonical
            $m.Key = $key
            if ($i -eq 0) {
                $m.NewId = "$($m.Slug)_$key"
            }
            else {
                $m.NewId = "$($m.Slug)_" + $key + "_$($i + 1)"
            }
        }
    }

    $families.Add([pscustomobject]@{
        Key       = $key
        Canonical = $canonical
        Anchor    = $anchor
        Members   = $members
    })
}

foreach ($d in $all) {
    if ($d.OldId -eq $script:ReservedId) {
        $d.Action = 'skip'; $d.Reason = 'reserved Favorites id'
        continue
    }
    if ($d.Kind -eq 'raw') { $d.Action = 'init'; continue }
    if ($d.OldId -eq $d.NewId) { $d.Action = 'skip'; $d.Reason = 'already keyed'; continue }
    $d.Action = 'rekey'
}

# A target that another dataset already answers to would make one of them
# invisible; refuse rather than merge two works.
foreach ($d in $all) {
    if ($d.Action -eq 'skip' -or -not $d.NewId) { continue }
    if ($byOldId.ContainsKey($d.NewId) -and $byOldId[$d.NewId] -ne $d.Dir) {
        throw "target id '$($d.NewId)' for $($d.Dir) is already used by $($byOldId[$d.NewId])"
    }
}

# -- report ----------------------------------------------------------------
Write-Host ''
foreach ($f in ($families | Sort-Object Canonical)) {
    $multi = @($f.Members | Where-Object { $_.Action -ne 'skip' })
    if ($multi.Count -eq 0) { continue }
    $mark = 'single'
    if ($f.Members.Count -gt 1) { $mark = 'family' }
    Write-Host ("  {0}  {1}  (key {2}, named by {3}/{4})" -f $mark, $f.Canonical, $f.Key, $f.Anchor.Slug, $f.Anchor.Name) -ForegroundColor Cyan
    foreach ($d in ($f.Members | Sort-Object { '{0}|{1}' -f $script:AnchorPriority[$_.Slug], $_.Name })) {
        if ($d.Action -eq 'skip') {
            Write-Host ("      {0,-7} {1,-10} {2,-40} {3}" -f 'skip', $d.Slug, $d.Name, $d.Reason) -ForegroundColor DarkGray
            continue
        }
        $was = '(no info.json yet)'
        if ($d.OldId) { $was = "(was $($d.OldId))" }
        Write-Host ("      {0,-7} {1,-10} {2,-40} -> {3}  {4}" -f $d.Action, $d.Slug, $d.Name, $d.NewId, $was)
    }
}

$rekey = @($all | Where-Object { $_.Action -eq 'rekey' })
$inits = @($all | Where-Object { $_.Action -eq 'init' })
$skips = @($all | Where-Object { $_.Action -eq 'skip' })
$familyCount = @($families | Where-Object { $_.Members.Count -gt 1 }).Count

Write-Host ''
Write-Host ("{0} dataset(s) in {1} families; {2} to re-key, {3} to initialize, {4} left alone." -f `
    $all.Count, $familyCount, $rekey.Count, $inits.Count, $skips.Count) -ForegroundColor Green

if ($rekey.Count -eq 0 -and $inits.Count -eq 0) {
    Write-Host 'Nothing to migrate.' -ForegroundColor Green
    return
}

if (-not $Apply) {
    Write-Host ''
    Write-Host 'Dry run: re-run with -Apply to write.' -ForegroundColor Yellow
    return
}

# ---------------------------------------------------------------------------
# Apply
# ---------------------------------------------------------------------------

$backupDir = Join-Path $env:APPDATA "fms-app\family-migration-$($script:Stamp)"
New-Item -ItemType Directory -Path $backupDir -Force | Out-Null

$moved = @($rekey + $inits)
$map = $moved | ForEach-Object {
    [pscustomobject]@{
        slug = $_.Slug; name = $_.Name; dir = $_.Dir
        old_id = $_.OldId; new_id = $_.NewId
        family = $_.Family; family_key = $_.Key
        action = $_.Action
        dir_renamed = ($_.DirName -eq $_.OldId)
    }
}
$map | ConvertTo-Json -Depth 4 | ForEach-Object { Write-FileText (Join-Path $backupDir 'mapping.json') $_ }
Write-Host ''
Write-Host "Backups and mapping: $backupDir" -ForegroundColor Cyan

# 1. Descriptors (and directory names).
foreach ($d in $inits) {
    Write-NewInfo $d.InfoPath $d.NewId $d.Slug $d.Name
    Write-Host ("  init     {0}  {1} -> {2}" -f $d.Slug, $d.DirName, $d.NewId)
}
foreach ($d in $rekey) {
    $leaf = Split-Path $d.Dir -Leaf
    $bak = Join-Path $backupDir ("{0}--{1}--info.json" -f $d.Slug, ($leaf -replace '[^\w\.-]', '_'))
    Copy-Item $d.InfoPath $bak
    Set-InfoUuid $d.InfoPath $d.OldId $d.NewId
    if ($d.DirName -eq $d.OldId) {
        $newDir = Join-Path (Split-Path $d.Dir -Parent) $d.NewId
        if (Test-Path $newDir) { throw "cannot rename $($d.Dir): $newDir already exists" }
        Rename-Item -Path $d.Dir -NewName $d.NewId
        $d.Dir = $newDir
    }
    Write-Host ("  rekeyed  {0}  {1} -> {2}" -f $d.Slug, $d.OldId, $d.NewId)
}

# 2. Every database that points at an old uuid. This is what keeps progress, XP,
#    review history, sync state and a book's own chapters attached instead of
#    orphaned.
$pairs = @($rekey | ForEach-Object { [pscustomobject]@{ old_id = $_.OldId; new_id = $_.NewId } })
if ($pairs.Count -gt 0) {
    Update-References $pairs @($rekey | ForEach-Object { $_.Dir }) $backupDir
}

Write-Host ''
Write-Host ("Done: {0} re-keyed, {1} initialized. Restart fms-app to pick up the new ids." -f $rekey.Count, $inits.Count) -ForegroundColor Green
