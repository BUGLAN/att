<#
.SYNOPSIS
    实时 ASR 一键启动 / 停止 / 状态脚本（Docker 双容器）

.DESCRIPTION
    对应需求文档 docs/需求文档-Docker化实时ASR-2026-10-01.md 的 FR-2 / FR-3 / FR-4 / FR-5。

    启动流程（分段输出进度，任一步失败即非零退出并给出下一步建议）：
      [1/8] 检查配置文件与卷目录
      [2/8] 检查流式相关设置
      [3/8] 检查 Docker 守护进程（必要时自动拉起 Docker Desktop）
      [4/8] 检查端口占用
      [5/8] 准备镜像
      [6/8] 启动容器
      [7/8] 校验容器内 GPU 可用
      [8/8] 预热模型并等待就绪

.NOTES
    数据落在仓库内目录（决策 D8）：
      data\   业务数据与加密密钥（必须持久化，丢失加密密钥将无法解密已保存的 API Key）
      models\ 模型缓存（直接复用宿主已下载模型）
      logs\   应用日志与桥接服务日志

    重要警示：data\ 与 models\ 处于 .gitignore 忽略状态，执行 git clean -xfd 会删除全部
    业务数据与模型缓存，排查问题时禁止使用该命令。

.EXAMPLE
    pwsh -File scripts\docker\att.ps1 -Action start
.EXAMPLE
    pwsh -File scripts\docker\att.ps1 -Action start -WithApp
#>
[CmdletBinding()]
param(
    [ValidateSet('start', 'stop', 'status', 'restart', 'logs')]
    [string]$Action = 'start',

    # 强制重新构建镜像
    [switch]$Rebuild,

    # 跳过镜像构建（镜像已存在时加快启动）
    [switch]$NoBuild,

    # 启动后同时拉起 Electron 桌面端
    [switch]$WithApp,

    # 只提示不修改 data\settings.json
    [switch]$SkipSettingsFix,

    # 等待模型就绪的超时时间（秒）
    [int]$ReadyTimeoutSeconds = 600
)

$ErrorActionPreference = 'Stop'

# 让中文输出在控制台正常显示
try {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
} catch {
    # 某些宿主环境下不允许修改编码，忽略即可
}

# ------------------------------------------------------------------ 基础路径
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$ComposeFile = Join-Path $RepoRoot 'docker-compose.yml'
$EnvFile = Join-Path $RepoRoot '.env'
$EnvExampleFile = Join-Path $RepoRoot '.env.example'
$DataDir = Join-Path $RepoRoot 'data'
$ModelsDir = Join-Path $RepoRoot 'models'
$LogsDir = Join-Path $RepoRoot 'logs'
$BackendPort = 8000

# ------------------------------------------------------------------ 输出工具
function Write-Head([string]$Text) {
    Write-Host ''
    Write-Host "==== $Text ====" -ForegroundColor Cyan
}

function Write-Step([int]$Index, [int]$Total, [string]$Text) {
    Write-Host ''
    Write-Host "[$Index/$Total] $Text" -ForegroundColor Cyan
}

function Write-Ok([string]$Text) {
    Write-Host "  [OK] $Text" -ForegroundColor Green
}

function Write-Warn([string]$Text) {
    Write-Host "  [警告] $Text" -ForegroundColor Yellow
}

function Write-Err([string]$Text) {
    Write-Host "  [失败] $Text" -ForegroundColor Red
}

function Write-Hint([string]$Text) {
    Write-Host "  [建议] $Text" -ForegroundColor Yellow
}

# ------------------------------------------------------------------ 命令封装
# 说明：compose 参数一律以数组形式传入，避免 "-d" / "-fsS" 这类参数被
# PowerShell 当作函数自身的参数名解析。
function Invoke-Compose {
    param([string[]]$ComposeArgs)

    & docker compose -f $ComposeFile --project-directory $RepoRoot @ComposeArgs
    $code = $LASTEXITCODE

    if ($code -ne 0) {
        throw "docker compose $($ComposeArgs -join ' ') 执行失败（退出码 $code）"
    }

    return $code
}

# 在 asr 容器内执行 shell 命令，统一返回退出码与输出
function Invoke-InAsrShell {
    param([string]$Command)

    $output = & docker compose -f $ComposeFile --project-directory $RepoRoot exec -T asr sh -c $Command 2>&1

    return @{
        ExitCode = $LASTEXITCODE
        Output   = (($output | Out-String).Trim())
    }
}

