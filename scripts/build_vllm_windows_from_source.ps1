param(
    [string]$EnvDir = ".venv",
    [string]$PythonVersion = "3.12",
    [string]$VllmRepoDir = "external/vllm-windows",
    [string]$TorchPackage = "torch==2.8.0+cu128",
    [string]$TorchIndexUrl = "https://download.pytorch.org/whl/cu128",
    [string]$CudaVersion = "12.8",
    [string]$CudaRoot = "",
    [string]$PlatformToolset = "x64",
    [int]$MaxJobs = 8,
    [string]$SetuptoolsPackage = "setuptools==80.10.2",
    [string]$VllmVersionOverride = "0.17.0+cu128",
    [string]$LogFile = "",
    [switch]$SkipSm120ScaledMmWorkaround,
    [switch]$SkipProjectDeps,
    [switch]$BuildWheelOnly
)

$ErrorActionPreference = "Stop"

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$envPath = Join-Path $projectRoot $EnvDir
$pythonExe = Join-Path $envPath "Scripts\python.exe"
$repoPath = if ([System.IO.Path]::IsPathRooted($VllmRepoDir)) {
    $VllmRepoDir
} else {
    Join-Path $projectRoot $VllmRepoDir
}
$wheelOutputDir = Join-Path $projectRoot "wheels"
$logsDir = Join-Path $projectRoot "logs"
$tmpDir = Join-Path $projectRoot "tmp"
$script:BuildLogFile = $null
$torchValidationCode = "import sys, torch; cv = torch.version.cuda or ''; print('python', sys.version); print('torch', torch.__version__); print('cuda_available', torch.cuda.is_available()); print('cuda_version', cv); sys.exit(0 if sys.version_info[:2] == (3, 12) and torch.__version__ == '2.8.0+cu128' and cv.startswith('12.8') else 1)"
$pipValidationCode = "import importlib.util, sys; sys.exit(0 if importlib.util.find_spec('pip') is not None else 1)"
$sitePackagesPath = Join-Path $envPath "Lib\site-packages"

function Write-Step {
    param([string]$Message)
    $line = "[vllm-build] $Message"
    Write-Host $line
    if ($script:BuildLogFile) {
        Add-Content -Path $script:BuildLogFile -Value $line -Encoding UTF8
    }
}

function Invoke-Checked {
    param(
        [string]$Description,
        [scriptblock]$Action
    )

    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE"
    }
}

function Resolve-VcVarsAllPath {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) {
        throw "vswhere.exe was not found. Please install Visual Studio 2022 Build Tools with C++ components."
    }

    $resolved = & $vswhere `
        -latest `
        -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -find "VC\Auxiliary\Build\vcvarsall.bat" |
        Select-Object -First 1

    if (-not $resolved) {
        throw "vcvarsall.bat was not found. Please install Visual Studio 2022 Build Tools with C++ components."
    }

    return $resolved.Trim()
}

function Resolve-CMakeExecutable {
    $cmakeCommand = Get-Command cmake -ErrorAction SilentlyContinue
    if ($cmakeCommand) {
        return $cmakeCommand.Source
    }

    $vsCMake = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe"
    if (Test-Path $vsCMake) {
        return $vsCMake
    }

    throw "cmake.exe was not found. Please install Visual Studio CMake components or add CMake to PATH."
}

function Resolve-WindowsSdkTools {
    $kitsRoot = "C:\Program Files (x86)\Windows Kits\10\bin"
    if (-not (Test-Path $kitsRoot)) {
        throw "Windows Kits bin directory was not found. Please install the Windows 10/11 SDK."
    }

    $versionDirs = Get-ChildItem -Path $kitsRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' } |
        Sort-Object Name -Descending

    foreach ($dir in $versionDirs) {
        $x64Dir = Join-Path $dir.FullName "x64"
        $rcExe = Join-Path $x64Dir "rc.exe"
        $mtExe = Join-Path $x64Dir "mt.exe"
        if ((Test-Path $rcExe) -and (Test-Path $mtExe)) {
            return @{
                BinDir = $x64Dir
                RcExe = $rcExe
                MtExe = $mtExe
            }
        }
    }

    throw "rc.exe and mt.exe were not found under the Windows SDK. Please install the Windows 10/11 SDK x64 tools."
}

