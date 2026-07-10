[CmdletBinding()]
param(
    [string]$Token = $env:GITEE_TOKEN,
    [string]$Owner = "M4rkzzz",
    [string]$Repository = "oopz-plus",
    [string]$Version,
    [string]$MsiPath,
    [string]$NotesFile,
    [string]$TargetCommitish = "main",
    [switch]$SkipBuild,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$packageJsonPath = Join-Path $repoRoot "package.json"
$cargoTomlPath = Join-Path $repoRoot "src-tauri/Cargo.toml"

function Get-ReleaseVersion {
    if ($Version) {
        return $Version.TrimStart("v")
    }

    $packageVersion = (Get-Content -Raw -LiteralPath $packageJsonPath | ConvertFrom-Json).version
    $cargoText = Get-Content -Raw -LiteralPath $cargoTomlPath
    $cargoMatch = [regex]::Match($cargoText, '(?m)^version\s*=\s*"([^"]+)"')
    if (-not $cargoMatch.Success) {
        throw "Could not read the Rust package version from $cargoTomlPath"
    }
    $cargoVersion = $cargoMatch.Groups[1].Value
    if ($packageVersion -ne $cargoVersion) {
        throw "Version mismatch: package.json=$packageVersion, Cargo.toml=$cargoVersion"
    }
    return $packageVersion
}

function Get-ReleaseNotes([string]$ResolvedVersion, [string]$Sha256) {
    $resolvedNotesFile = $NotesFile
    if (-not $resolvedNotesFile) {
        $candidate = Join-Path $repoRoot ".github/releases/v$ResolvedVersion.md"
        if (Test-Path -LiteralPath $candidate) {
            $resolvedNotesFile = $candidate
        }
    }

    $notes = if ($resolvedNotesFile) {
        Get-Content -Raw -LiteralPath $resolvedNotesFile
    } else {
        "OOPZ+ v$ResolvedVersion"
    }
    return "$($notes.TrimEnd())`n`nSHA-256: ``$Sha256``"
}

function Get-ExistingRelease([string]$Tag) {
    $encodedTag = [Uri]::EscapeDataString($Tag)
    $uri = "https://gitee.com/api/v5/repos/$Owner/$Repository/releases/tags/$encodedTag"
    try {
        return Invoke-RestMethod -Method Get -Uri $uri -Headers @{ Accept = "application/json" }
    } catch {
        if ($_.Exception.Response -and
            $_.Exception.Response.StatusCode -eq [System.Net.HttpStatusCode]::NotFound) {
            return $null
        }
        throw
    }
}

function Assert-Token {
    if ([string]::IsNullOrWhiteSpace($Token)) {
        throw "GITEE_TOKEN is required. Set it as an environment variable or GitHub Actions secret."
    }
}

Push-Location $repoRoot
try {
    $resolvedVersion = Get-ReleaseVersion
    if ($resolvedVersion -notmatch '^\d+\.\d+\.\d+$') {
        throw "Version must use x.y.z format: $resolvedVersion"
    }
    $tag = "v$resolvedVersion"
    $expectedName = "OOPZ+_${resolvedVersion}_x64_en-US.msi"

    if (-not $SkipBuild) {
        Write-Host "Building OOPZ+ $resolvedVersion..."
        & pnpm tauri build
        if ($LASTEXITCODE -ne 0) {
            throw "Tauri build failed with exit code $LASTEXITCODE"
        }
    }

    if (-not $MsiPath) {
        $MsiPath = Join-Path $repoRoot "src-tauri/target/release/bundle/msi/$expectedName"
    } elseif (-not [IO.Path]::IsPathRooted($MsiPath)) {
        $MsiPath = Join-Path $repoRoot $MsiPath
    }
    $MsiPath = [IO.Path]::GetFullPath($MsiPath)
    if (-not (Test-Path -LiteralPath $MsiPath -PathType Leaf)) {
        throw "MSI not found: $MsiPath"
    }
    if ([IO.Path]::GetFileName($MsiPath) -cne $expectedName) {
        throw "Unexpected MSI name. Expected $expectedName"
    }

    $msi = Get-Item -LiteralPath $MsiPath
    $sha256 = (Get-FileHash -LiteralPath $MsiPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $body = Get-ReleaseNotes $resolvedVersion $sha256
    Write-Host "Release: $tag"
    Write-Host "MSI: $($msi.FullName) ($($msi.Length) bytes)"
    Write-Host "SHA-256: $sha256"

    $release = Get-ExistingRelease $tag
    $normalizedName = $expectedName.Replace("+", " ")
    if ($release) {
        $existingAsset = @($release.assets) | Where-Object {
            $_.name -ieq $expectedName -or $_.name -ieq $normalizedName
        } | Select-Object -First 1
        if ($existingAsset) {
            $temporaryMsi = Join-Path ([IO.Path]::GetTempPath()) "oopz-plus-gitee-$([guid]::NewGuid()).msi"
            try {
                Write-Host "Verifying the existing Gitee MSI asset..."
                Invoke-WebRequest -Uri $existingAsset.browser_download_url -OutFile $temporaryMsi
                $existingHash = (Get-FileHash -LiteralPath $temporaryMsi -Algorithm SHA256).Hash.ToLowerInvariant()
                if ($existingHash -ne $sha256) {
                    throw "Gitee already contains $($existingAsset.name), but its SHA-256 does not match the local MSI."
                }
                Write-Host "Gitee release and verified MSI asset already exist; nothing to publish."
                Write-Host "https://gitee.com/$Owner/$Repository/releases/tag/$tag"
                exit 0
            } finally {
                if (Test-Path -LiteralPath $temporaryMsi) {
                    Remove-Item -LiteralPath $temporaryMsi -Force
                }
            }
        }
    }

    if ($DryRun) {
        $action = if ($release) { "upload the missing MSI asset" } else { "create the release and upload the MSI asset" }
        Write-Host "Dry run: would $action on Gitee."
        exit 0
    }

    Assert-Token
    if (-not $release) {
        Write-Host "Creating Gitee release $tag..."
        $createReleaseParams = @{
            Method      = "Post"
            Uri         = "https://gitee.com/api/v5/repos/$Owner/$Repository/releases"
            ContentType = "application/x-www-form-urlencoded"
            Body        = @{
                access_token     = $Token
                tag_name         = $tag
                name             = "OOPZ+ $tag"
                body             = $body
                prerelease       = "false"
                target_commitish = $TargetCommitish
            }
        }
        $release = Invoke-RestMethod @createReleaseParams
    }

    if (-not $release.id) {
        throw "Gitee did not return a release id."
    }
    Write-Host "Uploading $expectedName to Gitee..."
    $uploadAssetParams = @{
        Method = "Post"
        Uri    = "https://gitee.com/api/v5/repos/$Owner/$Repository/releases/$($release.id)/attach_files"
        Form   = @{
            access_token = $Token
            file         = $msi
        }
    }
    $null = Invoke-RestMethod @uploadAssetParams

    $published = Get-ExistingRelease $tag
    $uploadedAsset = @($published.assets) | Where-Object {
        $_.name -ieq $expectedName -or $_.name -ieq $normalizedName
    } | Select-Object -First 1
    if (-not $uploadedAsset) {
        throw "Gitee release was created, but the MSI asset could not be verified."
    }

    Write-Host "Published: https://gitee.com/$Owner/$Repository/releases/tag/$tag"
    Write-Host "Download: $($uploadedAsset.browser_download_url)"
} finally {
    Pop-Location
}
