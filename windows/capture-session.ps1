<#
.SYNOPSIS
  Guided, menu-driven Windows capture session for the Pimax Dream Air (see docs/WINDOWS-CAPTURE-PLAN.md).
  CAPTURE ONLY: USBPcap reads USB traffic; nothing is written to the headset, controllers or Pimax settings.

.DESCRIPTION
  Per scenario: it reads (and speaks) the instruction, you press Enter, it starts USBPcap on every root hub
  (and, for tracking scenarios, the Pimax pose logger), you do the action, press Enter, it stops and names the files
  YYYYMMDD-HHMMSS-<scenario>-hub<N>.pcap (same naming as all earlier takes; -Pcapng converts with editcap) in <session>\usb-captures\.
  While recording type  m <text>  + Enter to drop a timestamped marker into notes.txt.
  Session folder: <OutRoot>\pimax-win-session-<date>\ . Put this script on the shared NTFS drive (UUID from `shared_drive_uuid` in `~/.config/omarchyvr/paths.toml`)
  and run it from there; everything lands on that drive, no copying needed.
  Licensing: copied Pimax Play files are for local analysis only, never redistributed.
  Needs: USBPcap (C:\Program Files\USBPcap\USBPcapCMD.exe). Optional: Wireshark (editcap for .pcapng).
#>
[CmdletBinding()]
param(
    [string]$OutRoot = '',     # default: folder of this script
    [switch]$NoSpeech,
    [switch]$Quiet,
    [switch]$Pcapng   # also convert to .pcapng (needs Wireshark editcap). Default .pcap like all earlier takes
)
$ErrorActionPreference = 'Continue'
if ($OutRoot -eq '') { $OutRoot = $PSScriptRoot }
if ($OutRoot -eq '') { $OutRoot = (Get-Location).Path }

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) { Write-Host 'Run via capture-session.cmd (Administrator).' -ForegroundColor Red; Read-Host 'Enter to close'; exit 1 }

$usbcmd = 'C:\Program Files\USBPcap\USBPcapCMD.exe'
if (-not (Test-Path $usbcmd)) {
    Write-Host 'USBPcap is not installed. Install it from https://desowin.org/usbpcap/ , reboot once, run again.' -ForegroundColor Red
    Read-Host 'Enter to close'; exit 1
}
$editcap = 'C:\Program Files\Wireshark\editcap.exe'
$haveEditcap = Test-Path $editcap
$poseScript = Join-Path $PSScriptRoot 'pimax-dump-distortion.ps1'
$havePose = Test-Path $poseScript

