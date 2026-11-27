$ErrorActionPreference = 'Stop'
$script:Utf8 = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = $script:Utf8
$script:ApiHelper = Join-Path $PSScriptRoot 'Qwen27.exe'
if (-not (Test-Path -LiteralPath $script:ApiHelper)) { throw 'Qwen27.exe is missing. Build the native client first.' }

function Show-QwenJobSummary($job, [string]$savedDir)
{
    if ($job.result)
    {
        Write-Host $job.result.review.final_answer
        Write-Host "Model approved: $($job.result.review.approved); achieved: $($job.result.achieved); tests: $($job.result.testsExecuted)"
    }
    elseif ($job.error) { Write-Host $job.error -ForegroundColor Red }
    if ($savedDir) { Write-Host "Saved: $savedDir" }
}

function Read-InteractiveAnswer($q)
{
    Write-Host "`n[質問] $($q.text)" -ForegroundColor Cyan
    $opts = @($q.options)
    for ($i = 0; $i -lt $opts.Count; $i++) { Write-Host ("  {0}) {1}" -f ($i + 1), $opts[$i]) }
    while ($true)
    {
        $ans = Read-Host '回答 (番号または自由記述)'
        if (-not $ans) { continue }
        $num = 0
        if ([int]::TryParse($ans.Trim(), [ref]$num) -and $num -ge 1 -and $num -le $opts.Count) { return [string]$opts[$num - 1] }
        return $ans.Trim()
    }
}

