# Crystal Super eye tracking captures for omarchyvr on Linux (2026-10-18), ALL IN ONE RUN. See CRYSTAL-EYE-2026-10-18.md.
# CAPTURE ONLY: USBPcap reads USB traffic and the file copies only read; nothing is written to the headset or to Pimax
# Play's settings.
#
# Order: copy the eye tracking files (before), takes wear, fit, slip, calibrate, dfr, copy the files again (after).
# Before every take it says what comes; Enter starts it, s + Enter skips it.
# Timed takes (wear, fit, slip): a beep and a spoken word at every step, the step in large letters; every step's time
# goes to usb-captures\<stamp>-<take>-markers.txt. Free takes (calibrate, dfr): type  m <text>  + Enter for a marker,
# an empty Enter ends the take.
param([switch]$NoSpeech)

$cmd = "C:\Program Files\USBPcap\USBPcapCMD.exe"
if (-not (Test-Path $cmd)) { Write-Host "USBPcap is not installed ($cmd)." -ForegroundColor Red; Read-Host "Enter to close"; exit 1 }
$out = Join-Path $PSScriptRoot "usb-captures"
New-Item -ItemType Directory -Force $out | Out-Null
$ifaces = & $cmd --extcap-interfaces | Select-String -Pattern 'value=(\\\\\.\\USBPcap\d+)' | ForEach-Object { $_.Matches[0].Groups[1].Value }
if (-not $ifaces) { Write-Host "No USBPcap interfaces." -ForegroundColor Red; Read-Host "Enter to close"; exit 1 }

$voice = $null
if (-not $NoSpeech) { try { Add-Type -AssemblyName System.Speech; $voice = New-Object System.Speech.Synthesis.SpeechSynthesizer } catch {} }

# T = seconds from the start, Show = what to do, Say = spoken word, M = marker
$timed = @{
    'wear' = @(
        @{ T = 0;   Show = "WEAR the headset, look straight ahead";                                    Say = "Wear it";          M = "worn" },
        @{ T = 15;  Show = "TAKE IT OFF, put it on the desk, do not touch it";                         Say = "Take it off";      M = "off-desk" },
        @{ T = 35;  Show = "PICK IT UP and hold it in your hands, do NOT put it on";                   Say = "Hold it";          M = "in-hands" },
        @{ T = 50;  Show = "PUT IT ON, look straight ahead";                                           Say = "Put it on";        M = "worn" },
        @{ T = 75;  Show = "TAKE IT OFF, put it on the desk";                                          Say = "Take it off";      M = "off-desk" },
        @{ T = 95;  Show = "PUT IT ON, look straight ahead";                                           Say = "Put it on";        M = "worn" },
        @{ T = 120; Show = "TAKE IT OFF, put it on the desk, then CLOSE PIMAX PLAY fully (window and tray). Leave it closed."; Say = "Close Pimax Play"; M = "close-play" },
        @{ T = 220; Show = "Take done. START PIMAX PLAY AGAIN and wait until the headset shows the Pimax home.";         Say = "Start Pimax Play"; M = "end" }
    )
    'fit' = @(
        @{ T = 0;   Show = "Headset ON, sitting RIGHT, look straight ahead";                           Say = "Sit it right";     M = "fit-good" },
        @{ T = 15;  Show = "Push the headset UP on your face (1-2 cm), keep looking ahead";            Say = "Push it up";       M = "fit-high" },
        @{ T = 35;  Show = "Back to the RIGHT position";                                               Say = "Back";             M = "fit-good" },
        @{ T = 50;  Show = "Pull the headset DOWN on your face (1-2 cm)";                              Say = "Pull it down";     M = "fit-low" },
        @{ T = 70;  Show = "Back to the RIGHT position";                                               Say = "Back";             M = "fit-good" },
        @{ T = 85;  Show = "TILT the headset (one side lower)";                                        Say = "Tilt it";          M = "fit-tilt" },
        @{ T = 105; Show = "Back to the RIGHT position";                                               Say = "Back";             M = "fit-good" },
        @{ T = 120; Show = "Push it AWAY from your face (loose, gap at the nose)";                     Say = "Loosen it";        M = "fit-far" },
        @{ T = 140; Show = "Back to the RIGHT position";                                               Say = "Back";             M = "fit-good" },
        @{ T = 155; Show = "Take done";                                                                Say = "Done";             M = "end" }
    )
    'slip' = @(
        @{ T = 0;   Show = "Headset ON, sitting right. LOOK AT ONE FIXED POINT straight ahead and keep looking at it"; Say = "Look at one point"; M = "fixate" },
        @{ T = 15;  Show = "Keep looking at the point, SLOWLY slide the headset UP a little";          Say = "Slowly up";        M = "slide-up" },
        @{ T = 30;  Show = "Keep looking, hold the headset still there";                               Say = "Hold";             M = "hold-high" },
        @{ T = 40;  Show = "Keep looking, SLOWLY slide it back DOWN to the right place";               Say = "Slowly back";      M = "slide-back" },
        @{ T = 55;  Show = "Keep looking, hold still";                                                 Say = "Hold";             M = "hold-good" },
        @{ T = 65;  Show = "Keep looking, SLOWLY slide the headset LEFT a little";                     Say = "Slowly left";      M = "slide-left" },
        @{ T = 80;  Show = "Keep looking, hold still";                                                 Say = "Hold";             M = "hold-left" },
        @{ T = 90;  Show = "Keep looking, SLOWLY back to the right place";                             Say = "Slowly back";      M = "slide-back" },
        @{ T = 105; Show = "Take done";                                                                Say = "Done";             M = "end" }
    )
}

