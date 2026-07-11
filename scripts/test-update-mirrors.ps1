param(
    [string]$Version = "1.1.7",
    [ValidateRange(1, 16)]
    [int]$SampleMiB = 2,
    [ValidateRange(5, 120)]
    [int]$TimeoutSeconds = 30
)

$ErrorActionPreference = "Stop"

$fileName = "OOPZ+_${Version}_x64_en-US.msi"
$githubUrl = "https://github.com/M4rkzzz/oopz-plus/releases/download/v${Version}/${fileName}"
$sampleBytes = $SampleMiB * 1MB
$rangeEnd = $sampleBytes - 1

$mirrors = [ordered]@{
    "GitHub"     = $githubUrl
    "ghfast.top" = "https://ghfast.top/$githubUrl"
    "gh-proxy"   = "https://gh-proxy.com/$githubUrl"
    "ghproxy.net" = "https://ghproxy.net/$githubUrl"
    "moeyy"      = "https://github.moeyy.xyz/$githubUrl"
}

if (-not (Get-Command curl.exe -ErrorAction SilentlyContinue)) {
    throw "未找到 curl.exe，Windows 10/11 通常已内置该命令。"
}

Write-Host "测试文件: $fileName"
Write-Host "每条线路最多等待 ${TimeoutSeconds}s，目标采样大小 ${SampleMiB} MiB。"
Write-Host ""

$results = foreach ($entry in $mirrors.GetEnumerator()) {
    Write-Host ("正在测试 {0}..." -f $entry.Key)
    $format = "%{http_code}|%{time_connect}|%{time_starttransfer}|%{time_total}|%{size_download}|%{speed_download}"
    $output = & curl.exe `
        --location `
        --silent `
        --show-error `
        --max-time $TimeoutSeconds `
        --range "0-$rangeEnd" `
        --user-agent "OOPZ-Plus-Mirror-Test" `
        --output NUL `
        --write-out $format `
        $entry.Value 2>&1
    $exitCode = $LASTEXITCODE
    $metrics = ($output | Select-Object -Last 1) -split "\|"

    if ($exitCode -eq 0 -and $metrics.Count -eq 6 -and $metrics[0] -in @("200", "206")) {
        $downloaded = [double]$metrics[4]
        $speed = [double]$metrics[5]
        [pscustomobject]@{
            Line = $entry.Key
            Status = "可用"
            Http = [int]$metrics[0]
            ConnectMs = [math]::Round(([double]$metrics[1]) * 1000)
            FirstByteMs = [math]::Round(([double]$metrics[2]) * 1000)
            Seconds = [math]::Round([double]$metrics[3], 2)
            DownloadedMiB = [math]::Round($downloaded / 1MB, 2)
            SpeedMiBps = [math]::Round($speed / 1MB, 2)
            Error = $null
            Url = $entry.Value
        }
    } else {
        $detail = ($output -join " ").Trim()
        if ($detail.Length -gt 100) {
            $detail = $detail.Substring(0, 100) + "..."
        }
        [pscustomobject]@{
            Line = $entry.Key
            Status = "失败"
            Http = if ($metrics.Count -gt 0 -and $metrics[0] -match "^\d+$") { [int]$metrics[0] } else { 0 }
            ConnectMs = $null
            FirstByteMs = $null
            Seconds = $null
            DownloadedMiB = $null
            SpeedMiBps = $null
            Error = if ($detail) { $detail } else { "curl exit $exitCode" }
            Url = $entry.Value
        }
    }
}

Write-Host ""
$results |
    Sort-Object @{ Expression = { $_.Status -ne "可用" } }, @{ Expression = "SpeedMiBps"; Descending = $true } |
    Format-Table Line, Status, Http, ConnectMs, FirstByteMs, Seconds, DownloadedMiB, SpeedMiBps -AutoSize

$failures = $results | Where-Object { $_.Status -eq "失败" }
if ($failures) {
    Write-Host "失败详情:"
    $failures | ForEach-Object { Write-Host ("- {0}: {1}" -f $_.Line, $_.Error) }
    Write-Host ""
}

$winner = $results |
    Where-Object { $_.Status -eq "可用" } |
    Sort-Object SpeedMiBps -Descending |
    Select-Object -First 1

if ($winner) {
    Write-Host ("当前最快: {0} ({1} MiB/s)" -f $winner.Line, $winner.SpeedMiBps) -ForegroundColor Green
} else {
    Write-Warning "本次没有可用线路。"
}