function Resolve-WindowsSdkLayout {
    $kitsRoot = "C:\Program Files (x86)\Windows Kits\10"
    if (-not (Test-Path $kitsRoot)) {
        throw "Windows Kits root was not found. Please install the Windows 10/11 SDK."
    }

    $libRoot = Join-Path $kitsRoot "Lib"
    $includeRoot = Join-Path $kitsRoot "Include"
    $versionDirs = Get-ChildItem -Path $libRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' } |
        Sort-Object Name -Descending

    foreach ($dir in $versionDirs) {
        $version = $dir.Name
        $umLib = Join-Path $dir.FullName "um\x64"
        $ucrtLib = Join-Path $dir.FullName "ucrt\x64"
        $sharedInclude = Join-Path $includeRoot "$version\shared"
        $umInclude = Join-Path $includeRoot "$version\um"
        $ucrtInclude = Join-Path $includeRoot "$version\ucrt"
        $winrtInclude = Join-Path $includeRoot "$version\winrt"
        $cppwinrtInclude = Join-Path $includeRoot "$version\cppwinrt"

        if ((Test-Path $umLib) -and (Test-Path $ucrtLib) -and
            (Test-Path $sharedInclude) -and (Test-Path $umInclude) -and (Test-Path $ucrtInclude)) {
            return @{
                Version = $version
                UmLib = $umLib
                UcrtLib = $ucrtLib
                SharedInclude = $sharedInclude
                UmInclude = $umInclude
                UcrtInclude = $ucrtInclude
                WinrtInclude = $winrtInclude
                CppWinrtInclude = $cppwinrtInclude
            }
        }
    }

    throw "Windows SDK include/lib layout was not found under $kitsRoot"
}

function Resolve-LatestMsvcToolset {
    $msvcRoot = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC"
    if (-not (Test-Path $msvcRoot)) {
        throw "MSVC tools directory was not found. Please install Visual Studio 2022 Build Tools."
    }

    $latest = Get-ChildItem -Path $msvcRoot -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending |
        Select-Object -First 1

    if (-not $latest) {
        throw "No MSVC toolset was found under $msvcRoot"
    }

    $binDir = Join-Path $latest.FullName "bin\HostX64\x64"
    $includeDir = Join-Path $latest.FullName "include"
    $libDir = Join-Path $latest.FullName "lib\x64"
    $clExe = Join-Path $binDir "cl.exe"

    if (-not (Test-Path $clExe)) {
        throw "cl.exe was not found in the latest MSVC toolset: $binDir"
    }

    $vcvarsVersion = ($latest.Name -split '\.')[0..1] -join '.'
    return @{
        ToolsetDir = $latest.FullName
        Version = $latest.Name
        VcVarsVersion = $vcvarsVersion
        BinDir = $binDir
        IncludeDir = $includeDir
        LibDir = $libDir
        ClExe = $clExe
    }
}

function Quote-CmdArg {
    param([string]$Value)

    if ($null -eq $Value -or $Value -eq "") {
        return '""'
    }

    return '"' + $Value.Replace('"', '\"') + '"'
}

function Resolve-ShortWindowsPath {
    param([string]$PathValue)

    if (-not $PathValue) {
        return ""
    }

    $resolved = (Resolve-Path $PathValue).Path
    $cmdPath = $resolved.Replace('"', '\"')
    $short = & cmd.exe /d /c "for %I in (""$cmdPath"") do @echo %~sI"
    if ($LASTEXITCODE -ne 0 -or -not $short) {
        return $resolved
    }

    return ($short | Select-Object -First 1).Trim()
}

