# Copies (READ ONLY) the eye tracking files of Pimax Play and the Tobii runtime for omarchyvr on Linux (2026-10-18):
# Tobii profiles and calibration (*.setpm and the rest), Pimax eye tracking / wear reminder settings, logs, the list of
# Tobii and Pimax services. No program files: .dll .exe .sys .msi .cab are never copied. Nothing is changed.
# Output: <this folder>\eye-files-<stamp>-<label>\ . Files may hold serials: they stay on this drive.
param([string]$Label = 'copy', [switch]$NoPause)
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$dest = Join-Path $PSScriptRoot "eye-files-$stamp-$Label"
New-Item -ItemType Directory -Force $dest | Out-Null
$log = Join-Path $dest "copy.log"
$skip = @('.dll', '.exe', '.sys', '.msi', '.cab', '.pdb', '.node')
$maxBytes = 50MB

function CopyTree([string]$root, [string]$label) {
    if (-not (Test-Path $root)) { Add-Content $log "absent: $root"; return }
    Add-Content $log "copying $root -> $label"
    Get-ChildItem -LiteralPath $root -Recurse -File -Force -ErrorAction SilentlyContinue | ForEach-Object {
        if ($skip -contains $_.Extension.ToLower()) { return }
        if ($_.Length -gt $maxBytes) { Add-Content $log ("too big, skipped: {0} ({1:N0} B)" -f $_.FullName, $_.Length); return }
        $rel = $_.FullName.Substring($root.Length).TrimStart('\')
        $to = Join-Path (Join-Path $dest $label) $rel
        New-Item -ItemType Directory -Force (Split-Path $to) | Out-Null
        try { Copy-Item -LiteralPath $_.FullName -Destination $to -Force -ErrorAction Stop }
        catch { Add-Content $log ("could not copy {0}: {1}" -f $_.FullName, $_.Exception.Message) }
    }
}

# Tobii runtime and Pimax data, machine-wide and per user
foreach ($d in (Get-ChildItem 'C:\ProgramData' -Directory -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'tobii|pimax' })) {
    CopyTree $d.FullName ("ProgramData\" + $d.Name)
}
foreach ($base in @($env:LOCALAPPDATA, $env:APPDATA)) {
    foreach ($d in (Get-ChildItem $base -Directory -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'tobii|pimax' })) {
        CopyTree $d.FullName ((Split-Path $base -Leaf) + "\" + $d.Name)
    }
}
# Pimax and Tobii install folders: settings only (json, ini, xml, cfg, config, txt, yaml)
foreach ($pf in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
    foreach ($d in (Get-ChildItem $pf -Directory -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'tobii|pimax' })) {
        Get-ChildItem -LiteralPath $d.FullName -Recurse -File -Force -ErrorAction SilentlyContinue |
            Where-Object { @('.json', '.ini', '.xml', '.cfg', '.config', '.txt', '.yaml', '.yml', '.setpm') -contains $_.Extension.ToLower() -and $_.Length -lt $maxBytes } |
            ForEach-Object {
                $rel = $_.FullName.Substring($pf.Length).TrimStart('\')
                $to = Join-Path (Join-Path $dest "ProgramFiles") $rel
                New-Item -ItemType Directory -Force (Split-Path $to) | Out-Null
                Copy-Item -LiteralPath $_.FullName -Destination $to -Force -ErrorAction SilentlyContinue
            }
    }
}
# every *.setpm (Tobii calibration / profile) wherever it is, with its time
Get-ChildItem @('C:\ProgramData', $env:LOCALAPPDATA, $env:APPDATA) -Recurse -File -Force -Filter '*.setpm' -ErrorAction SilentlyContinue |
    Select-Object FullName, Length, LastWriteTime | Format-Table -AutoSize | Out-String -Width 400 | Set-Content (Join-Path $dest "setpm-files.txt")
# services and processes of Tobii and Pimax
Get-Service | Where-Object { $_.Name -match 'tobii|pimax|pvr' -or $_.DisplayName -match 'tobii|pimax' } |
    Select-Object Name, DisplayName, Status, StartType | Format-Table -AutoSize | Out-String -Width 400 | Set-Content (Join-Path $dest "services.txt")
Get-Process | Where-Object { $_.Name -match 'tobii|pimax|pvr|pi_' } | Select-Object Name, Id, Path | Format-Table -AutoSize |
    Out-String -Width 400 | Set-Content (Join-Path $dest "processes.txt")

$n = (Get-ChildItem $dest -Recurse -File).Count
Write-Host ("Copied {0} files to {1}" -f $n, $dest) -ForegroundColor Green
if (-not $NoPause) { Read-Host "Enter to close" }
