param(
    [ValidateSet("backend", "frontend", "custom")]
    [string]$Preset = "backend",
    [string]$Command = "",
    [string]$HealthUrl = "",
    [string]$SuccessPattern = "",
    [int]$StartupTimeoutSeconds = 25,
    [int]$ExcerptLines = 80
)

$ErrorActionPreference = "Stop"

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$sessionRoot = Join-Path $projectRoot "tmp\codex-startup"
$sessionDir = Join-Path $sessionRoot $timestamp
$stdoutLog = Join-Path $sessionDir "stdout.log"
$stderrLog = Join-Path $sessionDir "stderr.log"
$promptFile = Join-Path $sessionDir "codex-prompt.md"
$latestPromptFile = Join-Path $sessionRoot "latest-codex-prompt.md"
$latestSessionFile = Join-Path $sessionRoot "latest-session.txt"

function Write-Step {
    param([string]$Message)
    Write-Host "[codex-dev] $Message"
}

function Resolve-StartupPreset {
    param(
        [string]$PresetName,
        [string]$OverrideCommand,
        [string]$OverrideHealthUrl,
        [string]$OverrideSuccessPattern
    )

    $resolved = switch ($PresetName) {
        "backend" {
            [pscustomobject]@{
                Name = "backend"
                Command = "npm run backend:dev:uv"
                HealthUrl = "http://127.0.0.1:8000/api/health"
                SuccessPattern = "Application startup complete|Uvicorn running on"
            }
        }
        "frontend" {
            [pscustomobject]@{
                Name = "frontend"
                Command = "npm run dev"
                HealthUrl = "http://127.0.0.1:5173"
                SuccessPattern = "Local:"
            }
        }
        "custom" {
            if (-not $OverrideCommand) {
                throw "Preset=custom 时必须传入 -Command。"
            }

            [pscustomobject]@{
                Name = "custom"
                Command = $OverrideCommand
                HealthUrl = $OverrideHealthUrl
                SuccessPattern = $OverrideSuccessPattern
            }
        }
        default {
            throw "不支持的 preset: $PresetName"
        }
    }

    if ($OverrideCommand) {
        $resolved.Command = $OverrideCommand
    }

    if ($OverrideHealthUrl) {
        $resolved.HealthUrl = $OverrideHealthUrl
    }

    if ($OverrideSuccessPattern) {
        $resolved.SuccessPattern = $OverrideSuccessPattern
    }

    return $resolved
}

function Get-LogTailText {
    param(
        [string]$Path,
        [int]$TailLines
    )

    if (-not (Test-Path $Path)) {
        return "(日志文件尚未生成)"
    }

    $lines = Get-Content -Path $Path -Tail $TailLines -ErrorAction SilentlyContinue
    if (-not $lines) {
        return "(没有输出)"
    }

    return ($lines -join "`n")
}

function Test-HealthEndpoint {
    param([string]$Url)

    if (-not $Url) {
        return $false
    }

    try {
        $response = Invoke-WebRequest -Uri $Url -Method Get -TimeoutSec 3
        return $response.StatusCode -ge 200 -and $response.StatusCode -lt 400
    }
    catch {
        return $false
    }
}

function Test-SuccessPatternInLogs {
    param(
        [string]$Pattern,
        [string[]]$Paths
    )

    if (-not $Pattern) {
        return $false
    }

    foreach ($path in $Paths) {
        if (-not (Test-Path $path)) {
            continue
        }

        $content = Get-Content -Path $path -Tail 120 -ErrorAction SilentlyContinue
        if ($content -and (($content -join "`n") -match $Pattern)) {
            return $true
        }
    }

    return $false
}

function New-CodexPrompt {
    param(
        [pscustomobject]$Config,
        [string]$FailureReason
    )

    $stdoutTail = Get-LogTailText -Path $stdoutLog -TailLines $ExcerptLines
    $stderrTail = Get-LogTailText -Path $stderrLog -TailLines $ExcerptLines

    return @"
# Codex 启动报错修复请求

请在仓库 `$projectRoot` 中直接定位并修复启动失败问题。

## 启动信息
- 预设: $($Config.Name)
- 命令: `$($Config.Command)`
- 失败原因: $FailureReason
- 发生时间: $(Get-Date -Format "yyyy-MM-dd HH:mm:ss")
- stdout: `$stdoutLog`
- stderr: `$stderrLog`

## 错误摘录
```text
[stdout]
$stdoutTail

[stderr]
$stderrTail
```

请按下面方式处理：
1. 先分析根因，再直接修改代码。
2. 修复后给出验证命令，优先使用当前仓库已有命令。
3. 如果问题属于环境、依赖或配置缺失，也请明确指出。
"@
}

