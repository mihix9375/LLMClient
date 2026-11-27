param([switch]$Test, [string]$OutputPath)
$ErrorActionPreference = 'Stop'
$guiRoot = $PSScriptRoot
$frameworkRoot = Split-Path -Parent $guiRoot
if (!$OutputPath) { $OutputPath = Join-Path $frameworkRoot 'Qwen27.exe' }
$OutputPath = [System.IO.Path]::GetFullPath($OutputPath)

# Ensure Cargo, VS 2022 MSVC link.exe/cl.exe, and Windows SDK LIB/INCLUDE/mt.exe are configured
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path -LiteralPath $cargoBin) {
    $env:PATH = "$cargoBin;$env:PATH"
}
$msvcBase = if ($env:QWEN_MSVC_BASE) { $env:QWEN_MSVC_BASE } else { 'C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC' }
$msvcVer = $null
if (Test-Path -LiteralPath $msvcBase) {
    $msvcVer = Get-ChildItem -LiteralPath $msvcBase -Directory | Sort-Object Name -Descending | Select-Object -First 1
}
$kitsRoot = 'C:\Program Files (x86)\Windows Kits\10'
$sdkVer = $null
$kitsLib = Join-Path $kitsRoot 'Lib'
if (Test-Path -LiteralPath $kitsLib) {
    $sdkVer = Get-ChildItem -LiteralPath $kitsLib -Directory | Sort-Object Name -Descending | Select-Object -First 1
}

$extraPath = @()
$extraLib = @()
$extraInc = @()
if ($msvcVer) {
    $extraPath += (Join-Path $msvcVer.FullName 'bin\Hostx64\x64')
    $extraLib += (Join-Path $msvcVer.FullName 'lib\x64')
    $extraInc += (Join-Path $msvcVer.FullName 'include')
}
if ($sdkVer) {
    $v = $sdkVer.Name
    $extraPath += (Join-Path $kitsRoot "bin\$v\x64")
    $extraLib += @(
        (Join-Path $kitsRoot "Lib\$v\um\x64"),
        (Join-Path $kitsRoot "Lib\$v\ucrt\x64")
    )
    $extraInc += @(
        (Join-Path $kitsRoot "Include\$v\ucrt"),
        (Join-Path $kitsRoot "Include\$v\um"),
        (Join-Path $kitsRoot "Include\$v\shared")
    )
}
if ($extraPath.Count) { $env:PATH = (($extraPath -join ';') + ';' + $env:PATH) }
if ($extraLib.Count)  { $env:LIB  = (($extraLib -join ';')  + $(if ($env:LIB) { ';' + $env:LIB } else { '' })) }
if ($extraInc.Count)  { $env:INCLUDE = (($extraInc -join ';') + $(if ($env:INCLUDE) { ';' + $env:INCLUDE } else { '' })) }

$manifest = Join-Path $guiRoot 'Cargo.toml'
# app.manifest embed (absolute path; build scripts of dependencies run in other directories)
$env:CARGO_ENCODED_RUSTFLAGS = (@('-C', 'link-arg=/MANIFEST:EMBED', '-C', ('link-arg=/MANIFESTINPUT:' + (Join-Path $guiRoot 'app.manifest')), ('--remap-path-prefix=' + $env:USERPROFILE + '=~'), ('--remap-path-prefix=' + $guiRoot + '=gui')) -join [char]0x1f)
$targetExe = Join-Path $guiRoot 'target\release\Qwen27.exe'