# 写入无 BOM 的 UTF-8 文件（PS 5.1 的 Set-Content 会写入 BOM，导致 Python 解析 JSON 失败）
function Write-Utf8NoBom {
    param([string]$Path, [string]$Content)

    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

# 构建镜像：Docker Desktop 的 buildx 偶发崩溃（实测约每数次触发一次，与代码无关），
# 因此失败时自动重试一次，避免「一键启动」因环境抖动而失败
function Invoke-BuildWithRetry([int]$Attempts = 2) {
    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        Write-Host "  执行镜像构建（第 $attempt/$Attempts 次）"
        & docker compose -f $ComposeFile --project-directory $RepoRoot build --progress plain

        if ($LASTEXITCODE -eq 0) {
            Write-Ok '镜像构建完成'
            return
        }

        Write-Warn "镜像构建失败（退出码 $LASTEXITCODE）"
        if ($attempt -lt $Attempts) {
            Write-Host '  buildx 偶发失败，3 秒后自动重试'
            Start-Sleep -Seconds 3
        }
    }

    throw '镜像构建失败（已重试仍不通过），请查看上方构建输出'
}

# ------------------------------------------------------------------ 检查函数
function Test-DockerDaemon {
    & docker info --format '{{.ServerVersion}}' 2>&1 | Out-Null
    return ($LASTEXITCODE -eq 0)
}

function Start-DockerDesktopAndWait([int]$TimeoutSeconds = 240) {
    $candidates = @(
        'C:\Program Files\Docker\Docker\Docker Desktop.exe',
        "$env:LOCALAPPDATA\Docker\Docker Desktop.exe"
    )
    $exe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1

    if (-not $exe) {
        return $false
    }

    Write-Host "  正在启动 Docker Desktop：$exe"
    Start-Process $exe | Out-Null

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 3
        if (Test-DockerDaemon) {
            return $true
        }
        Write-Host '  ... 等待 Docker 守护进程就绪'
    }

    return $false
}

function Test-PortListening([int]$Port) {
    try {
        $connections = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction Stop
        return ($null -ne $connections)
    } catch {
        return $false
    }
}

function Test-ImageExists([string]$ImageName) {
    & docker image inspect $ImageName 2>&1 | Out-Null
    return ($LASTEXITCODE -eq 0)
}

function Get-ContainerHealth([string]$ContainerName) {
    $value = & docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' $ContainerName 2>$null
    if ($LASTEXITCODE -ne 0) {
        return 'absent'
    }
    return (($value | Out-String).Trim())
}

function Get-AsrHealth {
    $result = Invoke-InAsrShell 'curl -fsS --max-time 10 http://127.0.0.1:9000/api/health'
    if ($result.ExitCode -ne 0 -or -not $result.Output) {
        return $null
    }

    try {
        return ($result.Output | ConvertFrom-Json)
    } catch {
        return $null
    }
}

function Wait-ContainerHealthy([string]$ContainerName, [int]$TimeoutSeconds) {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $status = Get-ContainerHealth $ContainerName
        if ($status -eq 'healthy') {
            return $true
        }
        if ($status -eq 'absent') {
            return $false
        }
        Start-Sleep -Seconds 2
    }
    return $false
}

# ------------------------------------------------------------------ 配置与设置
function Initialize-EnvFile {
    if (Test-Path $EnvFile) {
        Write-Ok ".env 已存在：$EnvFile"
        return
    }

    if (Test-Path $EnvExampleFile) {
        Copy-Item $EnvExampleFile $EnvFile -Force
        Write-Warn '.env 不存在，已从 .env.example 复制一份默认配置'
        Write-Hint "如需调整模型、显存占用比例或分块参数，请编辑 $EnvFile 后重新执行"
        return
    }

    throw '既没有 .env 也没有 .env.example，无法确定运行配置'
}

function Initialize-VolumeDirectories {
    foreach ($dir in @($DataDir, $ModelsDir, $LogsDir)) {
        if (-not (Test-Path $dir)) {
            New-Item -ItemType Directory -Path $dir -Force | Out-Null
            Write-Ok "已创建卷目录：$dir"
        }
    }
    Write-Ok '卷目录就绪：data / models / logs（均在仓库内，对应决策 D8）'
}

