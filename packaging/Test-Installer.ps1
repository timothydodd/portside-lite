<#
.SYNOPSIS
  Smoke-tests the installer on a clean Windows machine (CI runs it on windows-latest).

.DESCRIPTION
  Per-user silent install with the autostart task -> files, Start menu shortcut and login item exist
  -> the app starts (in tray mode) and is still running after a few seconds -> reinstall over itself
  -> a downgrade is refused -> uninstall stops the app and leaves no files, shortcut or login item.

  Changes the machine it runs on; meant for throwaway CI runners and test VMs.

.EXAMPLE
  .\packaging\Test-Installer.ps1 -Installer artifacts\PortsideLite-Setup-0.1.0.exe
#>
param(
    [Parameter(Mandatory)][string]$Installer,
    [string]$LogDir = (Join-Path ([System.IO.Path]::GetTempPath()) 'portside-lite-installer-logs')
)

$ErrorActionPreference = 'Stop'
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Portside Lite'
$exe = Join-Path $installDir 'portside-lite.exe'
$shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Portside Lite.lnk'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
New-Item $LogDir -ItemType Directory -Force | Out-Null
$failures = [System.Collections.Generic.List[string]]::new()

function Check([bool]$condition, [string]$what) {
    if ($condition) { Write-Host "  ok   $what" }
    else { Write-Host "  FAIL $what" -ForegroundColor Red; $failures.Add($what) }
}

function Invoke-Setup([string[]]$extra = @()) {
    $log = Join-Path $LogDir ("setup-{0}.log" -f [DateTime]::Now.Ticks)
    $arguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', "/LOG=`"$log`"") + $extra
    return (Start-Process -FilePath $Installer -ArgumentList $arguments -Wait -PassThru).ExitCode
}

function Get-RunValue { (Get-ItemProperty $runKey -ErrorAction SilentlyContinue).'Portside Lite' }

Write-Host "Install (per user, autostart task)"
Check ((Invoke-Setup @('/TASKS="autostart"')) -eq 0) "installer exits 0"
Check (Test-Path $exe) "executable installed to $installDir"
Check (Test-Path $shortcut) "Start menu shortcut created"
foreach ($f in "LICENSE.txt", "THIRD_PARTY_NOTICES.md", "THIRD_PARTY_LICENSES.txt") {
    Check (Test-Path (Join-Path $installDir $f)) "$f installed"
}
Check ((Get-RunValue) -like '*portside-lite.exe" --tray') "login item starts it in the tray"

Write-Host "Run"
$p = Start-Process -FilePath $exe -ArgumentList '--tray' -PassThru
Start-Sleep -Seconds 8
Check (-not $p.HasExited) "app is still running after 8 s (exit code: $(if ($p.HasExited) { $p.ExitCode } else { 'n/a' }))"

Write-Host "Reinstall over itself (app running)"
Check ((Invoke-Setup @('/TASKS="autostart"')) -eq 0) "reinstall exits 0"
Check (Test-Path $exe) "executable still present"

Write-Host "Downgrade is refused"
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{9C3E5F1A-4B7D-4E2A-9F6C-2D8B1A7E5C44}_is1'
$realVersion = (Get-ItemProperty $uninstallKey).DisplayVersion
Set-ItemProperty $uninstallKey -Name DisplayVersion -Value '999.0.0'
Check ((Invoke-Setup) -ne 0) "installing over a newer version fails"
Set-ItemProperty $uninstallKey -Name DisplayVersion -Value $realVersion

Write-Host "Uninstall (with the app running)"
Start-Process -FilePath $exe -ArgumentList '--tray' | Out-Null
Start-Sleep -Seconds 3
$uninstaller = Join-Path $installDir 'unins000.exe'
Start-Process -FilePath $uninstaller -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait | Out-Null
for ($i = 0; $i -lt 120 -and (Test-Path $uninstaller); $i++) { Start-Sleep -Milliseconds 500 }
Check (-not (Get-Process portside-lite -ErrorAction SilentlyContinue)) "app stopped"
Check (-not (Test-Path $exe)) "executable removed"
Check (-not (Test-Path $shortcut)) "Start menu shortcut removed"
Check (-not (Get-RunValue)) "login item removed"

if ($failures.Count) {
    throw "$($failures.Count) installer check(s) failed: $($failures -join '; ')"
}
Write-Host "All installer checks passed."
