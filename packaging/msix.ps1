# P16: the MSIX packaging layer for the product binary (plan 8-R17, sequence "fixed
# in R17", first half - everything that does not touch this machine's system state).
#
#   packaging\msix.ps1                  builds stage/ and dist/*.msix, prints a line
#                                       per measurement with a [P16] prefix
#
# Deliberately NOT done here: creating a certificate, signing, and Add-AppxPackage.
# Those three write outside this directory (certificate store, the user's package
# registry, %PROGRAMFILES%\WindowsApps) and are authorised separately - R17's steps
# 3 to 6. Everything up to step 2 is reproducible from this file alone.
#
# The windeployqt line is the one pinned by plan 7.2 (measured 1316 files / 66.78
# MiB / 0 warnings on the spike binary), applied here to the *product* binary for the
# first time. --qmldir is not decoration: this app's QML lives inside the executable
# as qrc resources, so without the source directory to scan, windeployqt ships a
# package that starts and draws nothing (plan 8-R14).

$ErrorActionPreference = 'Stop'

# This machine's PowerShell 5.1 has no Get-FileHash (measured: `Get-Command Get-FileHash`
# returns nothing under -NoProfile), so the digest used for the round trip is SHA-256
# straight off .NET.
function Get-Sha256([string]$path) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $fs = [System.IO.File]::OpenRead($path)
    try { (($sha.ComputeHash($fs) | ForEach-Object { $_.ToString('x2') }) -join '') }
    finally { $fs.Dispose(); $sha.Dispose() }
}

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$qt = if ($env:QTDIR) { $env:QTDIR } else { 'C:\Users\baiyl3\dev\qt\6.10.1\msvc2022_64' }
$sdk = 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64'
$exe = Join-Path $root 'target\release\falconshot.exe'
$stage = Join-Path $root 'packaging\stage'
$dist = Join-Path $root 'packaging\dist'
$qml = Join-Path $root 'crates\qt_bridge\qml'

foreach ($p in @($exe, $qt, $sdk, $qml)) {
    if (-not (Test-Path $p)) { throw "missing $p" }
}

# ---- version guard: the manifest must not silently lag the workspace ----
$cargo = Get-Content (Join-Path $root 'Cargo.toml') -Raw
$wsVersion = [regex]::Match($cargo, 'version\s*=\s*"([^"]+)"').Groups[1].Value
$manifestSrc = Get-Content (Join-Path $PSScriptRoot 'AppxManifest.xml') -Raw
$pkgVersion = [regex]::Match($manifestSrc, 'Version="([^"]+)"').Groups[1].Value
Write-Output "[P16] version workspace=$wsVersion manifest=$pkgVersion"
if (-not $pkgVersion.StartsWith("$wsVersion.")) {
    throw "AppxManifest.xml Identity Version does not carry the workspace version"
}

# ---- stage: fresh, so the file count below means what it says ----
# Empty the stage rather than deleting the directory: a shell whose cwd is inside it
# holds a handle on the directory and Remove-Item -Recurse dies with "being used by
# another process" on the very first line. The contents are what must be fresh.
if (Test-Path $stage) {
    Get-ChildItem $stage -Force -Recurse | Remove-Item -Force -Recurse -ErrorAction SilentlyContinue
    if ((Get-ChildItem $stage -Force -Recurse).Count -gt 0) { throw 'stage is not empty and not reclaimable' }
} else {
    New-Item -ItemType Directory -Path $stage | Out-Null
}
New-Item -ItemType Directory -Path (Join-Path $stage 'Assets') | Out-Null

$exeInfo = Get-Item $exe
Write-Output "[P16] exe size=$($exeInfo.Length)B mtime=$($exeInfo.LastWriteTime.ToString('yyyy-MM-dd HH:mm:ss'))"
Copy-Item $exe (Join-Path $stage 'falconshot.exe') -Force

# ---- Qt payload, the pinned line ----
$wdq = Join-Path $qt 'bin\windeployqt.exe'
$wdqArgs = @(
    '--release', '--dir', $stage, '--qmldir', $qml,
    '--no-compiler-runtime', '--no-system-dxc-compiler', '--no-opengl-sw',
    '--no-translations', '--skip-plugin-types', 'qmltooling',
    (Join-Path $stage 'falconshot.exe')
)
$log = & $wdq @wdqArgs 2>&1
$wdqExit = $LASTEXITCODE
Write-Output "[P16] windeployqt exit=$wdqExit lines=$($log.Count)"
$log | Where-Object { $_ -match 'warn|error|unknown|Unable|not found' } |
    ForEach-Object { Write-Output "[P16] wdq> $_" }