function Test-ModelCache {
    $qwenDir = Join-Path $ModelsDir 'qwen'
    if (-not (Test-Path $qwenDir)) {
        Write-Warn "未找到模型目录：$qwenDir"
        Write-Hint '请先下载 Qwen3-ASR 模型，或确认 models\qwen 路径正确'
        return $false
    }

    $modelConfig = Get-ChildItem -Path $qwenDir -Recurse -Filter 'config.json' -ErrorAction SilentlyContinue |
        Where-Object { $_.DirectoryName -like '*Qwen*ASR*' } |
        Select-Object -First 1

    # 优先检查桥接服务实际解析的目录：<models>\qwen\<模型短名>\config.json
    $directModelDir = Join-Path $qwenDir 'Qwen3-ASR-1.7B'
    if (Test-Path (Join-Path $directModelDir 'config.json')) {
        Write-Ok "模型缓存可用：$directModelDir"
        return $true
    }

    if (-not $modelConfig) {
        Write-Warn 'models\qwen 下未找到可用的模型目录（需包含 config.json）'
        return $false
    }

    Write-Ok "模型缓存可用：$($modelConfig.DirectoryName)"
    return $true
}

function Confirm-StreamingSettings {
    $settingsPath = Join-Path $DataDir 'settings.json'

    if (-not (Test-Path $settingsPath)) {
        Write-Warn '未找到 data\settings.json，后端将写入默认设置（默认录音模式为 single，不是流式）'
        Write-Hint '建议启动应用后把录音模式切换为「真流式」，再重新执行本脚本'
        return
    }

    $raw = Get-Content -Raw -Encoding UTF8 $settingsPath
    $settings = $null
    try {
        $settings = $raw | ConvertFrom-Json
    } catch {
        Write-Warn 'data\settings.json 不是合法 JSON，已跳过设置检查'
        return
    }

    $changed = @()
    if ($settings.asr_provider -ne 'qwen_asr') {
        $settings.asr_provider = 'qwen_asr'
        $changed += 'asr_provider'
    }
    if ($settings.recording_mode -ne 'streaming') {
        $settings.recording_mode = 'streaming'
        $changed += 'recording_mode'
    }

    if ($changed.Count -eq 0) {
        Write-Ok '流式设置正确：asr_provider=qwen_asr，recording_mode=streaming'
        return
    }

    if ($SkipSettingsFix) {
        Write-Warn "需要修正的设置项：$($changed -join ', ')（已按 -SkipSettingsFix 跳过修改）"
        Write-Hint '真流式要求 asr_provider=qwen_asr 且 recording_mode=streaming'
        return
    }

    $backupPath = "$settingsPath.bak-$(Get-Date -Format 'yyyyMMdd-HHmmss')"
    Copy-Item $settingsPath $backupPath -Force
    Write-Utf8NoBom -Path $settingsPath -Content ($settings | ConvertTo-Json -Depth 20)

    Write-Warn "已修正设置项：$($changed -join ', ')"
    Write-Hint "原文件已备份为：$backupPath"
}

