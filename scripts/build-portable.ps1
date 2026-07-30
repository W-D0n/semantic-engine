[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$WebView2Cab,

    [string]$OutputDirectory,

    [switch]$Archive
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path -LiteralPath (Join-Path $cargoBin 'cargo.exe')) {
    $env:PATH = $cargoBin + [IO.Path]::PathSeparator + $env:PATH
}
if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
    throw 'Cargo is required to build Semantic Engine and was not found.'
}
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$desktopRoot = Join-Path $repoRoot 'apps\desktop'
$sourceRuntimeLink = Join-Path $desktopRoot 'src-tauri\WebView2'
$releaseExecutable = Join-Path $repoRoot 'target\release\semantic-engine-desktop.exe'
$cabPath = (Resolve-Path -LiteralPath $WebView2Cab).Path
$runtimeLockPath = Join-Path $PSScriptRoot 'webview2-runtime.json'
$runtimeLock = Get-Content -LiteralPath $runtimeLockPath -Raw -Encoding UTF8 | ConvertFrom-Json

if ([IO.Path]::GetFileName($cabPath) -ne $runtimeLock.filename) {
    throw "Unexpected WebView2 package. Expected $($runtimeLock.filename)."
}

$cabHash = (Get-FileHash -LiteralPath $cabPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($runtimeLock.sha256 -ne 'pending-download-verification' -and $cabHash -ne $runtimeLock.sha256) {
    throw "WebView2 package checksum mismatch. Expected $($runtimeLock.sha256), got $cabHash."
}

if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $repoRoot 'portable\SemanticEngine'
}
$outputPath = [IO.Path]::GetFullPath($OutputDirectory)
if ($outputPath -eq $repoRoot -or $outputPath -eq [IO.Path]::GetPathRoot($outputPath)) {
    throw "Unsafe portable output directory: $outputPath"
}
if (Test-Path -LiteralPath $outputPath) {
    throw "Portable output already exists: $outputPath. Move or remove it explicitly first."
}
if (Test-Path -LiteralPath $sourceRuntimeLink) {
    throw "Portable runtime build link already exists: $sourceRuntimeLink"
}

$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ('semantic-engine-portable-' + [guid]::NewGuid().ToString('N'))
$runtimeExtract = Join-Path $temporaryRoot 'runtime'
$packageStaging = Join-Path $temporaryRoot 'package'

try {
    New-Item -ItemType Directory -Force -Path $runtimeExtract, $packageStaging | Out-Null

    & expand.exe $cabPath '-F:*' $runtimeExtract | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "WebView2 extraction failed with exit code $LASTEXITCODE."
    }

    $runtimeExecutables = @(Get-ChildItem -LiteralPath $runtimeExtract -Recurse -Filter 'msedgewebview2.exe' -File)
    if ($runtimeExecutables.Count -ne 1) {
        throw "Expected one msedgewebview2.exe after extraction, found $($runtimeExecutables.Count)."
    }
    $runtimeRoot = $runtimeExecutables[0].Directory.FullName
    New-Item -ItemType Junction -Path $sourceRuntimeLink -Target $runtimeRoot | Out-Null

    Push-Location $desktopRoot
    try {
        & npm.cmd run tauri -- build --no-bundle --config src-tauri/tauri.portable.conf.json
        if ($LASTEXITCODE -ne 0) {
            throw "Tauri portable build failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }

    if (-not (Test-Path -LiteralPath $releaseExecutable -PathType Leaf)) {
        throw "Tauri release executable is missing: $releaseExecutable"
    }

    Copy-Item -LiteralPath $releaseExecutable -Destination (Join-Path $packageStaging 'SemanticEngine.exe')
    Copy-Item -LiteralPath $runtimeRoot -Destination (Join-Path $packageStaging 'WebView2') -Recurse

    $launcher = @'
@echo off
setlocal
set "APP_ROOT=%~dp0"
icacls "%APP_ROOT%WebView2" /grant *S-1-15-2-2:(OI)(CI)(RX) /T /C /Q >nul 2>&1
icacls "%APP_ROOT%WebView2" /grant *S-1-15-2-1:(OI)(CI)(RX) /T /C /Q >nul 2>&1
start "" "%APP_ROOT%SemanticEngine.exe"
'@
    [IO.File]::WriteAllText(
        (Join-Path $packageStaging 'Start-SemanticEngine.cmd'),
        ($launcher -replace "`n", "`r`n"),
        [Text.Encoding]::ASCII
    )

    $readme = @"
Semantic Engine portable hors ligne
====================================

Lancer Start-SemanticEngine.cmd.

Cette distribution contient Microsoft Edge WebView2 Fixed Version
$($runtimeLock.version) $($runtimeLock.architecture). Elle ne nécessite ni
installation ni téléchargement au premier lancement.

Ne pas lancer depuis un chemin réseau ou UNC. Les données opérateur restent
dans l'AppData local Windows ; déplacer ce dossier ne déplace pas les données.

WebView2 est redistribué selon les conditions présentes dans le dossier
WebView2. Source officielle : $($runtimeLock.source)
"@
    [IO.File]::WriteAllText(
        (Join-Path $packageStaging 'LISEZ-MOI.txt'),
        ($readme -replace "`n", "`r`n"),
        [Text.UTF8Encoding]::new($false)
    )

    $packageStagingPrefix = $packageStaging.TrimEnd('\') + '\'
    $hashLines = Get-ChildItem -LiteralPath $packageStaging -Recurse -File |
        Where-Object Name -ne 'SHA256SUMS.txt' |
        Sort-Object FullName |
        ForEach-Object {
            $relativePath = $_.FullName.Substring($packageStagingPrefix.Length).Replace('\', '/')
            $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            "$hash  $relativePath"
        }
    [IO.File]::WriteAllLines(
        (Join-Path $packageStaging 'SHA256SUMS.txt'),
        $hashLines,
        [Text.UTF8Encoding]::new($false)
    )

    New-Item -ItemType Directory -Force -Path (Split-Path $outputPath) | Out-Null
    Move-Item -LiteralPath $packageStaging -Destination $outputPath

    $rootLauncher = @'
@echo off
call "%~dp0portable\SemanticEngine\Start-SemanticEngine.cmd"
'@
    [IO.File]::WriteAllText(
        (Join-Path $repoRoot 'SemanticEngine Portable.cmd'),
        ($rootLauncher -replace "`n", "`r`n"),
        [Text.Encoding]::ASCII
    )

    if ($Archive) {
        $archivePath = "$outputPath.zip"
        Compress-Archive -LiteralPath $outputPath -DestinationPath $archivePath -CompressionLevel Optimal
        Write-Output "Archive: $archivePath"
    }

    Write-Output "Portable directory: $outputPath"
    Write-Output "WebView2 CAB SHA256: $cabHash"
}
finally {
    if (Test-Path -LiteralPath $sourceRuntimeLink) {
        [IO.Directory]::Delete($sourceRuntimeLink)
    }
    if (Test-Path -LiteralPath $temporaryRoot) {
        $resolvedTemporaryRoot = (Resolve-Path -LiteralPath $temporaryRoot).Path
        $expectedPrefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
        if (-not $resolvedTemporaryRoot.StartsWith($expectedPrefix)) {
            throw "Unsafe temporary cleanup target: $resolvedTemporaryRoot"
        }
        Remove-Item -LiteralPath $resolvedTemporaryRoot -Recurse -Force
    }
}
