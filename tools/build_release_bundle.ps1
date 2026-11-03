# Build di release di Lume e zip di distribuzione con i language server.
#
#   pwsh tools/build_release_bundle.ps1            # lume.exe + zip + .sha256 + manifest
#   pwsh tools/build_release_bundle.ps1 -SoloExe   # solo lume.exe (CI)
#
# I percorsi di CARGO_HOME, RUSTUP_HOME e del repo si rimappano: finirebbero
# nelle stringhe dei panic e quindi nel crash log. Poi si controlla che il
# nome utente non compaia nell'eseguibile.
#
# L'artefatto è lume-<versione>-windows-x86_64-<sha12>.zip e nasce solo da un
# albero Git pulito. Accanto scrive .sha256 e la copia di MANIFEST.json; non
# sovrascrive output già esistenti. Logica simmetrica a lib_release.sh, ma
# questo script è verificato solo per lettura: PowerShell non è installato
# nell'ambiente di sviluppo Unix.
param(
    [string]$OutputDirectory = "artifacts",
    # Solo lume.exe, senza lo zip: lo zip scarica i language server dalla rete.
    [switch]$SoloExe
)

$ErrorActionPreference = "Stop"
$repositoryRoot = Split-Path -Parent $PSScriptRoot
# Verificare prima di compilare: una release deve provenire da un commit pulito.
$stato = & git -C $repositoryRoot status --porcelain --untracked-files=normal
if ($LASTEXITCODE -ne 0) { throw "impossibile verificare lo stato Git" }
if ($stato) { throw "release rifiutata: albero Git non pulito`n$($stato -join "`n")" }

$commit = (& git -C $repositoryRoot rev-parse --verify HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $commit -notmatch '^[0-9a-f]{40}$') {
    throw "impossibile leggere lo SHA completo di HEAD: $commit"
}
$sha12 = $commit.Substring(0, 12)
$versione = (Select-String -Path (Join-Path $repositoryRoot "Cargo.toml") -Pattern '^version = "(.+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $versione) { throw "versione non trovata in Cargo.toml" }

$triple = (& rustc -vV | Where-Object { $_ -like "host: *" } | Select-Object -First 1)
if ($LASTEXITCODE -ne 0 -or -not $triple) { throw "impossibile leggere la triple host da rustc -vV" }
$triple = $triple.Substring(6).Trim()
$rustcVersion = (& rustc --version).Trim()
if ($LASTEXITCODE -ne 0 -or -not $rustcVersion) { throw "impossibile leggere rustc --version" }
$cargoVersion = (& cargo --version).Trim()
if ($LASTEXITCODE -ne 0 -or -not $cargoVersion) { throw "impossibile leggere cargo --version" }
$featuresRaw = if ($env:LUME_RELEASE_FEATURES) { $env:LUME_RELEASE_FEATURES } else { "default" }
$features = @($featuresRaw.Split(",") | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($features.Count -eq 0) { throw "LUME_RELEASE_FEATURES non contiene feature" }

$outputRoot = if ([System.IO.Path]::IsPathRooted($OutputDirectory)) {
    [System.IO.Path]::GetFullPath($OutputDirectory)
} else {
    Join-Path $repositoryRoot $OutputDirectory
}
$bundleName = "lume-$versione-windows-x86_64-$sha12.zip"
$bundle = Join-Path $outputRoot $bundleName
$checksum = "$bundle.sha256"
$manifestPath = "$bundle.MANIFEST.json"
$exe = Join-Path $repositoryRoot "target\release\lume.exe"

function Assert-Nuovo([string]$percorso) {
    if (Test-Path -LiteralPath $percorso) {
        throw "release rifiutata: output già esistente: $percorso"
    }
}

$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME ".cargo" }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $HOME ".rustup" }
$flags = @(
    "--remap-path-prefix=$cargoHome=/cargo",
    "--remap-path-prefix=$rustupHome=/rustup",
    "--remap-path-prefix=$repositoryRoot=/lume"
)
# CARGO_ENCODED_RUSTFLAGS separa i flag con 0x1F: un percorso con spazi non
# li spezza, come farebbe RUSTFLAGS. Se è impostata, cargo ignora RUSTFLAGS.
$env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1F
# Il C di aws-lc-sys (cc, non rustc) incorpora __FILE__ con il percorso di CARGO_HOME:
# /d1trimfile lo toglie da MSVC, anche nella forma corta 8.3 (C:\Users\NOME~1\CARGO~1)
# con cui alcuni file compaiono. Le virgolette e le barre raddoppiate servono a cc con
# CC_SHELL_ESCAPED_FLAGS, altrimenti spezza il flag su un percorso con spazi.
$fso = New-Object -ComObject Scripting.FileSystemObject
$prefissiC = @([System.IO.Path]::GetFullPath($cargoHome).TrimEnd('\'))
if (Test-Path -LiteralPath $prefissiC[0]) { $prefissiC += $fso.GetFolder($prefissiC[0]).ShortPath }
$env:CC_SHELL_ESCAPED_FLAGS = "1"
$env:CFLAGS = ($prefissiC | Select-Object -Unique | ForEach-Object {
    '"/d1trimfile:' + $_.Replace('\', '\\') + '"'
}) -join " "

Push-Location $repositoryRoot
try {
    # Solo lume-app: con --workspace la feature `testing` di lume-lsp, nei suoi
    # default, verrebbe unificata anche nella build dell'exe.
    cargo build --release --locked -p lume-app
    if ($LASTEXITCODE -ne 0) { throw "build di release fallita" }

    $utente = $env:USERNAME
    if ($utente -and $utente.Length -ge 4) {
        $byte = [System.IO.File]::ReadAllBytes($exe)
        # Anche il nome corto 8.3 della cartella utente (es. WONDER~1) rivela il nome.
        $nomi = @($utente)
        if ($env:USERPROFILE -and (Test-Path -LiteralPath $env:USERPROFILE)) {
            $corto = $fso.GetFolder($env:USERPROFILE).ShortName
            if ($corto -like "*~*") { $nomi += $corto }
        }
        $ago = ($nomi | ForEach-Object { [regex]::Escape($_) }) -join "|"
        # Le stringhe wide del C (assert) sono UTF-16LE, con allineamento pari o dispari: la
        # lettura a 8 bit non le vede.
        $testo8 = [System.Text.Encoding]::GetEncoding(28591).GetString($byte)
        $testo16 = [System.Text.Encoding]::Unicode.GetString($byte, 0, $byte.Length - ($byte.Length % 2))
        $testo16b = [System.Text.Encoding]::Unicode.GetString($byte, 1, ($byte.Length - 1) - (($byte.Length - 1) % 2))
        $occ8 = [regex]::Matches($testo8, $ago, "IgnoreCase").Count
        $occ16 = [regex]::Matches($testo16, $ago, "IgnoreCase").Count + [regex]::Matches($testo16b, $ago, "IgnoreCase").Count
        $occorrenze = $occ8 + $occ16
        Write-Output "nome utente «$utente» in lume.exe: $occorrenze occorrenze ($occ8 a 8 bit + $occ16 UTF-16)"
        if ($occorrenze -gt 0) { throw "lume.exe contiene percorsi personali non rimappati" }
    } else {
        Write-Output "nome utente assente o più corto di 4 caratteri: controllo saltato"
    }
    Write-Output "lume.exe: $((Get-Item $exe).Length) byte"

    if (-not $SoloExe) {
        # Controllare prima di compilare lo zip: nulla di preesistente si tocca.
        Assert-Nuovo $bundle
        Assert-Nuovo $checksum
        Assert-Nuovo $manifestPath
        New-Item -ItemType Directory -Force $outputRoot | Out-Null
        cargo run --release --locked -p lume-language-servers --bin lume-server-bundle -- `
            --lume-exe $exe `
            --output $bundle
        if ($LASTEXITCODE -ne 0) { throw "creazione dello zip fallita" }

        # Hash di ogni file dentro Lume/ e manifest JSON nella radice del bundle.
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $files = [ordered]@{}
        $zip = [System.IO.Compression.ZipFile]::OpenRead($bundle)
        try {
            foreach ($entry in $zip.Entries) {
                $nome = $entry.FullName
                if (-not $nome.StartsWith("Lume/", [System.StringComparison]::Ordinal)) {
                    throw "voce fuori da Lume/ nello zip: $nome"
                }
                if ($nome.EndsWith("/")) { continue }
                $relativo = $nome.Substring(5)
                if ($relativo -eq "MANIFEST.json") { continue }
                $stream = $entry.Open()
                try {
                    $sha = [System.Security.Cryptography.SHA256]::Create()
                    try { $digest = $sha.ComputeHash($stream) } finally { $sha.Dispose() }
                } finally {
                    $stream.Dispose()
                }
                $hex = -join ($digest | ForEach-Object { $_.ToString("x2") })
                $files[$relativo] = [ordered]@{ sha256 = $hex; size = [int64]$entry.Length }
            }
        } finally {
            $zip.Dispose()
        }

        $manifesto = [ordered]@{
            schema = 1
            package = "lume"
            version = $versione
            commit = $commit
            build_date_utc = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
            rustc = $rustcVersion
            cargo = $cargoVersion
            target_triple = $triple
            target_label = "windows-x86_64"
            cargo_features = [object[]]$features
            files = $files
        }
        $json = $manifesto | ConvertTo-Json -Depth 10
        $utf8 = New-Object System.Text.UTF8Encoding($false)
        $manifestBytes = $utf8.GetBytes($json)
        $zip = [System.IO.Compression.ZipFile]::Open($bundle, [System.IO.Compression.ZipArchiveMode]::Update)
        try {
            $entry = $zip.CreateEntry("Lume/MANIFEST.json", [System.IO.Compression.CompressionLevel]::Optimal)
            $stream = $entry.Open()
            try { $stream.Write($manifestBytes, 0, $manifestBytes.Length) } finally { $stream.Dispose() }
        } finally {
            $zip.Dispose()
        }
        [System.IO.File]::WriteAllBytes($manifestPath, $manifestBytes)

        # Il tool ha scritto il checksum prima del manifest: ora lo zip è
        # cambiato, quindi si ricalcola il file creato in questa esecuzione.
        Remove-Item -LiteralPath $checksum
        $digest = (Get-FileHash -LiteralPath $bundle -Algorithm SHA256).Hash.ToLowerInvariant()
        [System.IO.File]::WriteAllText($checksum, "$digest  $bundleName`n", $utf8)

        Write-Output $bundle
    }
} finally {
    Pop-Location
}