# ------------------------------------------------------------------ 启动流程
function Invoke-Start {
    $totalSteps = 8

    Write-Head '实时 ASR 一键启动（Docker 双容器）'

    # [1/8] 配置与卷目录
    Write-Step 1 $totalSteps '检查配置文件与卷目录'
    Initialize-EnvFile
    Initialize-VolumeDirectories
    [void](Test-ModelCache)

    # [2/8] 流式设置
    Write-Step 2 $totalSteps '检查流式相关设置'
    Confirm-StreamingSettings

    # [3/8] Docker 守护进程
    Write-Step 3 $totalSteps '检查 Docker 守护进程'
    if (-not (Test-DockerDaemon)) {
        Write-Warn 'Docker 守护进程未响应，尝试启动 Docker Desktop'
        if (-not (Start-DockerDesktopAndWait)) {
            Write-Err 'Docker 守护进程仍不可用'
            Write-Hint '请手动启动 Docker Desktop 后重新执行本脚本'
            throw 'Docker 守护进程不可用'
        }
    }
    Write-Ok 'Docker 守护进程可用'

    # [4/8] 端口占用
    Write-Step 4 $totalSteps "检查端口占用（后端 $BackendPort）"
    if (Test-PortListening $BackendPort) {
        $existing = & docker ps --filter "publish=$BackendPort" --format '{{.Names}}' 2>$null
        if ($existing) {
            Write-Ok "端口 $BackendPort 已由本项目容器占用（$existing），将走复用分支"
        } else {
            Write-Err "端口 $BackendPort 已被其他进程占用"
            Write-Hint '请关闭占用该端口的程序，或修改 .env 中的 ATT_BACKEND_PORT 后重试'
            throw "端口 $BackendPort 被占用"
        }
    } else {
        Write-Ok "端口 $BackendPort 空闲"
    }

    # [5/8] 镜像
    Write-Step 5 $totalSteps '准备运行时镜像'
    $imageName = 'att-runtime:local'
    if ($Rebuild) {
        Write-Host '  按 -Rebuild 强制重新构建镜像（首次构建需下载较大体积依赖，请耐心等待）'
        Invoke-BuildWithRetry
    } elseif (Test-ImageExists $imageName) {
        if ($NoBuild) {
            Write-Host '  按 -NoBuild 跳过构建'
        } else {
            Write-Ok '镜像已存在，跳过构建（需要重建时加 -Rebuild）'
        }
    } else {
        Write-Host '  镜像不存在，开始构建（首次构建需下载较大体积依赖，请耐心等待）'
        Invoke-BuildWithRetry
    }

    # [6/8] 启动容器
    Write-Step 6 $totalSteps '启动容器'
    [void](Invoke-Compose -ComposeArgs @('up', '-d'))
    Write-Host '  等待 asr 容器健康检查通过（此时模型尚未加载，只确认服务可响应）'
    if (-not (Wait-ContainerHealthy 'att-asr' 180)) {
        $state = Get-ContainerHealth 'att-asr'
        Write-Err "asr 容器未在预期时间内进入 healthy 状态（当前：$state）"
        Write-Hint '请执行 pwsh -File scripts\docker\att.ps1 -Action logs 查看具体报错'
        throw 'asr 容器健康检查未通过'
    }
    Write-Ok 'asr 容器已就绪（服务可响应）'

    # [7/8] GPU 校验
    Write-Step 7 $totalSteps '校验容器内 GPU 可用'
    $gpuProbe = Invoke-InAsrShell 'python3 -c "import torch,sys; sys.exit(0 if torch.cuda.is_available() else 1)"'
    if ($gpuProbe.ExitCode -ne 0) {
        Write-Err 'asr 容器内无法访问 GPU'
        Write-Hint '请确认 Docker Desktop 使用 WSL2 后端且已启用 NVIDIA GPU 支持；可执行 nvidia-smi 验证宿主驱动'
        throw '容器内 GPU 不可用'
    }
    $gpuName = (Invoke-InAsrShell 'nvidia-smi --query-gpu=name --format=csv,noheader').Output
    Write-Ok "容器内 GPU 可用：$gpuName"

    # [8/8] 预热与就绪
    Write-Step 8 $totalSteps "预热模型并等待就绪（超时 ${ReadyTimeoutSeconds}s）"
    Write-Host '  触发 ASR 服务加载模型（首次加载较慢；模型已缓存时通常 30-90 秒）'
    $preload = Invoke-InAsrShell 'curl -fsS -X POST --max-time 900 http://127.0.0.1:9000/api/preload'
    if ($preload.ExitCode -ne 0) {
        Write-Err '模型预热请求失败'
        Write-Hint "容器内错误详情：$($preload.Output)"
        Write-Hint '常见原因：显存不足、/dev/shm 过小、模型目录权限不足'
        throw '模型预热失败'
    }

    $deadline = (Get-Date).AddSeconds($ReadyTimeoutSeconds)
    $ready = $false
    while ((Get-Date) -lt $deadline) {
        $health = Get-AsrHealth
        if ($health -and $health.ready -and $health.loaded) {
            $ready = $true
            break
        }
        if ($health -and $health.error) {
            Write-Err "模型加载失败：$($health.error)"
            throw '模型加载失败'
        }
        Start-Sleep -Seconds 3
    }

    if (-not $ready) {
        Write-Err "等待模型就绪超时（${ReadyTimeoutSeconds}s）"
        Write-Hint '可查看 logs\qwen_streaming_service.log 与容器日志'
        throw '等待模型就绪超时'
    }

    $finalHealth = Get-AsrHealth
    Write-Ok "流式 ASR 已就绪：model=$($finalHealth.model_name)，gpu_memory_utilization=$($finalHealth.gpu_memory_utilization)"

    # 汇总
    Write-Head '启动完成'
    Write-Host "  后端地址    : http://127.0.0.1:$BackendPort  （Electron 会自动复用该端口）"
    Write-Host '  ASR 服务    : 仅在容器网络内可达（不发布到宿主，符合 NFR-5.2）'
    Write-Host '  宿主卷路径  :'
    Write-Host "    数据（含加密密钥）: $DataDir"
    Write-Host "    模型缓存          : $ModelsDir"
    Write-Host "    日志              : $LogsDir"
    Write-Host "    配置              : $EnvFile"
    Write-Host ''
    Write-Host '  下一步：运行 npm run electron:dev:existing-backend 打开桌面端，'
    Write-Host '          或执行 att-start-app.bat 在启动后自动打开桌面端。'
    Write-Host ''
    Write-Host '  警示：data\ 与 models\ 处于 git 忽略状态，git clean -xfd 会删除全部数据，请勿使用。'

    if ($WithApp) {
        Write-Head '启动 Electron 桌面端'
        $env:ATT_REQUIRE_EXISTING_BACKEND = '1'
        $env:ATT_BACKEND_PORT = "$BackendPort"
        Push-Location $RepoRoot
        try {
            & npm run electron:dev:existing-backend
        } finally {
            Pop-Location
        }
    }
}

