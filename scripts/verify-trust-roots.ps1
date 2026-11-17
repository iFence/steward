#!/usr/bin/env pwsh
# Validate the committed NSH production trust-root file before packaging.

param(
    [string]$Path = (Join-Path $PSScriptRoot "..\assets\nsh\trust-roots.json")
)

$ErrorActionPreference = "Stop"
$Path = (Resolve-Path -LiteralPath $Path).Path
$roots = @(
    Get-Content -Raw -Encoding UTF8 -LiteralPath $Path |
        ConvertFrom-Json
)
if ($roots.Count -eq 0) {
    throw "NSH trust-root file is empty: $Path"
}

$seen = @{}
for ($index = 0; $index -lt $roots.Count; $index++) {
    $root = [string]$roots[$index]
    if ([string]::IsNullOrWhiteSpace($root)) {
        throw "NSH trust root $index is empty"
    }
    try {
        $bytes = [Convert]::FromBase64String($root.Trim())
    } catch {
        throw "NSH trust root $index is not valid base64: $($_.Exception.Message)"
    }
    if ($bytes.Length -ne 32) {
        throw "NSH trust root $index must decode to a 32-byte Ed25519 public key; got $($bytes.Length) bytes"
    }

    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $digest = $sha256.ComputeHash($bytes)
    } finally {
        $sha256.Dispose()
    }
    $fingerprint = ($digest | ForEach-Object { $_.ToString("x2") }) -join ""
    if ($seen.ContainsKey($fingerprint)) {
        throw "NSH trust root $index duplicates root $($seen[$fingerprint])"
    }
    $seen[$fingerprint] = $index
    Write-Host "NSH trust root $index SHA-256 $fingerprint"
}
Write-Host "Validated $($roots.Count) production NSH trust root(s)."