$sessionDir = Join-Path $OutRoot ('pimax-win-session-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Force $sessionDir | Out-Null
$usbDir = Join-Path $sessionDir 'usb-captures'; New-Item -ItemType Directory -Force $usbDir | Out-Null
$notes = Join-Path $sessionDir 'notes.txt'
$logf  = Join-Path $sessionDir 'session.log'

# --- speech (instructions are heard in the headset or from the PC speakers) ---
$voice = $null
if (-not $NoSpeech) {
    try { Add-Type -AssemblyName System.Speech; $voice = New-Object System.Speech.Synthesis.SpeechSynthesizer; $voice.Rate = 0 } catch { $voice = $null }
}
function Say([string]$t, [string]$color = 'White', [switch]$Speak) {
    Write-Host $t -ForegroundColor $color
    Add-Content $logf ("{0} {1}" -f (Get-Date -Format o), $t)
    if ($Speak -and $voice) { try { $voice.SpeakAsync($t) | Out-Null } catch {} }
}
function Beep([int]$n = 1) { if ($Quiet) { return }; for ($i = 0; $i -lt $n; $i++) { [Console]::Beep(880, 200); Start-Sleep -Milliseconds 120 } }
function Note([string]$t) { Add-Content $notes ("{0} {1}" -f (Get-Date -Format o), $t) }   # local ISO time, same format as captures\notes.txt

# --- hubs ---
$ifaces = & $usbcmd --extcap-interfaces | Select-String -Pattern 'value=(\\\\\.\\USBPcap\d+)' | ForEach-Object { $_.Matches[0].Groups[1].Value }
if (-not $ifaces) { Say 'No USBPcap hubs. Reboot once after installing USBPcap.' Red; Read-Host 'Enter to close'; exit 1 }
$ifaces = @($ifaces)

# --- scenarios: id, name, seconds (0 = until Enter), pose logger, spoken/printed steps, why ---
$S = @(
 @{ Id=1;  Name='baseline-idle';       Snap=512; Sec=30;  Pose=$false; Do='Headset connected and powered, Pimax Play closed, controllers off. Do nothing for 30 seconds.'; Why='Background noise: what the USB bus does when nothing happens.' },
 @{ Id=2;  Name='poweron-home';        Snap=65535; Sec=0;   Pose=$false; Do='Headset switched off with its power button. Press Enter. After recording starts, type m on when you press the power button, switch it on, start Pimax Play (m play), wait until the Pimax home shows in the headset, press Enter.'; Why='Full power-on and init sequence, camera modes, IMU start, display and audio enable.' },
 @{ Id=3;  Name='ctrl-pair-unpair';    Snap=512; Sec=0;   Pose=$false; Do='In Pimax Play: unpair the left controller, pair it again, wait for it to connect. Then the same for the right controller. Type m left or m right at each step. Enter when done.'; Why='Controller pairing and radio traffic.' },
 @{ Id=4;  Name='ctrl-bt-reconnect';   Snap=512; Sec=0;   Pose=$false; Do='Disconnect the Bluetooth of the left controller in Windows, wait 10 seconds, let it reconnect. Same for the right. Mark each step with m. Enter when done.'; Why='Disconnect and reconnect traffic, and which side the host sees.' },
 @{ Id=5;  Name='ctrl-buttons-haptics'; Snap=512; Sec=0;   Pose=$false; Do='Controllers OFF at the start. Type m green now when a controller lights green, m buzz now when it vibrates. Then open the controller test in Pimax Play. Press every button one by one, move the sticks, squeeze triggers and grips, mark each with m. Then run the vibration test on each hand, or a game that rumbles. Enter when done.'; Why='Input reports and the vibration command, none captured yet.' },
 @{ Id=6;  Name='settings-bright-ipd-hz'; Snap=512; Sec=0; Pose=$false; Do='In Pimax Play change ONE setting at a time, 5 seconds apart, type m before each: brightness down and up, IPD narrow and wide, refresh 90 then back to 72 (the old take only had 90 once, no clean 72), then restore. Enter when done.'; Why='Brightness, IPD and refresh commands.' },
 @{ Id=7;  Name='audio-sound';         Snap=512; Sec=0;   Pose=$false; Do='Windows Sound settings: choose the headset as output. Play a sound or music in the headset for 20 seconds. Change the volume up and down. Unmute and mute. Enter when done.'; Why='Audio path: any audio-enable command and the volume path.' },
 @{ Id=8;  Name='track-desk-still';    Snap=65535; Sec=30;  Pose=$true;  Do='Put the headset on the desk, lying still, Pimax Play tracking. Do not touch it for 30 seconds. After the beep, give the desk one tap first.'; Why='IMU noise and bias, and tracking drift when still.' },
 @{ Id=9;  Name='track-worn-still';    Snap=65535; Sec=30;  Pose=$true;  Do='Wear the headset, sit still, look straight ahead for 30 seconds.'; Why='Head micro motion, still gate and jitter reference.' },
 @{ Id=10; Name='track-slow-look';     Snap=65535; Sec=60;  Pose=$true;  Do='Wear it. Slowly look left, right, up, down, a full circle, for 60 seconds. Standing, not walking.'; Why='Rotation tracking reference against our SLAM.' },
 @{ Id=11; Name='track-walk';          Snap=65535; Sec=90;  Pose=$true;  Do='Wear it. Walk slowly around the room, get near and far from walls, crouch once. 90 seconds, clear the floor first.'; Why='Position tracking reference: our drift and scale against Pimax.' },
 @{ Id=12; Name='track-fast-turns';    Snap=65535; Sec=45;  Pose=$true;  Do='Wear it. Turn your head fast left-right and up-down several times, safely, for 45 seconds.'; Why='Camera exposure and frame rate under fast motion, loss and recovery.' },
 @{ Id=13; Name='track-controllers';   Snap=65535; Sec=90;  Pose=$true;  Do='Wear it. Wave both controllers in front of you, then behind your back, in and out of camera view, 90 seconds. Mark with m.'; Why='Controller IR LED timing, sync and tracking loss.' },
 @{ Id=14; Name='room-setup-floor';    Snap=512; Sec=0;   Pose=$false; Do='Run room setup and floor calibration in Pimax Play to the end. Enter when done.'; Why='Floor and play area commands and files.' },
 @{ Id=15; Name='game-doe2';           Snap=65535; Sec=180; Pose=$true;  Do='Start Dungeons of Eternity in SteamVR. Play 3 minutes, use both hands, hit things so the controllers rumble.'; Why='Real game load, haptics from a game, SteamVR tracking path.' },
 @{ Id=16; Name='shutdown';            Snap=512; Sec=0;   Pose=$false; Do='Exit Pimax Play from the tray, wait 10 seconds. The power button only turns the panels off (the headset stays on): press it once to turn the panels off. Keep recording until Enter.'; Why='Power-off sequence.' }
)

# --- key moments to mark with  m <text>  (shown before recording, the next one is spoken while recording) ---
$MarkTable = @{
 2  = @('restart-clicked','headset-gone','headset-back','play-started','home-visible')
 3  = @('unpair-left','left-in-pairing-mode','pair-left-pressed','left-connected','unpair-right','right-in-pairing-mode','pair-right-pressed','right-connected')
 5  = @('green-light','buzz','trigger-l','trigger-r','grip-l','grip-r','stick-l','stick-r','button-a','button-b','button-x','button-y','menu','system','vibration-test-l','vibration-test-r')
 6  = @('ipd-narrow','ipd-wide','ipd-restored')
 13 = @('both-in-view','behind-back','back-in-view','left-hidden','right-hidden')
 14 = @('floor-set','boundary-done')
 15 = @('game-started','first-rumble','game-stopped')
}

# --- redo set (menu R): the takes of 2026-10-15 that failed, redone with the lessons of that day ---
# The power button only turns the panels off, never the headset: take 2 is "Restart headset in Pimax Play" instead.
# Controllers talk to the headset radio (not PC Bluetooth). Order: controller takes first, the headset restart last.
$Redo = @(
 @{ Id=3;  Pre='pairing'; Do='Pimax Play is running. THE LEFT UNPAIR IS THE MOST IMPORTANT PART (its packet was never captured). For ONE hand at a time: 1) in Pimax Play press Unpair for that hand (type m unpair-left), 2) put THAT controller in pairing mode first (type m left-in-pairing-mode), 3) only then press Pair for that hand and wait up to 60 s until it connects (m pair-left-pressed, m left-connected). Do the left, then the right. Pressing Pair for the other hand cancels the running one, and the other connected controller may drop out for a moment while the headset listens. Enter when both are connected.'; Why='Unpair and pair per hand. MUST include unpairing the LEFT controller in Pimax Play (left unpair not recorded yet: only the right one, 58 03 05 03, is captured).' },
 @{ Id=5;  Pre='both'; Do='Wear the headset (screen on) with BOTH controllers connected. Open the controller test in Pimax Play. Type m green when a controller lights green, m buzz when it vibrates. Press every button one by one on each hand, move the sticks, squeeze triggers and grips, mark each with m (a hint of the next one is spoken). Then run the vibration test on each hand. Enter when done.'; Why='Input reports and the vibration command, with both controllers worn.' },
 @{ Id=13; Pre='both'; Do='Wear the headset (screen on, head tracking OK) with BOTH controllers connected. Wave both controllers in front of you, then behind your back, in and out of camera view, 90 seconds. Mark with m.'; Why='Controller IR LED timing, sync and tracking loss, with the headset worn this time.' },
 @{ Id=6;  Pre='none'; Do='In Pimax Play change the IPD only: narrow, wait 5 s, wide, wait 5 s, back to your value. Type m before each. (Brightness and refresh are already captured.) Enter when done.'; Why='IPD command (0x5c was never sent in the first capture).' },
 @{ Id=2;  Pre='none'; Do='Pimax Play is running, headset on the desk. Press Enter to start recording. Then in Pimax Play press Restart headset (type m restart-clicked). The headset leaves USB and comes back by itself. Then start Pimax Play again if it does not (m play-started), wait until the Pimax home shows (m home-visible). At least 90 seconds in total. Do NOT use the power button: it only turns the panels off. Enter when done.'; Why='Full power-on and init sequence after a real headset restart.'; NoStopPimax=$true }
)

# --- pre-check: both controllers connected (best effort from the Pimax Play logs, then a spoken check + Enter) ---
function Precheck-Controllers {
    while ($true) {
        Say 'Check: are BOTH controllers on and shown as connected in Pimax Play?' Yellow -Speak
        try {
            $logs = foreach ($r in @("$env:ProgramData\Pimax", "$env:LOCALAPPDATA\Pimax", "$env:APPDATA\Pimax")) {
                if (Test-Path $r) { Get-ChildItem $r -Recurse -File -Filter *.log -ErrorAction SilentlyContinue | Where-Object { $_.LastWriteTime -gt (Get-Date).AddMinutes(-10) } }
            }
            $hits = foreach ($f in ($logs | Sort-Object LastWriteTime -Descending | Select-Object -First 4)) {
                Get-Content $f.FullName -Tail 400 -ErrorAction SilentlyContinue | Where-Object { $_ -match 'controller.*(sleeping|removed|connect|attach)|H3010[01]' } | Select-Object -Last 3
            }
            if ($hits) { Say 'Latest controller lines in the Pimax Play logs:' DarkGray; $hits | Select-Object -Last 6 | ForEach-Object { Say ('   ' + $_) DarkGray } }
        } catch {}
        $r = Read-Host 'Enter = both connected, w = wake them / wait and ask again, s = start anyway'
        if ($r -eq 's') { Note 'PRECHECK skipped (controllers not confirmed)'; return }
        if ($r -ne 'w') { Note 'PRECHECK both controllers confirmed connected'; return }
        Say 'Press a button on each controller to wake it, wait 10 s.' Yellow -Speak; Start-Sleep 10
    }
}
function Precheck-Pairing {
    Say 'Pairing: put the controller in pairing mode FIRST, then press Pair for that hand only, wait up to 60 s. One hand at a time.' Yellow -Speak
    Say '        While the headset listens, the other (connected) controller may drop out for a moment. That is normal and is what we want to see.' DarkGray
}

# --- one-off helpers (read only) ---
function Collect-Info {
    $d = Join-Path $sessionDir 'info'; New-Item -ItemType Directory -Force $d | Out-Null
    Say 'Collecting system info, USB devices, monitors, sound devices ...' Cyan
    try { systeminfo | Out-File (Join-Path $d 'systeminfo.txt') -Encoding UTF8 } catch {}
    try { Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -match 'VID_34A4|VID_2104|VID_0483|VID_28DE' } |
          Select-Object Status, Class, FriendlyName, InstanceId | Format-List | Out-File (Join-Path $d 'usb-pimax-devices.txt') -Encoding UTF8 } catch {}
    try { Get-PnpDevice -PresentOnly | Select-Object Status, Class, FriendlyName, InstanceId | Format-Table -AutoSize -Wrap |
          Out-File (Join-Path $d 'pnp-all.txt') -Width 250 -Encoding UTF8 } catch {}
    try { Get-CimInstance Win32_SoundDevice | Select-Object Name, Manufacturer, Status, DeviceID | Format-List | Out-File (Join-Path $d 'sound-devices.txt') -Encoding UTF8 } catch {}
    try { Get-PnpDevice -Class AudioEndpoint -PresentOnly | Select-Object Status, FriendlyName, InstanceId | Format-List | Out-File (Join-Path $d 'audio-endpoints.txt') -Encoding UTF8 } catch {}
    try { Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion, CurrentHorizontalResolution, CurrentVerticalResolution, CurrentRefreshRate | Format-List | Out-File (Join-Path $d 'gpu.txt') -Encoding UTF8 } catch {}
    try { Get-CimInstance -Namespace root\wmi -ClassName WmiMonitorID | ForEach-Object {
            $n = ($_.UserFriendlyName | Where-Object { $_ -ne 0 } | ForEach-Object { [char]$_ }) -join ''
            $m = ($_.ManufacturerName | Where-Object { $_ -ne 0 } | ForEach-Object { [char]$_ }) -join ''
            "{0} | {1} | {2}" -f $m, $n, $_.InstanceName } | Out-File (Join-Path $d 'monitors-edid-names.txt') -Encoding UTF8 } catch {}
    try { Get-CimInstance -Namespace root\wmi -ClassName WmiMonitorBasicDisplayParams | Format-List | Out-File (Join-Path $d 'monitors-basic.txt') -Encoding UTF8 } catch {}
    try { Add-Type -AssemblyName System.Windows.Forms; [Windows.Forms.Screen]::AllScreens | Select-Object DeviceName, Primary, Bounds | Format-List | Out-File (Join-Path $d 'screens.txt') -Encoding UTF8 } catch {}
    try { dxdiag /t (Join-Path $d 'dxdiag.txt'); Start-Sleep -Seconds 12 } catch {}
    try { Get-Process | Where-Object { $_.Name -match 'Pimax|Pi[A-Z]|vr|Tobii|Steam' } | Select-Object Name, Id, Path | Format-Table -AutoSize | Out-File (Join-Path $d 'processes.txt') -Width 250 -Encoding UTF8 } catch {}
    Say "Saved in $d" Green
}
function Copy-Logs([string]$tag) {
    $d = Join-Path (Join-Path $sessionDir 'pimax-play-logs') $tag; New-Item -ItemType Directory -Force $d | Out-Null
    $srcs = @("$env:ProgramData\Pimax", "$env:LOCALAPPDATA\Pimax", "$env:APPDATA\Pimax", "$env:ProgramFiles\Pimax\Runtime")
    foreach ($s in $srcs) {
        if (-not (Test-Path $s)) { continue }
        $dst = Join-Path $d (($s -replace '[:\\]', '_'))
        Get-ChildItem $s -Recurse -File -ErrorAction SilentlyContinue |
          Where-Object { $_.Extension -match '^\.(log|txt|csv|json|xml|ini|cfg|conf|vrchap|yaml|yml)$' -and $_.FullName -notmatch 'token|cookie|password|login|credential|Cache|CrashDumps' -and $_.Length -lt 50MB } |
          ForEach-Object {
            $rel = $_.FullName.Substring($s.Length).TrimStart('\')
            $t = Join-Path $dst $rel; New-Item -ItemType Directory -Force (Split-Path $t) | Out-Null
            Copy-Item $_.FullName $t -ErrorAction SilentlyContinue }
    }
    Say "Pimax Play logs and config copied to $d" Green
}
function Copy-InstallFiles {
    # Config / model / calibration files only. No dll/exe/sys. Local analysis only, never redistribute.
    $d = Join-Path $sessionDir 'pimax-play-install-files'; New-Item -ItemType Directory -Force $d | Out-Null
    $roots = @("$env:ProgramFiles\Pimax", "${env:ProgramFiles(x86)}\Pimax", "$env:ProgramData\Pimax", "$env:LOCALAPPDATA\Pimax", "$env:APPDATA\Pimax")
    $ext = '^\.(json|xml|ini|cfg|conf|txt|yaml|yml|csv|obj|fbx|gltf|glb|stl|ply|bin|dbow|vrchap|vrsettings|lua)$'
    $skip = 'token|cookie|password|login|credential|toolkit|CrashDumps|\\Cache\\|\\Logs?\\'
    $n = 0
    foreach ($s in $roots) {
        if (-not (Test-Path $s)) { continue }
        Say "Scanning $s ..." Gray
        Get-ChildItem $s -Recurse -File -ErrorAction SilentlyContinue |
          Where-Object { $_.Extension -match $ext -and $_.FullName -notmatch $skip -and $_.Length -lt 200MB } |
          ForEach-Object {
            $rel = $_.FullName.Substring($s.Length).TrimStart('\')
            $t = Join-Path (Join-Path $d (($s -replace '[:\\]', '_'))) $rel
            New-Item -ItemType Directory -Force (Split-Path $t) | Out-Null
            Copy-Item $_.FullName $t -ErrorAction SilentlyContinue; $n++ }
    }
    Get-ChildItem $d -Recurse -File | Select-Object @{n='path';e={$_.FullName.Substring($d.Length)}}, Length |
        Export-Csv (Join-Path $d '_file-list.csv') -NoTypeInformation -Encoding UTF8
    Say "$n files copied to $d (keep local, never redistribute)" Green
}

function Stop-Pimax {
    # Same as capture-round2.ps1: stop Pimax software so the REAL init sequence is recorded after power-on.
    Stop-Service PiServiceLauncher -Force -ErrorAction SilentlyContinue
    Stop-Service 'Tobii VR4PIMAXP3B Platform Runtime' -Force -ErrorAction SilentlyContinue
    Get-Process | Where-Object { $_.Path -match '\\Pimax\\' } | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep 3
}
# --- capture one scenario ---
function Run-Scenario($sc, $ov = $null) {
    if ($ov) { $sc = $sc.Clone(); foreach ($k in $ov.Keys) { $sc[$k] = $ov[$k] } }
    $name = $sc.Name
    $marks = @(); if ($MarkTable.ContainsKey([int]$sc.Id)) { $marks = @($MarkTable[[int]$sc.Id]) }
    Write-Host ''
    Say ("=== {0:D2} {1} ===" -f $sc.Id, $name) Cyan
    Say ("DO:  " + $sc.Do) White -Speak
    Say ("WHY: " + $sc.Why) DarkGray
    if ($sc.Sec -gt 0) { Say ("Length: {0} s, it stops by itself (Enter stops early)." -f $sc.Sec) DarkGray }
    if ($marks.Count -gt 0) { Say ('Markers to type (m <text> + Enter): ' + ($marks -join ', ')) DarkGray }
    if ($sc.Pre -eq 'both') { Precheck-Controllers } elseif ($sc.Pre -eq 'pairing') { Precheck-Pairing }
    elseif ($sc.Id -in 13, 15 -and -not $ov) { Precheck-Controllers }
    if ($sc.Name -eq 'poweron-home' -and -not $sc.NoStopPimax) {
        $r = Read-Host 'Stop ALL Pimax software now (as in round 2 cold-start)? Enter = yes, n = no'
        if ($r -ne 'n') { Stop-Pimax; Say 'Pimax software stopped. Switch the headset off with its power button, wait 5 s.' Yellow -Speak }
    }
    $go = Read-Host 'Enter = start, s = skip, q = back to menu'
    if ($go -eq 'q') { return 'q' }
    if ($go -eq 's') { return 's' }
    Say 'Starting in 3 ...' Yellow; Start-Sleep 1; Say '2 ...' Yellow; Start-Sleep 1; Say '1 ...' Yellow; Start-Sleep 1

    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $raw = @(); $procs = @()
    foreach ($i in $ifaces) {
        $n = ($i -split 'USBPcap')[1]
        $f = Join-Path $usbDir ("{0}-{1}-hub{2}.pcap" -f $stamp, $name, $n)
        $raw += $f
        $procs += Start-Process -FilePath $usbcmd -ArgumentList "-d $i -A -s $($sc.Snap) -o `"$f`"" -WindowStyle Hidden -PassThru
    }
    $poseProc = $null
    if ($sc.Pose -and $havePose) {
        $poseDir = Join-Path $sessionDir ("{0}-{1}-poses" -f $stamp, $name)
        New-Item -ItemType Directory -Force $poseDir | Out-Null
        $secs = if ($sc.Sec -gt 0) { $sc.Sec } else { 3600 }   # untimed: ends with stop.flag when the take stops
        # Sysnative only exists for 32-bit callers; this script normally runs in 64-bit PowerShell
        $psExe = "$env:SystemRoot\Sysnative\WindowsPowerShell\v1.0\powershell.exe"
        if (-not (Test-Path $psExe)) { $psExe = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" }
        $poseProc = Start-Process -FilePath $psExe -WindowStyle Hidden -PassThru `
            -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File `"$poseScript`" -Parts 2 -Seconds $secs -NoPause -OutRoot `"$poseDir`""
    }
    Note ("START {0} (snaplen {1})" -f $name, $sc.Snap)
    Beep 2
    Say ("RECORDING {0}  (GO!)" -f $name) Green -Speak
    $t0 = Get-Date
    if ($sc.Sec -gt 0) {
        # timed: poll for typed markers is not possible without blocking, so only Enter stops early
        $mi = 0
        while (((Get-Date) - $t0).TotalSeconds -lt $sc.Sec) {
            if ($marks.Count -gt 0 -and $mi -lt $marks.Count -and ((Get-Date) - $t0).TotalSeconds -ge ($sc.Sec * ($mi + 1) / ($marks.Count + 1))) {
                Say ("Marker moment: press any key, then type  " + $marks[$mi]) Yellow -Speak; $mi++
            }
            if ([Console]::KeyAvailable) {
                [void][Console]::ReadKey($true)
                $line = Read-Host '  marker text (empty = stop now)'
                if ($line -eq '') { break }
                Note ("MARK {0}: {1}" -f $name, $line); Say '  marked' DarkGray
            }
            Start-Sleep -Milliseconds 200
        }
        Say 'Time is up. Stopping.' Green -Speak
    } else {
        $mi = 0
        while ($true) {
            if ($mi -lt $marks.Count) { Say ("  next marker: m " + $marks[$mi]) DarkGray -Speak }
            $line = Read-Host '  (m <text> = marker, Enter = stop)'
            if ($line -match '^m\s*(.*)') { Note ("MARK {0}: {1}" -f $name, $Matches[1]); Say '  marked' DarkGray; if ($mi -lt $marks.Count) { $mi++ } } else { break }
        }
    }
    Beep 3
    $procs | Stop-Process -Force -ErrorAction SilentlyContinue
    if ($poseProc) {
        # ask the logger to flush and close its CSV (stop.flag), only kill it if it does not exit in 8 s
        try { Set-Content (Join-Path $poseDir 'stop.flag') 'stop' } catch {}
        [void]$poseProc.WaitForExit(8000)
        if (-not $poseProc.HasExited) { Stop-Process -Id $poseProc.Id -Force -ErrorAction SilentlyContinue; Say '   pose logger had to be killed (last second may be missing)' Yellow }
    }
    Note ("STOP {0}" -f $name)
    Start-Sleep -Milliseconds 500
    foreach ($f in $raw) {
        if (-not (Test-Path $f)) { continue }
        if ((Get-Item $f).Length -lt 256) { Remove-Item $f -Force; continue }   # empty hub (hub1 was empty in all earlier takes)
        if ($Pcapng -and $haveEditcap) {
            $ng = [IO.Path]::ChangeExtension($f, '.pcapng')
            & $editcap -F pcapng $f $ng 2>$null
            if ((Test-Path $ng) -and (Get-Item $ng).Length -gt 0) { Remove-Item $f -Force; $f = $ng }
        }
        Say ("   saved {0}  {1:N1} MB" -f (Split-Path $f -Leaf), ((Get-Item $f).Length / 1MB)) Green
    }
    return 'ok'
}

# --- menu ---
function Show-Menu {
    Write-Host ''
    Write-Host '==== Pimax Windows capture session (capture only, nothing is written to the headset) ====' -ForegroundColor Cyan
    Write-Host "Session folder: $sessionDir"
    $free = (Get-PSDrive -Name $sessionDir.Substring(0,1)).Free / 1GB
    Write-Host ("Free space: {0:N0} GB (have 20 GB or more; tracking scenarios are big)  hubs: {1}" -f $free, $ifaces.Count)
    foreach ($sc in $S) { Write-Host ("  {0,2}  {1,-24} {2}" -f $sc.Id, $sc.Name, $(if ($sc.Sec -gt 0) { "$($sc.Sec) s" } else { 'until Enter' })) }
    Write-Host '   R  REDO set from 2026-10-15: 3 pair/unpair, 5 buttons+haptics (worn), 13 controllers (worn), 6 IPD only, 2 headset restart (last)'
    Write-Host '   A  run ALL scenarios in order        I  system info (monitors, sound, USB)'
    Write-Host '   L  copy Pimax Play logs now          F  copy Pimax Play config/model/calibration files'
    Write-Host '   Q  quit (logs are copied again on exit)'
}

Say 'Before you start: Pimax Play installed, headset + both controllers charged, room clear, the Windows sound volume at 30 %.' Yellow
Collect-Info
Copy-Logs 'before'
while ($true) {
    Show-Menu
    $c = (Read-Host 'Choose numbers (e.g. 2 5), R, A, I, L, F or Q').Trim()
    if ($c -match '^[qQ]$') { break }
    elseif ($c -match '^[iI]$') { Collect-Info }
    elseif ($c -match '^[lL]$') { Copy-Logs (Get-Date -Format 'HHmmss') }
    elseif ($c -match '^[fF]$') { Copy-InstallFiles }
    elseif ($c -match '^[rR]$') {
        Say 'REDO set: have Pimax Play running, headset worn for 5 and 13, both controllers charged. Take 4 (Bluetooth) is dropped: controllers use the headset radio.' Yellow -Speak
        foreach ($ov in $Redo) {
            $sc = $S | Where-Object { $_.Id -eq $ov.Id }
            $r = Run-Scenario $sc $ov
            if ($r -eq 'q') { break }
        }
    }
    elseif ($c -match '^[aA]$') {
        foreach ($sc in $S) { $r = Run-Scenario $sc; if ($r -eq 'q') { break } }
    }
    else {
        foreach ($tok in ($c -split '[,\s]+' | Where-Object { $_ -match '^\d+$' })) {
            $sc = $S | Where-Object { $_.Id -eq [int]$tok }
            if ($sc) { [void](Run-Scenario $sc) } else { Say "No scenario $tok" Red }
        }
    }
}
Copy-Logs 'after'
Say ("Done. Everything is in {0}. On Linux: omarchyvr mount-shared, then tell the agent the folder name." -f $sessionDir) Green -Speak
Read-Host 'Enter to close'