function Format-WinArgs([string[]]$items)
{
    $quoted = foreach ($item in $items)
    {
        if ($null -eq $item -or $item.Length -eq 0) { '""' }
        elseif ($item -notmatch '[\s"]') { $item }
        else { '"' + ($item -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"' }
    }
    return ($quoted -join ' ')
}

function Invoke-QwenApi([string[]]$cliArgs, [switch]$ReturnValue, [switch]$ReturnJob, [switch]$Quiet)
{
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $script:ApiHelper
    $info.Arguments = Format-WinArgs (@('--api') + $cliArgs)
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.StandardOutputEncoding = $script:Utf8
    $info.StandardErrorEncoding = $script:Utf8

    $proc = New-Object System.Diagnostics.Process
    $proc.StartInfo = $info
    $lastPartial = ''
    $rawJson = $cliArgs -contains '--json'
    $showProgress = (-not $Quiet) -and (-not $rawJson)
    $resultValue = $null
    $resultJob = $null
    $questionInfo = $null
    $started = $false
    try
    {
        [void]$proc.Start()
        $started = $true
        $stderrTask = $proc.StandardError.ReadToEndAsync()
        while ($null -ne ($line = $proc.StandardOutput.ReadLine()))
        {
            if (-not $line.Trim()) { continue }
            $msg = $null
            try { $msg = $line | ConvertFrom-Json } catch { if (-not $Quiet) { Write-Host $line }; continue }
            if ($msg.type -eq 'progress' -and $showProgress) { Write-Host "[$($msg.id)] $($msg.stage) ($($msg.status))" }
            elseif ($msg.type -eq 'activity' -and $showProgress -and $msg.activity -and $msg.activity.Count -gt 0)
            {
                $last = $msg.activity[$msg.activity.Count - 1]
                Write-Host "  [step $($last.step)] $($last.tool): $($last.reason)" -ForegroundColor DarkGray
            }
            elseif ($msg.type -eq 'streaming' -and $showProgress -and $msg.partialAnswer)
            {
                $cur = [string]$msg.partialAnswer
                if ($cur.StartsWith($lastPartial) -and $cur.Length -gt $lastPartial.Length) { Write-Host -NoNewline $cur.Substring($lastPartial.Length) }
                elseif ($cur -ne $lastPartial) { Write-Host $cur }
                $lastPartial = $cur
            }
            elseif ($msg.type -eq 'question') { $questionInfo = $msg }
            elseif ($msg.type -eq 'error') { throw $msg.message }
            elseif ($msg.type -eq 'result')
            {
                if ($lastPartial) { Write-Host '' }
                $resultValue = $msg.value
                $resultJob = $msg.job
                if (-not $Quiet)
                {
                    if ($resultJob -and $rawJson) { Write-Host (ConvertTo-Json -InputObject $resultJob -Depth 100) }
                    elseif ($resultJob) { Show-QwenJobSummary $resultJob ([string]$msg.savedDir) }
                    elseif ($null -ne $resultValue) { Write-Host (ConvertTo-Json -InputObject $resultValue -Depth 100) }
                }
            }
        }
        $proc.WaitForExit()
        $err = $stderrTask.GetAwaiter().GetResult()
        if ($questionInfo)
        {
            $ans = Read-InteractiveAnswer $questionInfo.question
            [void](Invoke-QwenApi @('answer', $questionInfo.id, '--question-id', $questionInfo.question.id, '--answer', $ans) -Quiet)
            while ($true)
            {
                Start-Sleep -Milliseconds 700
                $st = Invoke-QwenApi @('status', $questionInfo.id, '--json') -ReturnValue -Quiet
                if ($st.status -eq 'waiting_user')
                {
                    $ans = Read-InteractiveAnswer $st.question
                    [void](Invoke-QwenApi @('answer', $st.id, '--question-id', $st.question.id, '--answer', $ans) -Quiet)
                }
                elseif ($st.status -notin @('running', 'queued'))
                {
                    return Invoke-QwenApi @('fetch', $st.id) -ReturnJob:$ReturnJob -ReturnValue:$ReturnValue -Quiet:$Quiet
                }
            }
        }
        if ($proc.ExitCode -notin @(0, 2)) { throw "Request failed (exit $($proc.ExitCode)). $err" }
        $script:LastExit = $proc.ExitCode
        if ($ReturnJob) { return $resultJob }
        if ($ReturnValue) { return $resultValue }
        return $proc.ExitCode
    }
    finally
    {
        if ($started -and -not $proc.HasExited) { $proc.Kill() }
        $proc.Dispose()
    }
}

function Convert-QwenCliArgs([string[]]$raw)
{
    $translated = [System.Collections.Generic.List[string]]::new()
    $cmd = $null
    $known = @('chat','help','health','connect','identity','commands','windows','gui-settings','settings','configure','preset','reset-sampling','perf','models','autotune','tune-status','tune-apply','standby','run','status','fetch','save','cancel','answer','interrupt','conversations','conversation','compact','memory','schedules','share','files','upload','classify','profiles','adapters','adapter-profiles')
    $valFlags = @('--url','--api-url','--ssh-target','--work-mode','--mode','--role','--model-role','--context','--tokens','--max-tokens','--iterations','--max-iterations','--effort','--reasoning-effort','--budget','--reasoning-budget','--preset','--temp','--temperature','--top-p','--top-k','--min-p','--repeat-penalty','--presence-penalty','--frequency-penalty','--workdir','--read-dir','--remove-read-dir','--window','--attach','--conv','--parent','--file','--output','--path','--ip','--task','--tune-mode','--answer','--question-id','--filter','--query')
    for ($i = 0; $i -lt $raw.Count; $i++)
    {
        $a = $raw[$i]
        if ($a -ieq '-Command' -and $i + 1 -lt $raw.Count) { $cmd = $raw[++$i] }
        elseif ($a -ieq '-GoalFile' -and $i + 1 -lt $raw.Count) { $translated.Add('--file'); $translated.Add($raw[++$i]) }
        elseif ($a -ieq '-OutputDirectory' -and $i + 1 -lt $raw.Count) { $translated.Add('--output'); $translated.Add($raw[++$i]) }
        elseif ($a -ieq '-Text') { while ($i + 1 -lt $raw.Count -and -not $raw[$i + 1].StartsWith('-')) { $translated.Add($raw[++$i]) } }
        elseif ($a -in @('--help', '-h', '-?')) { $cmd = 'help' }
        elseif ($valFlags -contains $a -and $i + 1 -lt $raw.Count) { $translated.Add($a); $translated.Add($raw[++$i]) }
        elseif ($null -eq $cmd -and -not $a.StartsWith('-')) { $cmd = $a }
        else { $translated.Add($a) }
    }
    if (-not $cmd -and $translated.Contains('--file')) { $cmd = 'run' }
    if (-not $cmd) { $cmd = 'chat' }
    if ($cmd -notin $known) { $translated.Insert(0, $cmd); $cmd = 'run' }
    return @{ Command = $cmd; Args = $translated.ToArray() }
}

try
{
    $parsed = Convert-QwenCliArgs $args
    $cmd = $parsed.Command
    $rest = $parsed.Args
    if ($cmd -eq 'chat')
    {
        $chatScript = Join-Path $PSScriptRoot 'scripts\qwen-chat.ps1'
        if (Test-Path -LiteralPath $chatScript)
        {
            . $chatScript
            exit (Start-QwenChat $rest)
        }
        & $script:ApiHelper (@('--cli') + $rest)
        exit $LASTEXITCODE
    }
    if ($cmd -eq 'standby')
    {
        & $script:ApiHelper (@('--api', 'standby') + $rest)
        exit $LASTEXITCODE
    }
    $script:LastExit = 0
    [void](Invoke-QwenApi (@($cmd) + $rest))
    exit $script:LastExit
}
catch { Write-Error $_; exit 1 }
