#!/usr/bin/env pwsh
# Exercise PDX-1 from the checked-in external fixture: install the packaged SDK,
# typecheck, test, build, sign, package, review, and install into a temporary
# Steward data/plugin directory.

param(
    [string]$SdkArtifact = "",
    [string]$Fixture = (Join-Path $PSScriptRoot "..\fixtures\pdx1-external-plugin")
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$FixtureSource = (Resolve-Path -LiteralPath $Fixture).Path
$compatibility = Get-Content -Raw -Encoding UTF8 -LiteralPath (
    Join-Path $root "contracts\plugin-compatibility.json"
) | ConvertFrom-Json
$sdkVersion = [string]$compatibility.current.sdkVersion
$sdkArtifactName = "steward-extension-api-$sdkVersion.tgz"
if (-not $SdkArtifact) {
    $SdkArtifact = Join-Path $root "target\sdk\$sdkArtifactName"
}
$SdkArtifact = (Resolve-Path -LiteralPath $SdkArtifact).Path

foreach ($command in @("corepack", "cargo", "nsh")) {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "$command is required for the PDX-1 external lifecycle test"
    }
}

$tempRoot = Join-Path $root ("target\pdx1-external-" + [guid]::NewGuid().ToString("N"))
$externalTarget = Join-Path $tempRoot "target"
$Fixture = Join-Path $tempRoot "fixtures\pdx1-external-plugin"
New-Item -ItemType Directory -Force -Path $Fixture | Out-Null
New-Item -ItemType Directory -Force -Path $externalTarget | Out-Null
try {
    Get-ChildItem -Force -LiteralPath $FixtureSource |
        Where-Object { $_.Name -notin @("node_modules", ".nsh", "dist") } |
        Copy-Item -Destination $Fixture -Recurse
    $externalSdk = Join-Path $externalTarget "sdk"
    New-Item -ItemType Directory -Force -Path $externalSdk | Out-Null
    Copy-Item -LiteralPath $SdkArtifact -Destination $externalSdk

    Push-Location $Fixture
    try {
        $packageJson = Get-Content -Raw -Encoding UTF8 -LiteralPath "package.json" | ConvertFrom-Json
        $sdkSpec = [string]$packageJson.dependencies."@steward/extension-api"
        if ($sdkSpec -notlike "file:*") {
            throw "external fixture must install a packaged SDK tarball, found '$sdkSpec'"
        }

        $previousCi = $env:CI
        try {
            $env:CI = "true"
            # The artifact is rebuilt in this job, so its tarball checksum can
            # differ between runners. Refresh the temporary fixture lock while
            # still requiring pnpm to install and inspect the exact local file.
            & corepack pnpm install `
                --ignore-workspace `
                --update-checksums `
                --store-dir (Join-Path $tempRoot "pnpm-store") `
                --package-import-method copy
            if ($LASTEXITCODE -ne 0) {
                throw "external fixture dependency installation failed"
            }
        } finally {
            $env:CI = $previousCi
        }
        $module = Join-Path $Fixture "node_modules\@steward\extension-api"
        if (-not (Test-Path -LiteralPath (Join-Path $module "dist\host.js"))) {
            throw "external fixture did not install compiled SDK JavaScript"
        }
        if (-not (Test-Path -LiteralPath (Join-Path $module "dist\host.d.ts"))) {
            throw "external fixture did not install SDK declarations"
        }
        if (Test-Path -LiteralPath (Join-Path $module "src\host.ts")) {
            throw "external fixture imported raw SDK source"
        }

        & nsh check App.tsx
        if ($LASTEXITCODE -ne 0) {
            throw "external fixture typecheck failed"
        }
        & nsh test .
        if ($LASTEXITCODE -ne 0) {
            throw "external fixture headless tests failed"
        }
        & nsh build App.tsx --release -o dist
        if ($LASTEXITCODE -ne 0) {
            throw "external fixture build failed"
        }
    } finally {
        Pop-Location
    }

    $signing = Join-Path $tempRoot "signing"
    New-Item -ItemType Directory -Force -Path $signing | Out-Null
    $rootKey = Join-Path $signing "root"
    $releaseKey = Join-Path $signing "release"
    $certificate = Join-Path $signing "release.cert"
    & nsh keygen $rootKey
    if ($LASTEXITCODE -ne 0) {
        throw "development root key generation failed"
    }
    & nsh keygen $releaseKey
    if ($LASTEXITCODE -ne 0) {
        throw "development release key generation failed"
    }
    & nsh certify `
        --root "$rootKey.secret" `
        --key "$releaseKey.pub" `
        --apps "com.example.*" `
        --days 1 `
        -o $certificate
    if ($LASTEXITCODE -ne 0) {
        throw "development release certificate failed"
    }

    $signedPackage = Join-Path $Fixture "dist\plugin.nshpkg"
    & nsh publish `
        (Join-Path $Fixture "App.tsx") `
        --key "$releaseKey.secret" `
        --cert $certificate `
        -o $signedPackage
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $signedPackage)) {
        throw "external fixture signing failed"
    }
    & nsh verify $signedPackage --root "$rootKey.pub"
    if ($LASTEXITCODE -ne 0) {
        throw "external fixture package did not verify against the development root"
    }

    $manifest = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $Fixture "plugin.json") |
        ConvertFrom-Json
    $staging = Join-Path $tempRoot ("package\" + $manifest.id)
    New-Item -ItemType Directory -Force -Path $staging | Out-Null
    Copy-Item -LiteralPath (Join-Path $Fixture "plugin.json") -Destination $staging
    Copy-Item -LiteralPath $signedPackage -Destination $staging

    $archivePath = Join-Path $tempRoot "pdx1-external-plugin.zip"
    $zipStream = [System.IO.File]::Open(
        $archivePath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $zip = New-Object System.IO.Compression.ZipArchive(
            $zipStream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $false
        )
        try {
            foreach ($fileName in @("plugin.json", "plugin.nshpkg")) {
                $entry = $zip.CreateEntry(
                    "$($manifest.id)/$fileName",
                    [System.IO.Compression.CompressionLevel]::Optimal
                )
                $input = [System.IO.File]::OpenRead((Join-Path $staging $fileName))
                try {
                    $output = $entry.Open()
                    try {
                        $input.CopyTo($output)
                    } finally {
                        $output.Dispose()
                    }
                } finally {
                    $input.Dispose()
                }
            }
        } finally {
            $zip.Dispose()
        }
    } finally {
        $zipStream.Dispose()
    }

    $dataDir = Join-Path $tempRoot "data"
    $pluginsDir = Join-Path $tempRoot "plugins"
    $trustRoot = (Get-Content -Raw -Encoding UTF8 -LiteralPath "$rootKey.pub").Trim()
    $pluginArgs = @(
        "run", "--quiet", "-p", "steward-plugin-cli", "--",
        "--data-dir", $dataDir,
        "--plugins-dir", $pluginsDir,
        "--trust-root", $trustRoot
    )
    & cargo @pluginArgs review $archivePath
    if ($LASTEXITCODE -ne 0) {
        throw "external fixture failed public admission review"
    }
    & cargo @pluginArgs check $archivePath --strict
    if ($LASTEXITCODE -ne 0) {
        throw "external fixture failed strict package checks"
    }
    & cargo @pluginArgs install $archivePath
    if ($LASTEXITCODE -ne 0) {
        throw "external fixture installation failed"
    }

    $installed = Join-Path $pluginsDir $manifest.id
    $installedFiles = @(Get-ChildItem -LiteralPath $installed -File)
    if (
        $installedFiles.Count -ne 2 -or
        -not (Test-Path -LiteralPath (Join-Path $installed "plugin.json")) -or
        -not (Test-Path -LiteralPath (Join-Path $installed "plugin.nshpkg"))
    ) {
        throw "installed plugin directory must contain exactly plugin.json and plugin.nshpkg"
    }
    $catalog = & cargo @pluginArgs list --json
    if ($LASTEXITCODE -ne 0) {
        throw "could not list the installed external fixture"
    }
    $parsed = $catalog | ConvertFrom-Json
    if ($parsed.installed[0].manifest.id -ne $manifest.id) {
        throw "installed catalog does not contain $($manifest.id)"
    }

    $scaffold = Join-Path $tempRoot "scaffold"
    $scaffoldSdk = "file:" + ((Join-Path $externalSdk $sdkArtifactName) -replace "\\", "/")
    & cargo run --quiet -p steward-plugin-cli -- new `
        $scaffold `
        --id "com.example.pdx1-scaffold" `
        --name "PDX-1 Scaffold" `
        --sdk $scaffoldSdk
    if ($LASTEXITCODE -ne 0) {
        throw "steward-plugin new failed"
    }
    $scaffoldPackage = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $scaffold "package.json") |
        ConvertFrom-Json
    $scaffoldDependency = [string]$scaffoldPackage.dependencies."@steward/extension-api"
    if ($scaffoldDependency -like "workspace:*" -or $scaffoldDependency -like "*src/host.ts") {
        throw "scaffold uses workspace or raw SDK source"
    }
    Push-Location $scaffold
    try {
        $previousCi = $env:CI
        try {
            $env:CI = "true"
            & corepack pnpm install `
                --ignore-workspace `
                --store-dir (Join-Path $tempRoot "pnpm-store") `
                --package-import-method copy
            if ($LASTEXITCODE -ne 0) {
                throw "generated scaffold dependency installation failed"
            }
        } finally {
            $env:CI = $previousCi
        }
        & nsh test .
        if ($LASTEXITCODE -ne 0) {
            throw "generated scaffold interaction and host-call test failed"
        }
    } finally {
        Pop-Location
    }

    Write-Host "PDX-1 external lifecycle passed: $SdkArtifact"
} finally {
    if (Test-Path -LiteralPath $tempRoot) {
        Remove-Item -Recurse -Force -LiteralPath $tempRoot
    }
}