function New-CudaAliasPath {
    param(
        [string]$TargetPath,
        [string]$WorkspaceRoot
    )

    $resolvedTarget = (Resolve-Path $TargetPath).Path

    $aliasRoot = Join-Path $WorkspaceRoot "tmp\cuda-toolkit"
    if (Test-Path $aliasRoot) {
        try {
            $currentTarget = (Get-Item $aliasRoot -Force).Target
            if ($currentTarget -and ($currentTarget -eq $resolvedTarget)) {
                return @{
                    Path = $aliasRoot
                    Kind = "junction"
                }
            }
        }
        catch {
            # Ignore target inspection failures and recreate the alias path.
        }

        Remove-Item -Path $aliasRoot -Recurse -Force
    }

    $aliasParent = Split-Path -Parent $aliasRoot
    if (-not (Test-Path $aliasParent)) {
        New-Item -ItemType Directory -Path $aliasParent | Out-Null
    }

    New-Item -ItemType Junction -Path $aliasRoot -Target $resolvedTarget | Out-Null
    return @{
        Path = $aliasRoot
        Kind = "junction"
    }
}

function Resolve-VllmVersionOverride {
    param(
        [string]$RequestedVersion,
        [string]$RepositoryPath
    )

    if ($RequestedVersion) {
        return $RequestedVersion
    }

    $gitCommand = Get-Command git -ErrorAction SilentlyContinue
    if ($gitCommand) {
        try {
            $tag = (& git -C $RepositoryPath describe --tags --abbrev=0 2>$null | Select-Object -First 1)
            if ($tag) {
                $normalized = $tag.Trim()
                if ($normalized.StartsWith("v")) {
                    $normalized = $normalized.Substring(1)
                }
                if ($normalized -and -not $normalized.Contains("+")) {
                    return "$normalized+cu128"
                }
                if ($normalized) {
                    return $normalized
                }
            }
        }
        catch {
            # Ignore git resolution failures and fall back to the default override.
        }
    }

    return "0.17.0+cu128"
}

function Apply-Sm120ScaledMmWorkaround {
    param([string]$RepositoryPath)

    $cmakeFile = Join-Path $RepositoryPath "CMakeLists.txt"
    if (-not (Test-Path $cmakeFile)) {
        throw "CMakeLists.txt was not found under $RepositoryPath"
    }

    $backupPath = "$cmakeFile.codex-sm120-backup"
    if (Test-Path $backupPath) {
        Copy-Item -Path $backupPath -Destination $cmakeFile -Force
    }

    $lines = [System.Collections.Generic.List[string]]::new()
    foreach ($line in Get-Content -Path $cmakeFile) {
        $lines.Add($line)
    }

    $startLine = -1
    $endLine = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i].Contains('if(${CMAKE_CUDA_COMPILER_VERSION} VERSION_GREATER_EQUAL 12.8 AND SCALED_MM_ARCHS)')) {
            $window = ($i + 1)..([Math]::Min($i + 13, $lines.Count - 1))
            $hasSm120 = $false
            foreach ($index in $window) {
                if ($lines[$index].Contains('scaled_mm_sm120_fp8.cu')) {
                    $hasSm120 = $true
                    break
                }
            }
            if ($hasSm120) {
                $startLine = $i
                break
            }
        }
    }

    if ($startLine -lt 0) {
        throw "Unable to locate the SM120 scaled-mm block in $cmakeFile"
    }

    for ($i = $startLine; $i -lt $lines.Count; $i++) {
        if ($lines[$i].Contains('message(STATUS "Building scaled_mm_c3x_sm120 for archs: ${SCALED_MM_ARCHS}")')) {
            $endLine = $i
            break
        }
    }

    if ($endLine -lt 0) {
        throw "Unable to locate the end of the SM120 scaled-mm block in $cmakeFile"
    }

    if (-not (Test-Path $backupPath)) {
        Copy-Item -Path $cmakeFile -Destination $backupPath
    }

    $replacementLines = @(
        '  if(${CMAKE_CUDA_COMPILER_VERSION} VERSION_GREATER_EQUAL 12.8 AND SCALED_MM_ARCHS)',
        '    # codex-sm120-scaled-mm-workaround',
        '    message(STATUS "Skipping scaled_mm_c3x_sm120 kernels due to temporary Windows/MSVC C2719 workaround")'
    )

    $removeCount = ($endLine - $startLine) + 1
    $lines.RemoveRange($startLine, $removeCount)
    $lines.InsertRange($startLine, $replacementLines)

    Set-Content -Path $cmakeFile -Value $lines -Encoding UTF8
    Write-Step "Applied temporary SM120 scaled-mm workaround to $cmakeFile"
}

