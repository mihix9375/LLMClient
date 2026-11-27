param(
    [string[]]$Targets = @('all'),
    [switch]$Test,
    [switch]$Online
)
$ErrorActionPreference = 'Stop'
$guiRoot = $PSScriptRoot
$frameworkRoot = Split-Path -Parent $guiRoot
$workspaceRoot = Split-Path -Parent $frameworkRoot
$manifest = Join-Path $guiRoot 'Cargo.toml'
$distRoot = Join-Path $frameworkRoot 'dist'
$rootExe = Join-Path $frameworkRoot 'Qwen27.exe'

$validTargets = @('all', 'win-amd64', 'linux-amd64', 'linux-arm64')
$parsedTargets = @($Targets | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($parsedTargets.Count -eq 0) { $parsedTargets = @('all') }
foreach ($t in $parsedTargets) { if ($validTargets -notcontains $t) { throw "Invalid target '$t'. Allowed: $($validTargets -join ', ')" } }
$selected = if ($parsedTargets -contains 'all') { @('win-amd64', 'linux-amd64', 'linux-arm64') } else { @($parsedTargets | Select-Object -Unique) }

function Initialize-BuildEnv
{
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if (Test-Path -LiteralPath $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }

    $msvcBase = if ($env:QWEN_MSVC_BASE) { $env:QWEN_MSVC_BASE } else { 'C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC' }
    $msvcVer = if (Test-Path -LiteralPath $msvcBase) { Get-ChildItem -LiteralPath $msvcBase -Directory | Sort-Object Name -Descending | Select-Object -First 1 } else { $null }
    $kitsRoot = 'C:\Program Files (x86)\Windows Kits\10'
    $kitsLib = Join-Path $kitsRoot 'Lib'
    $sdkVer = if (Test-Path -LiteralPath $kitsLib) { Get-ChildItem -LiteralPath $kitsLib -Directory | Sort-Object Name -Descending | Select-Object -First 1 } else { $null }

    $extraPath = @()
    $extraLib = @()
    $extraInc = @()
    if ($msvcVer)
    {
        $extraPath += (Join-Path $msvcVer.FullName 'bin\Hostx64\x64')
        $extraLib += (Join-Path $msvcVer.FullName 'lib\x64')
        $extraInc += (Join-Path $msvcVer.FullName 'include')
    }
    if ($sdkVer)
    {
        $v = $sdkVer.Name
        $extraPath += (Join-Path $kitsRoot "bin\$v\x64")
        $extraLib += @((Join-Path $kitsRoot "Lib\$v\um\x64"), (Join-Path $kitsRoot "Lib\$v\ucrt\x64"))
        $extraInc += @((Join-Path $kitsRoot "Include\$v\ucrt"), (Join-Path $kitsRoot "Include\$v\um"), (Join-Path $kitsRoot "Include\$v\shared"))
    }
    if ($extraPath.Count) { $env:PATH = (($extraPath -join ';') + ';' + $env:PATH) }
    if ($extraLib.Count) { $env:LIB = (($extraLib -join ';') + $(if ($env:LIB) { ';' + $env:LIB } else { '' })) }
    if ($extraInc.Count) { $env:INCLUDE = (($extraInc -join ';') + $(if ($env:INCLUDE) { ';' + $env:INCLUDE } else { '' })) }
}

function Initialize-ZigEnv
{
    $zigExe = $null
    $candidates = @(
        $env:ZIG_EXE,
        (Join-Path $workspaceRoot 'tools\zig-0.14.1\zig.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\zig\zig.exe'),
        (Join-Path $env:USERPROFILE '.local\bin\zig.exe')
    )
    $inPath = Get-Command zig.exe -ErrorAction SilentlyContinue
    if ($inPath) { $zigExe = $inPath.Source }
    if (!$zigExe)
    {
        foreach ($cand in $candidates)
        {
            if ($cand -and (Test-Path -LiteralPath $cand)) { $zigExe = $cand; break }
        }
    }
    if (!$zigExe) { throw 'zig.exe not found. Install Zig 0.14.1 under tools\zig-0.14.1 or add zig to PATH.' }
    $zigDir = Split-Path -Parent $zigExe
    if (($env:PATH -split ';') -notcontains $zigDir) { $env:PATH = "$zigDir;$env:PATH" }
    if (!(Get-Command cargo-zigbuild.exe -ErrorAction SilentlyContinue)) { throw 'cargo-zigbuild.exe not found. Run: cargo install cargo-zigbuild --locked' }

    $zigCache = Join-Path $guiRoot 'target\zig-cache'
    if (!$env:ZIG_GLOBAL_CACHE_DIR) { $env:ZIG_GLOBAL_CACHE_DIR = Join-Path $zigCache 'global' }
    if (!$env:ZIG_LOCAL_CACHE_DIR) { $env:ZIG_LOCAL_CACHE_DIR = Join-Path $zigCache 'local' }
    if (!$env:CARGO_ZIGBUILD_CACHE_DIR) { $env:CARGO_ZIGBUILD_CACHE_DIR = Join-Path $zigCache 'zigbuild' }
    foreach ($d in @($env:ZIG_GLOBAL_CACHE_DIR, $env:ZIG_LOCAL_CACHE_DIR, $env:CARGO_ZIGBUILD_CACHE_DIR))
    {
        New-Item -ItemType Directory -Force -Path $d | Out-Null
    }
}

function Invoke-CargoNoWarnings([string]$Label, [string[]]$CargoArgs)
{
    Write-Host "=== Building $Label ==="
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try
    {
        $output = & cargo @CargoArgs 2>&1 | ForEach-Object {
            $line = if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { "$_" }
            Write-Host $line
            $line
        }
        $code = $LASTEXITCODE
    }
    finally
    {
        $ErrorActionPreference = $prevEap
    }
    if ($code -ne 0) { throw "$Label failed with exit code $code" }
    $warnLines = @($output | Where-Object {
        ($_ -match '^warning:\s+`qwen27-gui`') -or
        (($_ -match '^warning(\[[^\]]+\])?:') -and ($_ -notmatch '^warning:\s+[^`\s]+@\d+\.\d+') -and ($_ -notmatch 'hard linking files in the incremental compilation cache'))
    })
    if ($warnLines.Count -gt 0)
    {
        throw "$Label emitted $($warnLines.Count) compiler warning(s):`n$($warnLines -join "`n")"
    }
}

function Remove-OldBinary([string]$Path)
{
    $old = "$Path.old"
    if (Test-Path -LiteralPath $old) { Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue }
}

function Copy-WithDir([string]$Source, [string]$Dest)
{
    $dir = Split-Path -Parent $Dest
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Copy-Item -LiteralPath $Source -Destination $Dest -Force
}

Initialize-BuildEnv
$needLinux = ($selected -contains 'linux-amd64') -or ($selected -contains 'linux-arm64')
if ($needLinux) { Initialize-ZigEnv }

$offlineFlag = if ($Online) { @() } else { @('--offline') }
$artifacts = @()

Push-Location $guiRoot
try
{
    if ($selected -contains 'win-amd64')
    {
        $env:CARGO_ENCODED_RUSTFLAGS = (@('-C', 'link-arg=/MANIFEST:EMBED', '-C', ('link-arg=/MANIFESTINPUT:' + (Join-Path $guiRoot 'app.manifest')), ('--remap-path-prefix=' + $env:USERPROFILE + '=~'), ('--remap-path-prefix=' + $guiRoot + '=gui')) -join [char]0x1f)
        Invoke-CargoNoWarnings 'win-amd64 (x86_64-pc-windows-msvc)' (@('build', '--release') + $offlineFlag + @('--manifest-path', $manifest))
        Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
        $srcExe = Join-Path $guiRoot 'target\release\Qwen27.exe'
        if (!(Test-Path -LiteralPath $srcExe)) { throw "Built binary missing: $srcExe" }

        if (Test-Path -LiteralPath $rootExe)
        {
            Remove-OldBinary $rootExe
            Move-Item -LiteralPath $rootExe -Destination "$rootExe.old" -Force -ErrorAction SilentlyContinue
        }
        Copy-Item -LiteralPath $srcExe -Destination $rootExe -Force
        Remove-OldBinary $rootExe

        $distWin = Join-Path $distRoot 'win-amd64\Qwen27.exe'
        Copy-WithDir $rootExe $distWin

        $selfOut = (& $rootExe '--self-test' | Out-String).Trim()
        if ($LASTEXITCODE -ne 0 -or $selfOut -notmatch 'Desktop key restrictions passed \(no UI input sent\)') { throw "Desktop self-test failed: $selfOut" }
        Write-Host $selfOut
        $artifacts += @($rootExe, $distWin)
    }

    foreach ($spec in @(
        @{ Key = 'linux-amd64'; Triple = 'x86_64-unknown-linux-gnu'; ZigTarget = 'x86_64-unknown-linux-gnu.2.28'; OutRel = 'linux-amd64\qwen27' },
        @{ Key = 'linux-arm64'; Triple = 'aarch64-unknown-linux-gnu'; ZigTarget = 'aarch64-unknown-linux-gnu.2.28'; OutRel = 'linux-arm64\qwen27' }
    ))
    {
        if ($selected -notcontains $spec.Key) { continue }
        Invoke-CargoNoWarnings "$($spec.Key) ($($spec.ZigTarget))" (@('zigbuild', '--release') + $offlineFlag + @('--manifest-path', $manifest, '--target', $spec.ZigTarget))
        $relDir = Join-Path $guiRoot "target\$($spec.Triple)\release"
        $srcBin = Join-Path $relDir 'Qwen27'
        if (!(Test-Path -LiteralPath $srcBin)) { $srcBin = Join-Path $relDir 'qwen27' }
        if (!(Test-Path -LiteralPath $srcBin)) { throw "Built Linux binary missing under $relDir" }
        $destBin = Join-Path $distRoot $spec.OutRel
        Copy-WithDir $srcBin $destBin
        $artifacts += $destBin
    }

    if ($Test)
    {
        Write-Host '=== Running Windows unit & integration tests (build.ps1 -Test) ==='
        & (Join-Path $guiRoot 'build.ps1') -Test
        if ($LASTEXITCODE -ne 0) { throw "build.ps1 -Test failed with exit code $LASTEXITCODE" }
        Remove-OldBinary $rootExe
        if ($selected -contains 'win-amd64') { Copy-WithDir $rootExe (Join-Path $distRoot 'win-amd64\Qwen27.exe') }
    }
}
finally
{
    Pop-Location
}

Write-Host '=== Build artifacts ==='
foreach ($art in ($artifacts | Select-Object -Unique))
{
    $item = Get-Item -LiteralPath $art
    Write-Host ("{0} ({1:N0} bytes)" -f $item.FullName, $item.Length)
}