function Invoke-Stop {
    Write-Head '停止实时 ASR 容器'
    [void](Invoke-Compose -ComposeArgs @('down'))
    Write-Ok '容器已停止（数据卷为宿主目录，未受影响）'

    $leftovers = & docker ps -a --filter 'name=att-' --format '{{.Names}}' 2>$null
    if ($leftovers) {
        Write-Warn "仍有残留容器：$($leftovers -join ', ')"
    } else {
        Write-Ok '无残留容器'
    }
}

function Invoke-Status {
    Write-Head '实时 ASR 运行状态'

    if (-not (Test-DockerDaemon)) {
        Write-Err 'Docker 守护进程未运行'
        return
    }

    Write-Host ''
    Write-Host '-- 容器状态 --'
    [void](Invoke-Compose -ComposeArgs @('ps'))

    Write-Host ''
    Write-Host '-- 健康检查 --'
    Write-Host ("  att-asr     : " + (Get-ContainerHealth 'att-asr'))
    Write-Host ("  att-backend : " + (Get-ContainerHealth 'att-backend'))

    Write-Host ''
    Write-Host '-- 流式就绪状态 --'
    $health = Get-AsrHealth
    if (-not $health) {
        Write-Warn 'ASR 服务不可达（可能容器未启动）'
    } else {
        Write-Host ("  模型        : " + $health.model_name)
        Write-Host ("  已加载      : " + $health.loaded)
        Write-Host ("  就绪        : " + $health.ready)
        Write-Host ("  活跃会话    : " + $health.active_sessions)
        Write-Host ("  采样率      : " + $health.sample_rate)
        if ($health.error) {
            Write-Err ("加载错误    : " + $health.error)
        }
    }

    Write-Host ''
    Write-Host '-- 宿主卷路径 --'
    Write-Host "  数据（含加密密钥）: $DataDir"
    Write-Host "  模型缓存          : $ModelsDir"
    Write-Host "  日志              : $LogsDir"
    Write-Host "  配置              : $EnvFile"
}

# ------------------------------------------------------------------ 入口
try {
    switch ($Action) {
        'start' { Invoke-Start }
        'restart' {
            Invoke-Stop
            Invoke-Start
        }
        'stop' { Invoke-Stop }
        'status' { Invoke-Status }
        'logs' { [void](Invoke-Compose -ComposeArgs @('logs', '-f', '--tail', '100')) }
    }
} catch {
    Write-Host ''
    Write-Err $_.Exception.Message
    Write-Hint '详细排查方式：pwsh -File scripts\docker\att.ps1 -Action logs'
    exit 1
}