New-Item -ItemType Directory -Path $sessionDir -Force | Out-Null

$config = Resolve-StartupPreset `
    -PresetName $Preset `
    -OverrideCommand $Command `
    -OverrideHealthUrl $HealthUrl `
    -OverrideSuccessPattern $SuccessPattern

$escapedProjectRoot = $projectRoot.Replace("'", "''")
$launcherScript = "Set-Location '$escapedProjectRoot'; $($config.Command)"

Write-Step "启动预设: $($config.Name)"
Write-Step "启动命令: $($config.Command)"
Write-Step "stdout 日志: $stdoutLog"
Write-Step "stderr 日志: $stderrLog"

$process = Start-Process `
    -FilePath "powershell.exe" `
    -ArgumentList @(
        "-NoLogo",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        $launcherScript
    ) `
    -WorkingDirectory $projectRoot `
    -RedirectStandardOutput $stdoutLog `
    -RedirectStandardError $stderrLog `
    -PassThru

$startupSucceeded = $false
$failureReason = ""
$deadline = (Get-Date).AddSeconds($StartupTimeoutSeconds)

while ((Get-Date) -lt $deadline) {
    $process.Refresh()

    if ($process.HasExited) {
        $failureReason = "进程在启动阶段提前退出，退出码 $($process.ExitCode)。"
        break
    }

    if (Test-HealthEndpoint -Url $config.HealthUrl) {
        $startupSucceeded = $true
        break
    }

    if (Test-SuccessPatternInLogs -Pattern $config.SuccessPattern -Paths @($stdoutLog, $stderrLog)) {
        $startupSucceeded = $true
        break
    }

    Start-Sleep -Seconds 1
}

if (-not $startupSucceeded -and -not $failureReason) {
    $process.Refresh()

    if (-not $config.HealthUrl -and -not $config.SuccessPattern -and -not $process.HasExited) {
        $startupSucceeded = $true
    }
    elseif ($process.HasExited) {
        $failureReason = "进程在健康检查完成前退出，退出码 $($process.ExitCode)。"
    }
    else {
        $failureReason = "在 $StartupTimeoutSeconds 秒内未通过健康检查或启动成功标记。"
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        $process.WaitForExit()
    }
}

if ($startupSucceeded) {
    Set-Content -Path $latestSessionFile -Value @(
        "preset=$($config.Name)"
        "pid=$($process.Id)"
        "command=$($config.Command)"
        "stdout=$stdoutLog"
        "stderr=$stderrLog"
        "started_at=$(Get-Date -Format "yyyy-MM-dd HH:mm:ss")"
    ) -Encoding UTF8

    Write-Step "服务启动成功，进程 PID: $($process.Id)"
    if ($config.HealthUrl) {
        Write-Step "健康检查通过: $($config.HealthUrl)"
    }
    elseif ($config.SuccessPattern) {
        Write-Step "命中启动成功标记: $($config.SuccessPattern)"
    }
    else {
        Write-Step "未配置健康检查，已按进程仍在运行视为成功。"
    }

    Write-Step "如需查看最近一次会话信息，可打开: $latestSessionFile"
    exit 0
}

$promptContent = New-CodexPrompt -Config $config -FailureReason $failureReason
Set-Content -Path $promptFile -Value $promptContent -Encoding UTF8
Set-Content -Path $latestPromptFile -Value $promptContent -Encoding UTF8

$clipboardCopied = $false
try {
    Set-Clipboard -Value $promptContent
    $clipboardCopied = $true
}
catch {
    $clipboardCopied = $false
}

Write-Step "启动失败: $failureReason"
Write-Step "Codex 提示文件: $promptFile"
Write-Step "最近一次提示文件: $latestPromptFile"
if ($clipboardCopied) {
    Write-Step "修复提示已复制到剪贴板，可直接粘贴给 Codex。"
}
else {
    Write-Step "剪贴板写入失败，请手动打开提示文件并复制内容。"
}

exit 1