# ---- manifest + tile assets ----
Copy-Item (Join-Path $PSScriptRoot 'AppxManifest.xml') (Join-Path $stage 'AppxManifest.xml') -Force
foreach ($a in Get-ChildItem (Join-Path $PSScriptRoot 'assets\*.png')) {
    Copy-Item $a.FullName (Join-Path $stage 'Assets') -Force
    Write-Output "[P16] asset $($a.Name) $($a.Length)B"
}

# ---- CRT: only what the import tables actually name ----
#
# Two candidates exist for MSVC's runtime under MSIX (plan 8-R17 item 2): ship the
# DLLs app-local, or declare a VCLibs framework PackageDependency. This run measures
# the first one, and measures *which files* rather than copying the redist folder:
# the demand is read out of the binaries' own import tables.
$crt = Get-ChildItem "$env:ProgramFiles\Microsoft Visual Studio\2022\Community\VC\Redist\MSVC\*\x64\Microsoft.VC143.CRT" -Directory |
    Where-Object { $_.Parent.Parent.Name -match '^\d+\.\d+\.\d+$' } |
    Sort-Object { [version]$_.Parent.Parent.Name } -Descending | Select-Object -First 1
if (-not $crt) { throw 'no Microsoft.VC143.CRT redist directory found' }
Write-Output "[P16] crt-source $($crt.FullName)"

$dumpbin = Get-ChildItem "$env:ProgramFiles\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe" |
    Select-Object -First 1
if (-not $dumpbin) { throw 'no dumpbin.exe' }

$binaries = @(Get-ChildItem $stage -Recurse -Include *.exe, *.dll)
$imports = & $dumpbin /imports $binaries.FullName
$names = [regex]::Matches(($imports -join "`n"), '(?im)^\s*([\w.-]+\.(?:dll|api))\s*$') |
    ForEach-Object { $_.Groups[1].Value.ToLower() } | Sort-Object -Unique
$need = $names | Where-Object { $_ -match '^(msvcp140|vcruntime140|concrt140|vccorlib140)' }
Write-Output "[P16] binaries=$($binaries.Count) distinct-imported-modules=$($names.Count) crt-named=$($need.Count)"
foreach ($n in $need) {
    $src = Get-ChildItem (Join-Path $crt.FullName $n) -ErrorAction SilentlyContinue
    if (-not $src) { $src = Get-ChildItem (Join-Path $crt.FullName "*$n*") -ErrorAction SilentlyContinue | Select-Object -First 1 }
    if ($src) {
        Copy-Item $src.FullName (Join-Path $stage $src.Name) -Force
        Write-Output "[P16] crt-copy $($src.Name) $((Get-Item (Join-Path $stage $src.Name)).Length)B"
    } else {
        Write-Output "[P16] crt-MISSING $n (imported, but not in the redist directory)"
    }
}
$ucrt = (Get-ChildItem "$env:SystemRoot\System32\downlevel\api-ms-win-crt-*.dll" -ErrorAction SilentlyContinue).Count
Write-Output "[P16] ucrt-downlevel-in-OS=$ucrt"

# ---- stage inventory ----
$files = Get-ChildItem $stage -Recurse -File
$bytes = ($files | Measure-Object Length -Sum).Sum
Write-Output "[P16] stage files=$($files.Count) bytes=$bytes ($([math]::Round($bytes/1MB,2)) MiB)"

# ---- pack ----
if (-not (Test-Path $dist)) { New-Item -ItemType Directory -Path $dist | Out-Null }
$msix = Join-Path $dist "falconshot_${pkgVersion}_x64.msix"
if (Test-Path $msix) { Remove-Item $msix -Force }
$packLog = & (Join-Path $sdk 'makeappx.exe') pack /d $stage /p $msix /v 2>&1
$packExit = $LASTEXITCODE
# /v prints one line per packed file (549 KB of log for 1394 files), which buries the
# verdict. Count the lines, print only the ones that carry a number or a problem.
Write-Output "[P16] makeappx pack exit=$packExit lines=$($packLog.Count)"
$packLog | Where-Object { "$_" -match 'Packing|hash method|Memory limit|manifest for|error|Error|warning|Warning|succeeded|failed' } |
    ForEach-Object { Write-Output "[P16] pack> $_" }
if ($packExit -ne 0 -or -not (Test-Path $msix)) { throw 'pack failed' }
$msixInfo = Get-Item $msix
Write-Output "[P16] msix $([System.IO.Path]::GetFileName($msix)) size=$($msixInfo.Length)B ($([math]::Round($msixInfo.Length/1MB,2)) MiB)"

