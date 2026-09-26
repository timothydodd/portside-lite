<#
.SYNOPSIS
  Builds the Portside Lite installer: PortsideLite-Setup-<version>.exe (Inno Setup 6).

.DESCRIPTION
  Stage   copies the release executable (built by `tauri build --no-bundle`) and the Microsoft
          Edge WebView2 bootstrapper into artifacts\installer-stage
  Compile compiles packaging\installer\PortsideLite.iss from that stage folder
  All     both (default)
  CI runs the halves separately so the executable can be code-signed between them; the installer
  itself is signed after Compile.

.PARAMETER Version
  Three-part version, the same one the app is built with.
.PARAMETER Exe
  The built executable (default: target\release\portside-lite.exe, the workspace target dir).

.EXAMPLE
  npm run tauri build -- --no-bundle
  .\packaging\Build-Installer.ps1 -Version 0.1.0
#>
[CmdletBinding()]
param(
    [ValidateSet("All", "Stage", "Compile")]
    [string]$Step = "All",
    [string]$Version = "0.1.0",
    [string]$Exe = "",
    [string]$Output = "artifacts"
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$stageDir = Join-Path $root "artifacts\installer-stage"
$outDir = Join-Path $root $Output
if (-not $Exe) { $Exe = Join-Path $root "target\release\portside-lite.exe" }
New-Item $outDir -ItemType Directory -Force | Out-Null

if ($Step -ne "Compile") {
    if (-not (Test-Path $Exe)) { throw "No executable at $Exe. Build it first: npm run tauri build -- --no-bundle" }
    if (Test-Path $stageDir) { Remove-Item $stageDir -Recurse -Force }
    New-Item $stageDir -ItemType Directory -Force | Out-Null
    Copy-Item $Exe (Join-Path $stageDir "portside-lite.exe")

    # Evergreen bootstrapper (~2 MB): the installer runs it only when WebView2 is missing.
    Write-Host "Downloading the WebView2 bootstrapper..."
    $bootstrapper = Join-Path $stageDir "MicrosoftEdgeWebview2Setup.exe"
    Invoke-WebRequest "https://go.microsoft.com/fwlink/p/?LinkId=2124703" -OutFile $bootstrapper -UseBasicParsing
    $sig = Get-AuthenticodeSignature $bootstrapper
    if ($sig.Status -ne "Valid" -or $sig.SignerCertificate.Subject -notmatch "O=Microsoft Corporation") {
        throw "The WebView2 bootstrapper isn't validly signed by Microsoft ($($sig.Status)); refusing to ship it."
    }
    if ($Step -eq "Stage") { return }
}
if (-not (Test-Path (Join-Path $stageDir "portside-lite.exe"))) {
    throw "Nothing staged at $stageDir. Run with -Step Stage (or All) first."
}

$iscc = @(
    (Get-Command iscc.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source),
    (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"),
    (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe")
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 (ISCC.exe) not found. Install it: choco install innosetup" }

& $iscc "/DAppVersion=$Version" "/DStageDir=$stageDir" "/DOutputDir=$outDir" (Join-Path $root "packaging\installer\PortsideLite.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
Write-Host "Built $(Join-Path $outDir "PortsideLite-Setup-$Version.exe")"
