[CmdletBinding()]
param(
    [ValidateSet('https://github.com/material-extensions/vscode-material-icon-theme/archive/refs/tags/v5.33.1.zip')]
    [string]$MaterialArchive = 'https://github.com/material-extensions/vscode-material-icon-theme/archive/refs/tags/v5.33.1.zip',

    [ValidateSet('https://github.com/catppuccin/vscode-icons/archive/refs/tags/v1.26.0.zip')]
    [string]$CatppuccinArchive = 'https://github.com/catppuccin/vscode-icons/archive/refs/tags/v1.26.0.zip'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$assetRoot = Join-Path $repositoryRoot 'crates\lume-icons\assets'
$temporaryRoot = Join-Path $repositoryRoot ("target\icon-theme-update-{0}" -f [Guid]::NewGuid().ToString('N'))
$expectedTemporaryParent = (Resolve-Path (Join-Path $repositoryRoot 'target')).Path

$sources = @(
    [ordered]@{
        Id = 'material'
        Repository = 'https://github.com/material-extensions/vscode-material-icon-theme'
        Tag = 'v5.33.1'
        Commit = 'e6f371b1e54c49200ac0c6bc44e0728b487aff2d'
        Url = $MaterialArchive
        ArchiveSha256 = 'A876633CA2D4786F55F226458ED17404453015963345EC6609C9249D185018CC'
        Output = 'material'
        Files = [ordered]@{
            'document.svg' = 'file.svg'
            'folder-base.svg' = 'folder.svg'
            'folder-base-open.svg' = 'folder-open.svg'
            'folder-git.svg' = 'root.svg'
            'folder-git-open.svg' = 'root-open.svg'
            'java.svg' = 'java.svg'
            'maven.svg' = 'maven.svg'
            'gradle.svg' = 'gradle.svg'
            'yaml.svg' = 'yaml.svg'
            'rust.svg' = 'rust.svg'
            'toml.svg' = 'toml.svg'
            'c.svg' = 'c.svg'
            'h.svg' = 'c-header.svg'
            'cpp.svg' = 'cpp.svg'
            'hpp.svg' = 'cpp-header.svg'
            'csharp.svg' = 'csharp.svg'
            'markdown.svg' = 'markdown.svg'
            'git.svg' = 'git.svg'
            'docker.svg' = 'docker.svg'
            'license.svg' = 'license.svg'
            'folder-src.svg' = 'folder-src.svg'
            'folder-src-open.svg' = 'folder-src-open.svg'
        }
        IconPrefix = 'icons'
    },
    [ordered]@{
        Id = 'catppuccin-mocha'
        Repository = 'https://github.com/catppuccin/vscode-icons'
        Tag = 'v1.26.0'
        Commit = 'b6915da9f6889b683a110aa747de96c2820a537d'
        Url = $CatppuccinArchive
        ArchiveSha256 = '66B8E5AA73C8F030DBC2AEC034E6DA3D659C733F5A1AB9F28F0E7E9A46A7396F'
        Output = 'catppuccin-mocha'
        Files = [ordered]@{
            '_file.svg' = 'file.svg'
            '_folder.svg' = 'folder.svg'
            '_folder_open.svg' = 'folder-open.svg'
            '_root.svg' = 'root.svg'
            '_root_open.svg' = 'root-open.svg'
            'java.svg' = 'java.svg'
            'cargo.svg' = 'cargo.svg'
            'gradle.svg' = 'gradle.svg'
            'properties.svg' = 'properties.svg'
            'yaml.svg' = 'yaml.svg'
            'rust.svg' = 'rust.svg'
            'toml.svg' = 'toml.svg'
            'c.svg' = 'c.svg'
            'c-header.svg' = 'c-header.svg'
            'cpp.svg' = 'cpp.svg'
            'cpp-header.svg' = 'cpp-header.svg'
            'csharp.svg' = 'csharp.svg'
            'markdown.svg' = 'markdown.svg'
            'git.svg' = 'git.svg'
            'docker.svg' = 'docker.svg'
            'license.svg' = 'license.svg'
            'folder_src.svg' = 'folder-src.svg'
            'folder_src_open.svg' = 'folder-src-open.svg'
            'folder_git.svg' = 'folder-git.svg'
            'folder_git_open.svg' = 'folder-git-open.svg'
        }
        IconPrefix = 'icons\mocha'
    }
)

function Assert-ChildPath([string]$Parent, [string]$Child) {
    $parentFull = [IO.Path]::GetFullPath($Parent).TrimEnd('\') + '\'
    $childFull = [IO.Path]::GetFullPath($Child)
    if (-not $childFull.StartsWith($parentFull, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Path escapes expected root: $childFull"
    }
}

function Assert-SafeZip([string]$Archive) {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName.Replace('/', '\')
            if ([IO.Path]::IsPathRooted($name) -or $name.Split('\').Contains('..')) {
                throw "Unsafe archive entry: $($entry.FullName)"
            }
        }
    }
    finally {
        $zip.Dispose()
    }
}

New-Item -ItemType Directory -Path $temporaryRoot | Out-Null
try {
    New-Item -ItemType Directory -Force -Path $assetRoot | Out-Null
    $inventorySources = [ordered]@{}
    $inventoryAssets = [ordered]@{}

    foreach ($source in $sources) {
        $sourceTemp = Join-Path $temporaryRoot $source.Id
        $archivePath = Join-Path $sourceTemp 'source.zip'
        $extractPath = Join-Path $sourceTemp 'extract'
        New-Item -ItemType Directory -Path $sourceTemp, $extractPath | Out-Null

        Invoke-WebRequest -Uri $source.Url -OutFile $archivePath
        $archiveHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash
        if ($archiveHash -ne $source.ArchiveSha256) {
            throw "Archive hash mismatch for $($source.Id): $archiveHash"
        }
        Assert-SafeZip $archivePath
        Expand-Archive -LiteralPath $archivePath -DestinationPath $extractPath
        $sourceRoot = @(Get-ChildItem -LiteralPath $extractPath -Directory)
        if ($sourceRoot.Count -ne 1) {
            throw "Expected one archive root for $($source.Id)"
        }

        $outputPath = Join-Path $assetRoot $source.Output
        Assert-ChildPath $assetRoot $outputPath
        if (Test-Path -LiteralPath $outputPath) {
            Remove-Item -LiteralPath $outputPath -Recurse -Force
        }
        New-Item -ItemType Directory -Path $outputPath | Out-Null

        foreach ($upstreamName in $source.Files.Keys) {
            $upstreamRelative = Join-Path $source.IconPrefix $upstreamName
            $inputPath = Join-Path $sourceRoot.FullName $upstreamRelative
            if (-not (Test-Path -LiteralPath $inputPath -PathType Leaf)) {
                throw "Missing pinned upstream asset: $upstreamRelative"
            }
            $destinationName = $source.Files[$upstreamName]
            $destinationPath = Join-Path $outputPath $destinationName
            Assert-ChildPath $outputPath $destinationPath
            Copy-Item -LiteralPath $inputPath -Destination $destinationPath
            $relativeInventoryPath = ($source.Output + '/' + $destinationName)
            $inventoryAssets[$relativeInventoryPath] = [ordered]@{
                sha256 = (Get-FileHash -LiteralPath $destinationPath -Algorithm SHA256).Hash.ToLowerInvariant()
                upstream_path = $upstreamRelative.Replace('\', '/')
            }
        }

        $licenseSource = Join-Path $sourceRoot.FullName 'LICENSE'
        if (-not (Test-Path -LiteralPath $licenseSource -PathType Leaf)) {
            throw "Missing upstream LICENSE for $($source.Id)"
        }
        Copy-Item -LiteralPath $licenseSource -Destination (Join-Path $outputPath 'LICENSE')
        $inventorySources[$source.Id] = [ordered]@{
            repository = $source.Repository
            tag = $source.Tag
            commit = $source.Commit
            license = 'MIT'
            archive_sha256 = $source.ArchiveSha256.ToLowerInvariant()
        }
    }

    $sortedAssets = [ordered]@{}
    foreach ($key in ($inventoryAssets.Keys | Sort-Object)) {
        $sortedAssets[$key] = $inventoryAssets[$key]
    }
    $manifest = [ordered]@{
        schema = 1
        generated_by = 'tools/update_icon_themes.ps1'
        sources = $inventorySources
        assets = $sortedAssets
    }
    $json = $manifest | ConvertTo-Json -Depth 8
    [IO.File]::WriteAllText((Join-Path $assetRoot 'ASSET_MANIFEST.json'), $json + "`n", [Text.UTF8Encoding]::new($false))
}
finally {
    Assert-ChildPath $expectedTemporaryParent $temporaryRoot
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