# ---- round trip: unpack and compare against the stage, file by file ----
#
# ComparePackage.exe is on disk but throws FileNotFoundException for
# Microsoft.Win32.Registry on this machine, so the comparison is done here: same
# relative paths, same SHA-256. It proves the package carries what was staged, which
# is all a pre-install step can prove.
$unpacked = Join-Path $root 'packaging\unpacked'
if (Test-Path $unpacked) {
    Get-ChildItem $unpacked -Force -Recurse | Remove-Item -Force -Recurse -ErrorAction SilentlyContinue
} else {
    New-Item -ItemType Directory -Path $unpacked | Out-Null
}
$unLog = & (Join-Path $sdk 'makeappx.exe') unpack /p $msix /d $unpacked 2>&1
Write-Output "[P16] makeappx unpack exit=$($LASTEXITCODE)"
$unLog | Where-Object { "$_" -match 'error|succeeded|warning|FAILED' } | ForEach-Object { Write-Output "[P16] unpack> $_" }

$before = @{}
foreach ($f in $files) {
    $rel = $f.FullName.Substring($stage.Length + 1).ToLower()
    if ($rel -ne 'appxmanifest.xml') { $before[$rel] = Get-Sha256 $f.FullName }
}
$after = @{}
foreach ($f in (Get-ChildItem $unpacked -Recurse -File)) {
    $rel = $f.FullName.Substring($unpacked.Length + 1).ToLower()
    $after[$rel] = Get-Sha256 $f.FullName
}
$missing = @($before.Keys | Where-Object { -not $after.ContainsKey($_) })
$extra = @($after.Keys | Where-Object { -not $before.ContainsKey($_) })
$diff = @($before.Keys | Where-Object { $after.ContainsKey($_) -and $after[$_] -ne $before[$_] })
$madeManifest = $after.ContainsKey('appxmanifest.xml')
# makeappx is allowed to rewrite a manifest; whether it did is a measurement, not an
# assumption - the comments and every attribute are compared after line endings.
$srcManifest = (Get-Content (Join-Path $PSScriptRoot 'AppxManifest.xml') -Raw) -replace "`r", ''
$outManifest = (Get-Content (Join-Path $unpacked 'AppxManifest.xml') -Raw) -replace "`r", ''
$manifestSame = ($srcManifest -eq $outManifest)
Write-Output "[P16] roundtrip staged=$($before.Count) in-package=$($after.Count) missing=$($missing.Count) extra=$($extra.Count) hash-diff=$($diff.Count) manifest-present=$madeManifest manifest-unchanged=$manifestSame"
$missing | Select-Object -First 5 | ForEach-Object { Write-Output "[P16] missing> $_" }
$extra | Select-Object -First 5 | ForEach-Object { Write-Output "[P16] extra> $_" }
$diff | Select-Object -First 5 | ForEach-Object { Write-Output "[P16] differs> $_" }

# ---- the R14 hole, checked against the package instead of the directory ----
#
# "Starts fine, draws nothing" is the failure plan 8-R14 records for a package that
# is missing its QML modules. A directory listing of the stage proves nothing about
# the package, so these eight paths are looked up in the *unpacked* set: the four
# `qmldir` files the app's imports resolve through, the platform plugin, two of the
# Qt DLLs the LGPL argument rests on being separable, and the binary itself.
$must = @(
    'qml\qtquick\controls\qmldir', 'qml\qtquick\window\qmldir',
    'qml\qtquick\qmldir', 'qml\qtqml\qmldir',
    'platforms\qwindows.dll', 'qt6core.dll', 'qt6quick.dll', 'falconshot.exe'
)
$absent = @($must | Where-Object { -not $after.ContainsKey($_) })
$absent | ForEach-Object { Write-Output "[P16] payload-ABSENT $_" }
Write-Output "[P16] payload required=$($must.Count) present-in-package=$($must.Count - $absent.Count)"

# ---- what signing still costs ----
# An unsigned package makes signtool write its complaint to stderr, which under
# ErrorActionPreference=Stop would end the script on a measurement that is expected
# to fail. This one call is run with Continue so the number gets printed either way.
$ErrorActionPreference = 'Continue'
$verify = & (Join-Path $sdk 'signtool.exe') verify /pa /all $msix 2>&1
$ErrorActionPreference = 'Stop'
Write-Output "[P16] signtool verify exit=$($LASTEXITCODE)"
$verify | Select-Object -First 6 | ForEach-Object { Write-Output "[P16] sign> $_" }

$ok = ($packExit -eq 0) -and ($missing.Count -eq 0) -and ($diff.Count -eq 0) -and $madeManifest -and ($absent.Count -eq 0)
Write-Output "[P16] msix pack: $(if ($ok) { 'PASS' } else { 'FAIL' })"
exit $(if ($ok) { 0 } else { 1 })