Push-Location $guiRoot
try {
    & cargo build --release --offline --manifest-path $manifest
    if ($LASTEXITCODE -ne 0) { throw 'Rust release build failed' }
    if (!(Test-Path -LiteralPath $targetExe)) { throw "Built binary missing: $targetExe" }

    if (Test-Path -LiteralPath $OutputPath) {
        $oldBinary = "$OutputPath.old"
        if (Test-Path -LiteralPath $oldBinary) {
            Remove-Item -LiteralPath $oldBinary -Force -ErrorAction SilentlyContinue
        }
        Move-Item -LiteralPath $OutputPath -Destination $oldBinary -Force -ErrorAction SilentlyContinue
    }
    Copy-Item -LiteralPath $targetExe -Destination $OutputPath -Force

    $selfOut = (& $OutputPath '--self-test' | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $selfOut -notmatch 'Desktop key restrictions passed \(no UI input sent\)') {
        throw "Desktop helper checks failed: $selfOut"
    }
    Write-Host $selfOut

    if ($Test) {
        Write-Host 'Running Rust unit tests...'
        & cargo test --offline --manifest-path $manifest
        if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }

        Write-Host 'Checking forbidden characters across gui/src...'
        $forbidden = [char]0x9BD6
        $hits = Get-ChildItem -Path (Join-Path $guiRoot 'src') -Recurse -File |
            Select-String -SimpleMatch ([string]$forbidden)
        if ($hits) {
            $hits | ForEach-Object { Write-Host "$($_.Path):$($_.LineNumber): $($_.Line)" }
            throw 'Forbidden character found inside gui/src'
        }

        Write-Host 'Checking zero wry/webview2 dependencies across gui...'
        $wvTargets = @(
            (Join-Path $guiRoot 'Cargo.toml'),
            (Join-Path $guiRoot 'Cargo.lock')
        ) + (Get-ChildItem -Path (Join-Path $guiRoot 'src') -Recurse -Filter '*.rs' | ForEach-Object { $_.FullName })
        $wvHits = $wvTargets | Where-Object { Test-Path -LiteralPath $_ } |
            Select-String -Pattern '\b(wry|webview2)\b' -CaseSensitive:$false
        if ($wvHits) {
            $wvHits | ForEach-Object { Write-Host "$($_.Path):$($_.LineNumber): $($_.Line)" }
            throw 'Unexpected wry/webview2 reference found inside gui/'
        }

        Write-Host 'Checking zero SSH code across gui/src...'
        if (Test-Path -LiteralPath (Join-Path $guiRoot 'src\ssh.rs')) {
            throw 'Forbidden gui/src/ssh.rs still exists'
        }
        $sshHits = Get-ChildItem -Path (Join-Path $guiRoot 'src') -Recurse -Filter '*.rs' |
            Select-String -Pattern '\b(SshClient|ensure_tunnel|sshTarget|autoTunnel|ssh\.exe)\b' -CaseSensitive:$false
        if ($sshHits) {
            $sshHits | ForEach-Object { Write-Host "$($_.Path):$($_.LineNumber): $($_.Line)" }
            throw 'Unexpected SSH reference found inside gui/src'
        }

        Write-Host 'Running CLI / Diag smoke checks...'
        foreach ($argList in @(@('--api', 'help'), @('--api', 'gui-settings'), @('--api', 'windows'), @('--diag-selftest'))) {
            $out = (& $OutputPath @argList | Out-String).Trim()
            if ($LASTEXITCODE -ne 0) { throw "Command failed: Qwen27.exe $($argList -join ' ')" }
            if ($argList[0] -eq '--api' -and [string]::IsNullOrWhiteSpace($out)) {
                throw "Empty stdout from Qwen27.exe $($argList -join ' ')"
            }
        }

        Write-Host 'Running desktop worker stdin/stdout local workspace roundtrip...'
        function Invoke-WorkerStdio([string]$ExePath, [string]$RootPath, [string[]]$Requests) {
            $utf8 = [System.Text.UTF8Encoding]::new($false)
            $psi = New-Object System.Diagnostics.ProcessStartInfo
            $psi.FileName = $ExePath
            $psi.Arguments = "--target 0 --root `"$RootPath`""
            $psi.UseShellExecute = $false
            $psi.CreateNoWindow = $true
            $psi.RedirectStandardInput = $true
            $psi.RedirectStandardOutput = $true
            $psi.RedirectStandardError = $true
            $psi.StandardOutputEncoding = $utf8
            $psi.StandardErrorEncoding = $utf8
            $payload = ($Requests -join "`n") + "`n"
            $bytes = $utf8.GetBytes($payload)
            $proc = [System.Diagnostics.Process]::Start($psi)
            try {
                $proc.StandardInput.BaseStream.Write($bytes, 0, $bytes.Length)
                $proc.StandardInput.BaseStream.Flush()
                $proc.StandardInput.BaseStream.Close()
                $stdout = $proc.StandardOutput.ReadToEnd()
                $stderr = $proc.StandardError.ReadToEnd()
                $proc.WaitForExit()
                if ($proc.ExitCode -ne 0) {
                    throw "Worker exited with $($proc.ExitCode): $stderr"
                }
                return ($stdout -split "`r?`n" | Where-Object { $_.Trim().Length -gt 0 })
            } finally {
                $proc.Dispose()
            }
        }

        $wsTmp = Join-Path ([System.IO.Path]::GetTempPath()) ("qwen27_worker_smoke_" + [System.Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Force -Path $wsTmp | Out-Null
        try {
            $req1 = '{"id":"1","tool":"local.list","args":{}}'
            $req2 = '{"id":"2","tool":"local.create","args":{"path":"smoke.txt","text":"line1\nline2\n"}}'
            $req3 = '{"id":"3","tool":"local.read","args":{"path":"smoke.txt"}}'
            $resp123Raw = Invoke-WorkerStdio -ExePath $OutputPath -RootPath $wsTmp -Requests @($req1, $req2, $req3)
            $verLine = ($resp123Raw | Where-Object { $_ -match '"version":"([0-9a-f]{64})"' } | Select-Object -First 1)
            if (-not $verLine -or $verLine -notmatch '"version":"([0-9a-f]{64})"') { throw "Missing version in local.read: $($resp123Raw | Out-String)" }
            $ver = $Matches[1]
            $req4 = "{`"id`":`"4`",`"tool`":`"local.edit`",`"args`":{`"path`":`"smoke.txt`",`"expectedVersion`":`"$ver`",`"operation`":`"replace`",`"oldText`":`"line2`",`"newText`":`"LINE2_EDITED`"}}"
            $req5 = '{"id":"5","tool":"local.run","args":{"command":"Write-Output ''pwsh-ok''","timeoutSeconds":10}}'
            $resp45Raw = Invoke-WorkerStdio -ExePath $OutputPath -RootPath $wsTmp -Requests @($req4, $req5)
            $respAll = (@($resp123Raw) + @($resp45Raw) | Out-String)
            if ($respAll -match 'expected value at line' -or $respAll -notmatch '"networkPolicy"' -or $respAll -notmatch '"localProtocol":3' -or $respAll -notmatch 'LINE2_EDITED' -or $respAll -notmatch 'pwsh-ok') {
                throw "Unexpected worker response: $respAll"
            }
        } finally {
            Remove-Item -LiteralPath $wsTmp -Recurse -Force -ErrorAction SilentlyContinue
        }
        Write-Host 'All Rust GUI checks passed.'
    }
} finally {
    Pop-Location
}
Write-Host "Built: $OutputPath"
