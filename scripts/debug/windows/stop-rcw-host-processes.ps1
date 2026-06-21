$ErrorActionPreference = "Stop"

$processNames = @(
    "rcw-host-debug",
    "rcw-host-gui-debug"
)

$taskNames = @(
    "Codex-Launch-RcwHost-Interactive",
    "Codex-Launch-RcwHostGui-Interactive"
)

$runnerPaths = @(
    "C:\Windows\Temp\rcw-host-session1-runner.ps1",
    "C:\Windows\Temp\rcw-host-gui-session1-runner.ps1"
)

Write-Output ("timestamp=" + (Get-Date -Format o))
Write-Output ("whoami=" + (whoami))

foreach ($name in $processNames) {
    $processes = @(Get-Process -Name $name -ErrorAction SilentlyContinue)
    if ($processes.Count -gt 0) {
        $ids = $processes | ForEach-Object { $_.Id }
        Write-Output ("stoppingProcess=" + $name + " ids=" + ($ids -join ","))
        $processes | Stop-Process -Force
    } else {
        Write-Output ("stoppingProcess=" + $name + " ids=")
    }
}

Start-Sleep -Seconds 2

$remaining = @(Get-Process -Name $processNames -ErrorAction SilentlyContinue)
Write-Output "remainingProcesses:"
if ($remaining.Count -gt 0) {
    $remaining |
        Select-Object ProcessName, Id, SessionId, Path |
        Format-Table -AutoSize |
        Out-String |
        Write-Output
} else {
    Write-Output "<none>"
}

foreach ($taskName in $taskNames) {
    $task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    if ($null -ne $task) {
        Write-Output ("unregisterTask=" + $taskName)
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    } else {
        Write-Output ("unregisterTask=" + $taskName + " missing")
    }
}

foreach ($path in $runnerPaths) {
    Remove-Item -Path $path -Force -ErrorAction SilentlyContinue
}

if ($remaining.Count -gt 0) {
    throw "rcw host processes still running after cleanup"
}
