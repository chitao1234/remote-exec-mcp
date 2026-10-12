param([Parameter(Mandatory = $true)][string]$OutputHeader)

$ErrorActionPreference = 'Stop'
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$projectDir = Split-Path -Parent $scriptDir
$revision = ''
$git = Get-Command git -ErrorAction SilentlyContinue
if ($git) {
    try {
        $revisionOutput = & $git.Path -C $projectDir rev-parse --short=12 HEAD 2>$null
        if ($LASTEXITCODE -eq 0) {
            $revision = ([string]$revisionOutput).Trim()
        }
    } catch {
        # Source archives and build machines without Git use the date alone.
        $revision = ''
    }
}
$buildDate = (Get-Date).ToUniversalTime().ToString('yyyyMMdd', [System.Globalization.CultureInfo]::InvariantCulture)
$metadata = "+$buildDate"
if ($revision -match '^[0-9a-fA-F]+$') {
    $metadata = "+g$revision.$buildDate"
}

$header = [System.IO.Path]::GetFullPath($OutputHeader)
$content = "#pragma once`n`n#define REMOTE_EXEC_CPP_BUILD_METADATA `"$metadata`"`n"
if (!(Test-Path -LiteralPath $header) -or [System.IO.File]::ReadAllText($header) -ne $content) {
    [System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($header)) | Out-Null
    [System.IO.File]::WriteAllText($header, $content, [System.Text.Encoding]::ASCII)
}
