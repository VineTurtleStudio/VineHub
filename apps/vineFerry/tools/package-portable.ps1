# 便携版打包：vine-ferry.exe + 便携标记 + 使用说明 → dist-portable\*.zip
# 用法：powershell -ExecutionPolicy Bypass -File tools\package-portable.ps1 [-Profile release|debug]
# 安全保证（与 vineSync tools/package-portable.mjs 同款）：
#   - dist-portable 下的 vine-ferry.exe 正在运行时拒绝打包（不杀进程、不删数据）
#   - 已存在的 portable_data（用户数据）保留：打包时移出，zip 完成后原样放回；zip 不含 portable_data

param(
    [ValidateSet('release', 'debug')]
    [string]$Profile = 'release'
)

$ErrorActionPreference = 'Stop'
$root     = Split-Path -Parent $PSScriptRoot
$wsRoot   = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSScriptRoot))
$exeSrc   = Join-Path $wsRoot "target\$Profile\vine-ferry.exe"
$outDir   = Join-Path $root 'dist-portable'
$stageDir = Join-Path $outDir 'vineFerry-portable'
$zipPath  = Join-Path $outDir 'vineferry-portable.zip'

if (!(Test-Path $exeSrc)) {
    Write-Error "未找到 $exeSrc`n请先执行: cargo build -p vine-ferry$(if ($Profile -eq 'release') { ' --release' })"
}

# 正在从 dist-portable 运行的实例 → 拒绝打包（打包会删除该目录）
$procs = Get-Process vine-ferry -ErrorAction SilentlyContinue
$lowerDir = ([System.IO.Path]::GetFullPath($outDir)).ToLower() + '\'
foreach ($p in $procs) {
    if ($p.Path -and $p.Path.ToLower().StartsWith($lowerDir)) {
        Write-Error "拒绝打包：dist-portable 下的 vine-ferry.exe 正在运行。`n请先退出 vineFerry 再执行（不会强制结束你的程序）。"
    }
}

# 保留旧的 portable_data（用户可能把这个目录当便携安装在用）
$oldData  = Join-Path $stageDir 'portable_data'
$savedData = $null
if (Test-Path $oldData) {
    $savedData = Join-Path ([System.IO.Path]::GetTempPath()) ("vineferry-pdata-" + [System.IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Path $savedData | Out-Null
    Copy-Item -Recurse -Force $oldData (Join-Path $savedData 'portable_data')
}

# 只清自己的舞台目录，绝不删除 dist-portable 下的其他内容
if (Test-Path $stageDir) { Remove-Item -Recurse -Force $stageDir }
New-Item -ItemType Directory -Path $stageDir | Out-Null

Copy-Item -Force $exeSrc (Join-Path $stageDir 'vine-ferry.exe')
Set-Content -Path (Join-Path $stageDir 'vineferry.portable') `
    -Value 'vineFerry portable mode marker. Delete this file to store data in the user profile instead.' `
    -Encoding ASCII

$readme = @"
vineFerry（藤渡）便携版使用说明
====================================
1. 解压后直接运行 vine-ferry.exe，无需安装。
2. 数据（转移历史 SQLite 等）保存在本目录 portable_data\ 下，
   随程序一起移动或复制到 U 盘即可。
3. 删除 vineferry.portable 标记文件后，数据将改存到系统用户目录
   （%LOCALAPPDATA%\vineFerry），行为与安装版一致。
4. 需要 Windows 10/11 与 Microsoft VC++ 运行库。
5. 从资源管理器拖文件进入窗口即加入复制队列。

vineFerry portable edition
====================================
1. Unzip and run vine-ferry.exe — no installation required.
2. Data (transfer history SQLite) lives in portable_data\ next to the exe,
   so you can move or copy the folder to a USB drive.
3. Delete the vineferry.portable marker to store data in the user profile
   (%LOCALAPPDATA%\vineFerry) instead, same as the installed build.
4. Requires Windows 10/11 and the Microsoft VC++ runtime.
5. Drag files from Explorer into the window to enqueue a copy.
"@
$readmePath = Join-Path $stageDir '使用说明.txt'
# UTF-8 BOM（PowerShell 5.1 识别中文用）
[System.IO.File]::WriteAllText($readmePath, $readme, [System.Text.UTF8Encoding]::new($true))

if (Test-Path $zipPath) { Remove-Item -Force $zipPath }
Compress-Archive -Force -Path $stageDir -DestinationPath $zipPath

# zip 已生成（不含 portable_data），把用户数据放回目录（zip 保持干净）
if ($savedData) {
    Copy-Item -Recurse -Force (Join-Path $savedData 'portable_data') $oldData
    Remove-Item -Recurse -Force $savedData
}

$mb = [math]::Round((Get-Item $zipPath).Length / 1MB, 2)
Write-Host ""
Write-Host "便携版打包完成:"
Write-Host "  目录: $stageDir"
Write-Host "  压缩: $zipPath ($mb MiB)"
if (Test-Path $oldData) {
    Write-Host "  已保留原有 portable_data（未打包进 zip，未被删除）"
}