$takes = @(
    @{ Name = 'wear';      Before = "WEAR (4 min): taking the headset off and on, then closing Pimax Play. Pimax Play OPEN, eye tracking ON, headset ON YOUR HEAD showing the Pimax home." },
    @{ Name = 'fit';       Before = "FIT (3 min): the wear reminder. The headset sits wrong on purpose (high, low, tilted, loose), back each time. Pimax Play running again, headset ON YOUR HEAD. Remember which steps showed the reminder." },
    @{ Name = 'slip';      Before = "SLIP (2 min): look at one point while you slowly slide the headset. Pick a point in the Pimax home." },
    @{ Name = 'calibrate'; Before = "CALIBRATE: run the eye calibration in Pimax Play to the end. Type  m start  when you start it,  m done  when it finishes, then an empty Enter." },
    @{ Name = 'dfr';       Before = "DFR (optional, s to skip): a game or app with Pimax's eye-tracked foveated rendering ON, play a minute. Markers: m <text>. Empty Enter ends." }
)

function Take([string]$take) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $markers = Join-Path $out "$stamp-$take-markers.txt"
    $procs = foreach ($i in $ifaces) {
        $n = ($i -split 'USBPcap')[1]
        Start-Process -FilePath $cmd -ArgumentList "-d $i -A -s 65535 -o `"$(Join-Path $out "$stamp-$take-hub$n.pcap")`"" -WindowStyle Hidden -PassThru
    }
    $t0 = Get-Date
    Add-Content $markers ("start {0:o} utc {1:o}" -f $t0, $t0.ToUniversalTime())
    $mark = { param($m) $now = Get-Date; Add-Content $markers ("{0,7:N2} s  {1}  {2:o}" -f ($now - $t0).TotalSeconds, $m, $now); $now }
    if ($timed.ContainsKey($take)) {
        foreach ($s in $timed[$take]) {
            $at = $t0.AddSeconds($s.T)
            while ((Get-Date) -lt $at) { Start-Sleep -Milliseconds 50 }
            $now = & $mark $s.M
            [Console]::Beep(880, 250)
            Write-Host ""
            Write-Host ("[{0,3:N0} s] {1}" -f ($now - $t0).TotalSeconds, $s.Show) -ForegroundColor Green
            if ($voice) { try { $voice.SpeakAsync($s.Say) | Out-Null } catch {} }
        }
    } else {
        Write-Host "RECORDING. Type  m <text>  + Enter for a marker, an empty Enter ends this take." -ForegroundColor Green
        while ($true) {
            $l = Read-Host
            if ($l -eq '') { break }
            if ($l.StartsWith('m ')) { $l = $l.Substring(2) }
            [void](& $mark $l)
            Write-Host "  marked"
        }
    }
    [void](& $mark "stop")
    $procs | Stop-Process -Force -ErrorAction SilentlyContinue
    $note = Read-Host "Anything to note about this take (e.g. which steps showed the wear reminder)? Enter = nothing"
    if ($note -ne '') { Add-Content $markers ("note: {0}" -f $note) }
}

$files = Join-Path $PSScriptRoot 'crystal-eye-files.ps1'
Write-Host ""
Write-Host "Crystal Super eye tracking captures: about 15 minutes. SteamVR must be CLOSED the whole time." -ForegroundColor Cyan
Write-Host "Step 0: copying the eye tracking files (before)..." -ForegroundColor Cyan
& $files -Label 'before' -NoPause

foreach ($t in $takes) {
    Write-Host ""
    Write-Host ("NEXT: " + $t.Before) -ForegroundColor Yellow
    if ($voice) { try { $voice.SpeakAsync("Next take: " + $t.Name) | Out-Null } catch {} }
    $a = Read-Host "Enter = start, s = skip"
    if ($a -eq 's') { Write-Host "  skipped"; continue }
    Take $t.Name
}

Write-Host ""
Write-Host "Last step: copying the eye tracking files (after)..." -ForegroundColor Cyan
& $files -Label 'after' -NoPause
Write-Host ""
Write-Host "ALL DONE. Everything is on this drive (usb-captures\ and eye-files-*). You can go back to Linux." -ForegroundColor Green
Read-Host "Enter to close"
