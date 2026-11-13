# 便携版打包：cargo build --release → vine-seal.exe + 使用说明 + README → dist-portable\*.zip
# 用法：powershell -ExecutionPolicy Bypass -File tools\package-portable.ps1 [-SkipBuild]
# 安全保证（与 vineFerry tools/package-portable.ps1 同款）：
#   - dist-portable 下的 vine-seal.exe 正在运行时拒绝打包（不杀进程）
#   - 只清理自己的舞台目录，不删除 dist-portable 下的其他内容
# vineSeal 无持久化数据（无 SQLite / 无便携标记），exe 即免安装本体。

param(
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$root     = Split-Path -Parent $PSScriptRoot
$wsRoot   = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
$exeSrc   = Join-Path $wsRoot 'target\release\vine-seal.exe'
$outDir   = Join-Path $root 'dist-portable'
$stageDir = Join-Path $outDir 'vineSeal-portable'
$zipPath  = Join-Path $outDir 'vineseal-portable.zip'

if (!$SkipBuild) {
    Push-Location $wsRoot
    try {
        cargo build --release -p vine-seal
        if ($LASTEXITCODE -ne 0) { throw "cargo build 失败（exit=$LASTEXITCODE）" }
    } finally { Pop-Location }
}

if (!(Test-Path $exeSrc)) {
    Write-Error "未找到 $exeSrc`n请先执行: cargo build --release -p vine-seal"
}

# 正在从 dist-portable 运行的实例 → 拒绝打包（打包会删除该目录）
$procs = Get-Process vine-seal -ErrorAction SilentlyContinue
$lowerDir = ([System.IO.Path]::GetFullPath($outDir)).ToLower() + '\'
foreach ($p in $procs) {
    if ($p.Path -and $p.Path.ToLower().StartsWith($lowerDir)) {
        Write-Error "拒绝打包：dist-portable 下的 vine-seal.exe 正在运行。`n请先退出 vineSeal 再执行（不会强制结束你的程序）。"
    }
}

# 只清自己的舞台目录
if (Test-Path $stageDir) { Remove-Item -Recurse -Force $stageDir }
New-Item -ItemType Directory -Path $stageDir | Out-Null

Copy-Item -Force $exeSrc (Join-Path $stageDir 'vine-seal.exe')
$readme = Join-Path $root 'README.md'
if (Test-Path $readme) {
    Copy-Item -Force $readme (Join-Path $stageDir 'README.md')
}

$usage = @"
vineSeal（藤印）便携版使用说明
====================================
1. 解压后直接运行 vine-seal.exe，无需安装、无数据目录，可放 U 盘随身携带。
2. 双击（无参数）打开图形界面：选源和目标 → 选「复制/移动」→ 点「开始」。
3. 命令行用法（在终端中执行）：
     vine-seal copy <源>... <目标目录>   复制并校验
     vine-seal move <源>... <目标目录>   移动并校验（校验通过才删源）
     vine-seal verify <文件> --hash <哈希>      按期望哈希校验单个文件
     vine-seal verify -m <清单> <基目录>        按 .sha256 清单校验目录
     vine-seal hash <文件>                     只计算并打印哈希
   可选：-a sha256|xxh3|crc32|md5（默认 sha256）、-c skip|overwrite|rename、--perfect
   退出码：0 = 全部成功 / 1 = 有失败或校验不符 / 2 = 参数错误
4. 安全承诺：先写入 目标名.seal.tmp，复读哈希比对通过才改名为正式文件；
   移动时校验通过才删除源文件；校验不过时源文件原封不动。
5. 需要 Windows 10/11 与 Microsoft VC++ 运行库。

vineSeal portable edition
====================================
1. Unzip and run vine-seal.exe — no installation, no data folder required.
2. Double-click (no args) for the GUI: pick source/destination, choose
   copy/move, press Start.
3. CLI: vine-seal copy|move|verify|hash — see README.md for details.
   Exit codes: 0 = all ok / 1 = failures or hash mismatch / 2 = usage error.
4. Safety: writes to "name.seal.tmp", re-reads and compares hashes, then
   atomically renames; a move deletes the source only after verification passes.
5. Requires Windows 10/11 and the Microsoft VC++ runtime.
"@
$usagePath = Join-Path $stageDir '使用说明.txt'
# UTF-8 BOM（PowerShell 5.1 识别中文用）
[System.IO.File]::WriteAllText($usagePath, $usage, [System.Text.UTF8Encoding]::new($true))

if (Test-Path $zipPath) { Remove-Item -Force $zipPath }
Compress-Archive -Force -Path $stageDir -DestinationPath $zipPath

$mb = [math]::Round((Get-Item $zipPath).Length / 1MB, 2)
$exeMb = [math]::Round((Get-Item $exeSrc).Length / 1MB, 2)
Write-Host ""
Write-Host "便携版打包完成:"
Write-Host "  目录: $stageDir"
Write-Host "  压缩: $zipPath ($mb MiB)"
Write-Host "  exe:  vine-seal.exe ($exeMb MiB, release)"
