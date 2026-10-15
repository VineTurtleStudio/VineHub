<#
.SYNOPSIS
  打包 vinePulse 绿色版 zip（M4）。

.DESCRIPTION
  产出 zip 内部结构（相对 zip 根）：
    VinePulse/vine-pulse.exe
    VinePulse/tools/ffmpeg/ffmpeg.exe
    VinePulse/tools/ffmpeg/ffprobe.exe
    VinePulse/README.md

  ffmpeg 从 zip 内自解压 vendor\ffmpeg-release-essentials.zip 取出
  （不做本地全量解压，只抽出 bin/ffmpeg.exe + bin/ffprobe.exe 两个 entry），
  避免在磁盘上落一份 ~200MB 的完整解压树。

.NOTES
  - ffmpeg 缺失/抽取失败不阻断打包：zip 照出，只是不含 tools\ffmpeg\。
    vinePulse 会退回结构校验（tools::ffmpeg_available() == false）。
  - 输出默认到 <repo>\target\pack\VinePulse-<version>-windows-x64.zip
#>
[CmdletBinding()]
param(
    [string]$RepoRoot = $(if ($PSScriptRoot) { $PSScriptRoot } elseif ($MyInvocation.MyCommand.Path) { Split-Path -Parent $MyInvocation.MyCommand.Path } else { (Get-Location).Path }),
    [string]$OutDir,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

function Write-Step([string]$msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Write-Ok([string]$msg)   { Write-Host "    $msg" -ForegroundColor Green }
function Write-Warn2([string]$msg){ Write-Host "    [warn] $msg" -ForegroundColor Yellow }

$RepoRoot = (Resolve-Path -LiteralPath $RepoRoot).Path
if (-not $OutDir) { $OutDir = Join-Path $RepoRoot 'target\pack' }

$PkgToml  = Join-Path $RepoRoot 'apps\vinePulse\Cargo.toml'
$Readme   = Join-Path $RepoRoot 'apps\vinePulse\README.md'
$Exe      = Join-Path $RepoRoot 'target\release\vine-pulse.exe'
$VendorZ  = Join-Path (Split-Path -Parent $RepoRoot) 'vendor\ffmpeg-release-essentials.zip'
if (-not (Test-Path -LiteralPath $VendorZ)) {
    $VendorZ = Join-Path 'C:\VineTurtleStudio\vendor' 'ffmpeg-release-essentials.zip'
}

# --- 版本号 ---------------------------------------------------------------
Write-Step "读取版本号"
$version = $null
if (Test-Path -LiteralPath $PkgToml) {
    $m = Select-String -Path $PkgToml -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($m) { $version = $m.Matches[0].Groups[1].Value }
    # version.workspace = true → 到 workspace 根 Cargo.toml 的 [workspace.package] 取
    if (-not $version) {
        $wsInherit = Select-String -Path $PkgToml -Pattern '^\s*version\.workspace\s*=\s*true' | Select-Object -First 1
        if ($wsInherit) {
            # 逐级向上找带 [workspace.package] 的根 Cargo.toml
            $probe = Split-Path -Parent $PkgToml
            while ($probe) {
                $candidate = Join-Path $probe 'Cargo.toml'
                if (Test-Path -LiteralPath $candidate) {
                    $ws = Select-String -Path $candidate -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
                    if ($ws) { $version = $ws.Matches[0].Groups[1].Value; break }
                }
                $probe = Split-Path -Parent $probe
            }
        }
    }
}
if (-not $version) { $version = '0.0.0' }
Write-Ok "version = $version"

# --- release 构建 ---------------------------------------------------------
if (-not $SkipBuild) {
    Write-Step "cargo build --release (vine-pulse)"
    Push-Location $RepoRoot
    try {
        cargo build -p vine-pulse --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build 失败（exit=$LASTEXITCODE）" }
    } finally { Pop-Location }
} else {
    Write-Step "跳过构建（-SkipBuild）"
}

if (-not (Test-Path -LiteralPath $Exe)) {
    throw "找不到 $Exe —— 构建产物缺失"
}
Write-Ok ("exe = {0:N1} MB" -f ((Get-Item -LiteralPath $Exe).Length / 1MB))

# --- 组装暂存目录 ---------------------------------------------------------
Write-Step "组装暂存目录"
$stage      = Join-Path $OutDir 'stage'
$stageRoot  = Join-Path $stage 'VinePulse'
$ffmpegDir  = Join-Path $stageRoot 'tools\ffmpeg'
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Path $ffmpegDir -Force | Out-Null

Copy-Item -LiteralPath $Exe -Destination (Join-Path $stageRoot 'vine-pulse.exe') -Force
Write-Ok "vine-pulse.exe"
if (Test-Path -LiteralPath $Readme) {
    Copy-Item -LiteralPath $Readme -Destination (Join-Path $stageRoot 'README.md') -Force
    Write-Ok "README.md"
} else {
    Write-Warn2 "README.md 不存在，跳过"
}

# --- ffmpeg 抽取 ----------------------------------------------------------
Write-Step "抽取 ffmpeg/ffprobe"
if (Test-Path -LiteralPath $VendorZ) {
    try {
        Add-Type -AssemblyName System.IO.Compression.FileSystem -ErrorAction Stop
        $zip = [System.IO.Compression.ZipFile]::OpenRead($VendorZ)
        try {
            $wanted = @('ffmpeg.exe', 'ffprobe.exe')
            foreach ($name in $wanted) {
                # entry 路径用 '/' 分隔；取 bin\ 下目标 exe
                $entry = $zip.Entries |
                    Where-Object { $_.FullName -match ("/bin/$([regex]::Escape($name))$") } |
                    Select-Object -First 1
                if (-not $entry) {
                    Write-Warn2 "zip 内未找到 $name"
                    continue
                }
                $dest = Join-Path $ffmpegDir $name
                # 立即解压到磁盘（Open() 只给流，不能直接 CopyTo 文件）
                [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $dest, $true)
                Write-Ok ("{0}  {1:N1} MB" -f $name, ((Get-Item -LiteralPath $dest).Length / 1MB))
            }
        } finally { $zip.Dispose() }
    } catch {
        Write-Warn2 "ffmpeg 抽取失败：$($_.Exception.Message)"
        Write-Warn2 "zip 将不含 tools\ffmpeg\；程序退回结构校验"
    }
} else {
    Write-Warn2 "未找到 $VendorZ"
    Write-Warn2 "zip 将不含 tools\ffmpeg\；程序退回结构校验"
}

# 若 ffmpeg 一个都没抽到，把空目录删掉，避免 zip 里出现空壳
if (-not (Get-ChildItem -LiteralPath $ffmpegDir -File -ErrorAction SilentlyContinue)) {
    Remove-Item -LiteralPath $ffmpegDir -Recurse -Force -ErrorAction SilentlyContinue
    Write-Warn2 "tools\ffmpeg\ 为空，已移除"
}

# --- 打包 -----------------------------------------------------------------
Write-Step "打包 zip"
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$zipName = "VinePulse-$version-windows-x64.zip"
$zipPath = Join-Path $OutDir $zipName
if (Test-Path -LiteralPath $zipPath) { Remove-Item -LiteralPath $zipPath -Force }

# 打包 stage 内容（即 VinePulse/...），zip 根即 VinePulse\
Compress-Archive -Path (Join-Path $stage 'VinePulse') -DestinationPath $zipPath -CompressionLevel Optimal

$zipItem = Get-Item -LiteralPath $zipPath
Write-Ok ("{0}  {1:N1} MB" -f $zipItem.FullName, ($zipItem.Length / 1MB))

# --- 清理 + 汇总 ----------------------------------------------------------
Write-Step "清理暂存目录"
Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "打包完成：" -ForegroundColor Green
Write-Host "  $zipPath"
Write-Host ""
Write-Host "验收清单（M4 5.4）：" -ForegroundColor Yellow
Write-Host "  1. 三模式回归：cargo test / cargo clippy / CLI samples（exit 2）/ --rpc"
Write-Host "  2. 解压后双击 VinePulse\vine-pulse.exe，GUI 能启动"
Write-Host "  3. 扫大目录做一次压力测试"
Write-Host "  4. 设置页显示 ffmpeg/7z 探测状态"
