# Records USB traffic with USBPcap on every USB root hub (needs Administrator; the .cmd launcher elevates).
#   -Name      label used in the file names
#   -Seconds   length; 0 = until you press Enter
#   -Snaplen   65535 = whole packets (default). Camera iso URBs are still cut at ~64 KB by USBPcap, see README.
#   -Hubs      only these hub numbers, e.g. "2" (default: all hubs)
param(
    [string]$Name = "session",
    [int]$Seconds = 0,
    [int]$Snaplen = 65535,
    [string]$Hubs = ""
)
$cmd = "C:\Program Files\USBPcap\USBPcapCMD.exe"
if (-not (Test-Path $cmd)) { Write-Host "USBPcap is not installed ($cmd). Install it from https://desowin.org/usbpcap/ and reboot." -ForegroundColor Red; Read-Host "Enter to close"; exit 1 }
$out = Join-Path $PSScriptRoot "usb-captures"
New-Item -ItemType Directory -Force $out | Out-Null
$free = (Get-PSDrive -Name ((Resolve-Path $out).Path.Substring(0,1))).Free
Write-Host ("Free space on the target drive: {0:N1} GB (a 2 minute camera recording can need 1 GB or more)" -f ($free / 1GB))
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"

$ifaces = & $cmd --extcap-interfaces | Select-String -Pattern 'value=(\\\\\.\\USBPcap\d+)' | ForEach-Object { $_.Matches[0].Groups[1].Value }
if (-not $ifaces) { Write-Host "No USBPcap interfaces. Reboot once after installing USBPcap." -ForegroundColor Red; Read-Host "Enter to close"; exit 1 }
if ($Hubs -ne "") { $want = $Hubs -split '[,\s]+'; $ifaces = $ifaces | Where-Object { $want -contains (($_ -split 'USBPcap')[1]) } }

$procs = foreach ($i in $ifaces) {
    $n = ($i -split 'USBPcap')[1]
    $file = Join-Path $out "$stamp-$Name-hub$n.pcap"
    Start-Process -FilePath $cmd -ArgumentList "-d $i -A -s $Snaplen -o `"$file`"" -WindowStyle Hidden -PassThru
}
$t0 = Get-Date
Write-Host ("RECORDING '{0}' on {1} hub(s), start {2:o} (UTC {3:o})" -f $Name, @($ifaces).Count, $t0, $t0.ToUniversalTime()) -ForegroundColor Green
if ($Seconds -gt 0) { Start-Sleep -Seconds $Seconds } else { [void](Read-Host "Press Enter to STOP the recording") }
$procs | Stop-Process -Force -ErrorAction SilentlyContinue
Write-Host ("STOPPED at {0:o} (UTC {1:o})" -f (Get-Date), (Get-Date).ToUniversalTime()) -ForegroundColor Green
Get-ChildItem $out -Filter "$stamp-$Name-*.pcap" | Select-Object Name, @{n='MB';e={[math]::Round($_.Length/1MB,1)}}
Read-Host "Enter to close"