if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
    throw "uv was not found in PATH. Please install uv first."
}

Push-Location $projectRoot
try {
    $env:UV_CACHE_DIR = Join-Path $projectRoot "tmp\uv-cache"
    $env:UV_PROJECT_ENVIRONMENT = $envPath

    if (-not (Test-Path $logsDir)) {
        New-Item -ItemType Directory -Path $logsDir | Out-Null
    }
    if (-not (Test-Path $tmpDir)) {
        New-Item -ItemType Directory -Path $tmpDir | Out-Null
    }

    $resolvedLogFile = $LogFile
    if (-not $resolvedLogFile) {
        $timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
        $resolvedLogFile = Join-Path $logsDir "vllm-build-$timestamp.log"
    } elseif (-not [System.IO.Path]::IsPathRooted($resolvedLogFile)) {
        $resolvedLogFile = Join-Path $projectRoot $resolvedLogFile
    }

    $resolvedLogDir = Split-Path -Parent $resolvedLogFile
    if ($resolvedLogDir -and -not (Test-Path $resolvedLogDir)) {
        New-Item -ItemType Directory -Path $resolvedLogDir | Out-Null
    }

    if (Test-Path $resolvedLogFile) {
        Remove-Item -Path $resolvedLogFile -Force
    }

    $script:BuildLogFile = $resolvedLogFile
    Set-Content -Path $script:BuildLogFile -Value "[vllm-build] Build log started at $(Get-Date -Format s)" -Encoding UTF8

    if (-not (Test-Path $repoPath)) {
        throw "vLLM source repository was not found: $repoPath"
    }

    if (-not (Test-Path (Join-Path $repoPath "use_existing_torch.py"))) {
        $hint = "VllmRepoDir must point to the vllm-windows source repository root, not the wheels output directory. Example: -VllmRepoDir `"E:\src\vllm-windows`""
        if ((Resolve-Path $repoPath).Path -eq $wheelOutputDir) {
            $hint = "You passed the wheels output directory. VllmRepoDir must point to the cloned vllm-windows source repository root. Example: -VllmRepoDir `"E:\src\vllm-windows`""
        }

        throw "use_existing_torch.py was not found under: $repoPath. $hint"
    }

    if (-not $PSBoundParameters.ContainsKey("SkipSm120ScaledMmWorkaround")) {
        Apply-Sm120ScaledMmWorkaround -RepositoryPath $repoPath
    }

    if (-not (Test-Path $pythonExe)) {
        Write-Step "Creating Python $PythonVersion virtual environment at $EnvDir"
        Invoke-Checked "Creating Python $PythonVersion virtual environment" {
            uv venv $envPath --python $PythonVersion
        }
    }

    if (-not (Test-Path $pythonExe)) {
        throw "Virtual environment Python was not found: $pythonExe"
    }

    if (Test-Path $sitePackagesPath) {
        Get-ChildItem -Path $sitePackagesPath -Force -Filter "~orch*" -ErrorAction SilentlyContinue |
            Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    }

    if (-not $SkipProjectDeps) {
        Write-Step "Installing project dependencies from pyproject.toml"
        Invoke-Checked "Installing project dependencies from pyproject.toml" {
            uv pip install --python $pythonExe -r pyproject.toml
        }
    }

    Write-Step "Installing torch baseline $TorchPackage from $TorchIndexUrl"
    Invoke-Checked "Installing torch baseline $TorchPackage" {
        uv pip install --python $pythonExe --index-url $TorchIndexUrl $TorchPackage
    }

    Write-Step "Validating Python and CUDA $CudaVersion torch baseline"
    Invoke-Checked "Validating Python and CUDA $CudaVersion torch baseline" {
        uv run --python $pythonExe python -c $torchValidationCode
    }

    Write-Step "Ensuring pip is available in $EnvDir"
    & $pythonExe -m ensurepip --upgrade
    if ($LASTEXITCODE -ne 0) {
        throw "Bootstrapping pip with ensurepip failed with exit code $LASTEXITCODE"
    }

    Invoke-Checked "Validating pip availability" {
        uv run --python $pythonExe python -c $pipValidationCode
    }

    Write-Step "Upgrading pip, wheel and pinning $SetuptoolsPackage"
    Invoke-Checked "Upgrading pip, wheel and pinning $SetuptoolsPackage" {
        & $pythonExe -m pip install --upgrade pip wheel $SetuptoolsPackage
    }

    $vcvarsallPath = Resolve-VcVarsAllPath
    $cmakeExePath = Resolve-CMakeExecutable
    $cmakeBinDir = Split-Path -Parent $cmakeExePath
    $windowsSdkTools = Resolve-WindowsSdkTools
    $windowsSdkLayout = Resolve-WindowsSdkLayout
    $msvcToolset = Resolve-LatestMsvcToolset
    Write-Step "Using vcvarsall at $vcvarsallPath"

    if (-not (Test-Path $wheelOutputDir)) {
        New-Item -ItemType Directory -Path $wheelOutputDir | Out-Null
    }

    $pythonCmd = Quote-CmdArg $pythonExe
    $repoCmd = Quote-CmdArg $repoPath
    $wheelCmd = Quote-CmdArg $wheelOutputDir
    $vcvarsCmd = Quote-CmdArg $vcvarsallPath
    $resolvedVersionOverride = Resolve-VllmVersionOverride -RequestedVersion $VllmVersionOverride -RepositoryPath $repoPath
    $resolvedCudaRoot = $CudaRoot
    if (-not $resolvedCudaRoot) {
        $preferredCudaRoot = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v$CudaVersion"
        if (Test-Path $preferredCudaRoot) {
            $resolvedCudaRoot = $preferredCudaRoot
        } elseif ($env:CUDA_PATH) {
            $resolvedCudaRoot = $env:CUDA_PATH
        } else {
            throw "CUDA toolkit root was not found. Pass -CudaRoot explicitly or install CUDA $CudaVersion."
        }
    }
    $shortCudaRoot = Resolve-ShortWindowsPath -PathValue $resolvedCudaRoot
    $nvccPath = Join-Path $shortCudaRoot "bin\nvcc.exe"
    if (-not (Test-Path $nvccPath)) {
        throw "nvcc.exe was not found under CUDA root: $shortCudaRoot. Pass -CudaRoot with the installed CUDA toolkit path."
    }
    $cudaAlias = New-CudaAliasPath -TargetPath $resolvedCudaRoot -WorkspaceRoot $projectRoot
    $cudaAliasPath = $cudaAlias.Path
    $cudaPathForCMake = $cudaAliasPath.Replace("\", "/")
    $windowsSdkBinDir = Resolve-ShortWindowsPath -PathValue $windowsSdkTools.BinDir
    $windowsSdkBinDirForCMake = $windowsSdkBinDir.Replace("\", "/")
    $rcExeForCMake = (Resolve-ShortWindowsPath -PathValue $windowsSdkTools.RcExe).Replace("\", "/")
    $mtExeForCMake = (Resolve-ShortWindowsPath -PathValue $windowsSdkTools.MtExe).Replace("\", "/")
    $windowsSdkUmLibDir = Resolve-ShortWindowsPath -PathValue $windowsSdkLayout.UmLib
    $windowsSdkUcrtLibDir = Resolve-ShortWindowsPath -PathValue $windowsSdkLayout.UcrtLib
    $windowsSdkSharedIncludeDir = Resolve-ShortWindowsPath -PathValue $windowsSdkLayout.SharedInclude
    $windowsSdkUmIncludeDir = Resolve-ShortWindowsPath -PathValue $windowsSdkLayout.UmInclude
    $windowsSdkUcrtIncludeDir = Resolve-ShortWindowsPath -PathValue $windowsSdkLayout.UcrtInclude
    $windowsSdkWinrtIncludeDir = if (Test-Path $windowsSdkLayout.WinrtInclude) { Resolve-ShortWindowsPath -PathValue $windowsSdkLayout.WinrtInclude } else { "" }
    $windowsSdkCppWinrtIncludeDir = if (Test-Path $windowsSdkLayout.CppWinrtInclude) { Resolve-ShortWindowsPath -PathValue $windowsSdkLayout.CppWinrtInclude } else { "" }
    $msvcBinDir = Resolve-ShortWindowsPath -PathValue $msvcToolset.BinDir
    $msvcIncludeDir = Resolve-ShortWindowsPath -PathValue $msvcToolset.IncludeDir
    $msvcLibDir = Resolve-ShortWindowsPath -PathValue $msvcToolset.LibDir
    $clExeForCMake = (Resolve-ShortWindowsPath -PathValue $msvcToolset.ClExe).Replace("\", "/")
    $buildRootDir = Join-Path $repoPath "build\temp.win-amd64-cpython-312"
    if (Test-Path $buildRootDir) {
        Write-Step "Removing stale build cache at $buildRootDir"
        Remove-Item -Path $buildRootDir -Recurse -Force
    }
    $probeCode = Quote-CmdArg "import os, torch; print('build_probe_torch', torch.__version__); print('build_probe_cuda', torch.version.cuda); print('build_probe_target', repr(os.getenv('VLLM_TARGET_DEVICE'))); print('build_probe_cuda_available', torch.cuda.is_available())"
    $cmakeArgsValue = "-DCUDAToolkit_ROOT=$cudaPathForCMake -DCUDA_TOOLKIT_ROOT_DIR=$cudaPathForCMake -DCUDA_TOOLKIT_INCLUDE=$cudaPathForCMake/include -DCUDAToolkit_INCLUDE_DIR=$cudaPathForCMake/include -DCMAKE_CUDA_COMPILER=$cudaPathForCMake/bin/nvcc.exe -Dnvtx3_dir=$cudaPathForCMake/include -DCUDA_cublas_LIBRARY=$cudaPathForCMake/lib/x64/cublas.lib -DCMAKE_RC_COMPILER=$rcExeForCMake -DCMAKE_MT=$mtExeForCMake"
    $includeSegments = @($msvcIncludeDir, $windowsSdkUcrtIncludeDir, $windowsSdkSharedIncludeDir, $windowsSdkUmIncludeDir)
    if ($windowsSdkWinrtIncludeDir) {
        $includeSegments += $windowsSdkWinrtIncludeDir
    }
    if ($windowsSdkCppWinrtIncludeDir) {
        $includeSegments += $windowsSdkCppWinrtIncludeDir
    }
    $includePrefix = [string]::Join(';', $includeSegments)
    $libPrefix = [string]::Join(';', @($msvcLibDir, $windowsSdkUmLibDir, $windowsSdkUcrtLibDir))

    $buildSteps = @(
        "call $vcvarsCmd $PlatformToolset -vcvars_ver=$($msvcToolset.VcVarsVersion)",
        "set `"DISTUTILS_USE_SDK=1`"",
        "set `"VLLM_TARGET_DEVICE=cuda`"",
        "set `"VLLM_MAIN_CUDA_VERSION=$CudaVersion`"",
        "set `"VLLM_VERSION_OVERRIDE=$resolvedVersionOverride`"",
        "set `"MAX_JOBS=$MaxJobs`"",
        "set `"VCToolsVersion=$($msvcToolset.Version)`"",
        "set `"VCToolsInstallDir=$($msvcToolset.ToolsetDir)\`"",
        "set `"CUDA_HOME=$cudaAliasPath`"",
        "set `"CUDA_PATH=$cudaAliasPath`"",
        "set `"CUDA_ROOT=$cudaAliasPath`"",
        "set `"CUDAToolkit_ROOT=$cudaAliasPath`"",
        "set `"CMAKE_ARGS=$cmakeArgsValue`"",
        "set `"RC=$rcExeForCMake`"",
        "set `"MT=$mtExeForCMake`"",
        "set `"WindowsSdkDir=C:\Program Files (x86)\Windows Kits\10\`"",
        "set `"WindowsSDKVersion=$($windowsSdkLayout.Version)\`"",
        "set `"INCLUDE=$includePrefix;%INCLUDE%`"",
        "set `"LIB=$libPrefix;%LIB%`"",
        "set `"PATH=$envPath\Scripts;$cmakeBinDir;$msvcBinDir;$windowsSdkBinDir;$cudaAliasPath\bin;%PATH%`"",
        "cd /d $repoCmd",
        "cmake --version",
        "ninja --version",
        "cl",
        "rc /?",
        "mt -?",
        "$pythonCmd use_existing_torch.py",
        "$pythonCmd -m pip install -r requirements/build.txt --no-deps",
        "$pythonCmd -m pip install $SetuptoolsPackage",
        "$pythonCmd -m pip install --index-url $TorchIndexUrl $TorchPackage",
        "$pythonCmd -m pip install -r requirements/windows.txt --no-deps",
        "$pythonCmd -c $probeCode"
    )

    if ($BuildWheelOnly) {
        $buildSteps += "$pythonCmd setup.py bdist_wheel --dist-dir $wheelCmd"
    } else {
        $buildSteps += "$pythonCmd -m pip install . --no-build-isolation --no-deps --no-use-pep517"
    }

    $commandLine = [string]::Join(" && ", $buildSteps)
    $runnerScriptPath = Join-Path $tmpDir "run-vllm-build.cmd"
    Set-Content -Path $runnerScriptPath -Value "@echo off`r`n$commandLine`r`n" -Encoding ASCII

    Write-Step "Using VLLM_VERSION_OVERRIDE=$resolvedVersionOverride"
    Write-Step "Using CUDA root $shortCudaRoot"
    Write-Step "Using CUDA alias path $cudaAliasPath"
    Write-Step "Using Windows SDK tools at $windowsSdkBinDirForCMake"
    Write-Step "Using Windows SDK version $($windowsSdkLayout.Version)"
    Write-Step "Using MSVC toolset $($msvcToolset.Version)"
    Write-Step "Using CMake at $cmakeExePath"
    Write-Step "Writing build output to $resolvedLogFile"
    Write-Step "Building vLLM from source at $repoPath"
    & cmd.exe /u /d /c "call `"$runnerScriptPath`" >> `"$resolvedLogFile`" 2>&1"
    if ($LASTEXITCODE -ne 0) {
        throw "Building vLLM from source failed with exit code $LASTEXITCODE. See log: $resolvedLogFile"
    }

    Write-Step "vLLM source build completed"
    if ($BuildWheelOnly) {
        Write-Host "Wheel output directory: $wheelOutputDir"
    }
    Write-Host "Build log: $resolvedLogFile"
}
finally {
    Pop-Location
}
