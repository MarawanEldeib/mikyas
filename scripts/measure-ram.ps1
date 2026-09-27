# Sums memory of the widget process and all of its WebView2 descendants.
# Usage: scripts/measure-ram.ps1 [-ProcessId <pid>]  (defaults to the running mikyas)
param([int]$ProcessId = 0)

if (-not $ProcessId) {
    $p = Get-Process mikyas -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $p) { Write-Error "mikyas is not running"; exit 1 }
    $ProcessId = $p.Id
}

$all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
$tree = [System.Collections.Generic.List[int]]::new()
$tree.Add($ProcessId)
for ($i = 0; $i -lt $tree.Count; $i++) {
    $all | Where-Object { $_.ParentProcessId -eq $tree[$i] } | ForEach-Object { $tree.Add([int]$_.ProcessId) }
}

$rows = foreach ($id in $tree) {
    $proc = Get-Process -Id $id -ErrorAction SilentlyContinue
    if (-not $proc) { continue }
    $pws = (Get-Counter "\Process(*)\ID Process" -ErrorAction SilentlyContinue).CounterSamples |
        Where-Object { $_.CookedValue -eq $id } | Select-Object -First 1
    $private = $null
    if ($pws) {
        $inst = $pws.Path -replace '\\id process$', '\working set - private'
        $private = (Get-Counter $inst -ErrorAction SilentlyContinue).CounterSamples[0].CookedValue
    }
    [pscustomobject]@{
        Name       = $proc.ProcessName
        Id         = $id
        WS_MB      = [math]::Round($proc.WorkingSet64 / 1MB, 1)
        PrivWS_MB  = if ($private) { [math]::Round($private / 1MB, 1) } else { $null }
    }
}
$rows | Format-Table -AutoSize | Out-String
"TOTAL working set: {0:N1} MB   private working set: {1:N1} MB" -f (($rows | Measure-Object WS_MB -Sum).Sum), (($rows | Measure-Object PrivWS_MB -Sum).Sum)
