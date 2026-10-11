#Requires -Version 5.1
# vineSync E2E 冒烟（完全隔离版）：定时同步回归 + 英文消息 + 单实例 + 开机自启动注册表
# 隔离保证：
#   - release exe 复制到 .testdata\app\ 并放置 vinesync.portable 标记，
#     数据全部落在 .testdata\app\portable_data\，结束随 .testdata 删除；
#     绝不读写 %APPDATA%\studio.vineturtle.vinesync（真实用户数据）
#   - vineSync 若已在运行则跳过（exit 2），绝不杀用户正在使用的进程
#   - HKCU Run 中现有 vinesync* 值先备份，测试后原样还原
# 用法: powershell -NoProfile -ExecutionPolicy Bypass -File tools\e2e-smoke.ps1
param(
  [int]$WaitSeconds = 75
)
$ErrorActionPreference = 'Stop'
$root       = Split-Path -Parent $PSScriptRoot
$exeSrc     = Join-Path $root 'src-tauri\target\release\vinesync.exe'
$test       = Join-Path $root '.testdata'
$stage      = Join-Path $test 'app'
$exe        = Join-Path $stage 'vinesync.exe'
$data       = Join-Path $stage 'portable_data'
$appdataDir = Join-Path $env:APPDATA 'studio.vineturtle.vinesync'
$runKey     = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

function Ok($msg)   { Write-Host "ok  : $msg" -ForegroundColor Green }
function Fail($msg) {
  Get-Process vinesync -EA SilentlyContinue |
    Where-Object { $_.Path -and ($_.Path -ieq $exe) } | Stop-Process -Force -EA SilentlyContinue
  Write-Host "FAIL: $msg" -ForegroundColor Red
  exit 1
}
function Skip($msg) {
  Write-Host "SKIP: $msg" -ForegroundColor Yellow
  exit 2
}
function Restore-RunKey {
  # 清掉测试写入的值，再原样恢复测试前的备份
  $rk = Get-Item $runKey -EA SilentlyContinue
  if ($rk) {
    foreach ($n in @($rk.GetValueNames() | Where-Object { $_ -like '*vinesync*' })) {
      Remove-ItemProperty -Path $runKey -Name $n -EA SilentlyContinue
    }
    foreach ($b in $script:runBackup) {
      New-ItemProperty -Path $runKey -Name $b.Name -Value $b.Value -PropertyType $b.Kind -Force | Out-Null
    }
  }
}

if (!(Test-Path $exeSrc)) { Fail "exe not found: $exeSrc (run npm run tauri build first)" }

# ---------- 保护：已有实例在运行（可能是用户正在使用）→ 跳过，不杀进程 ----------
$live = @(Get-Process vinesync -EA SilentlyContinue)
if ($live.Count -gt 0) {
  Skip "vineSync is already running (PID $($live.Id -join ',')). Close it first; tests never kill the live app."
}

# ---------- 备份 HKCU Run 现有 vinesync* 值（结束后还原） ----------
$script:runBackup = @()
$rk = Get-Item $runKey -EA SilentlyContinue
if ($rk) {
  foreach ($n in $rk.GetValueNames()) {
    if ($n -like '*vinesync*') {
      $script:runBackup += [pscustomobject]@{ Name = $n; Kind = $rk.GetValueKind($n).ToString(); Value = $rk.GetValue($n) }
      Remove-ItemProperty -Path $runKey -Name $n
    }
  }
}

# ---------- 记录真实数据目录状态（结束时断言未被触碰） ----------
$realBefore = if (Test-Path $appdataDir) {
  (Get-Item $appdataDir).LastWriteTimeUtc.ToString('o')
} else { '<absent>' }

# ---------- 清理并搭建隔离舞台 ----------
Remove-Item $test -Recurse -Force -EA SilentlyContinue
New-Item -ItemType Directory -Path $stage -Force | Out-Null
Copy-Item $exeSrc $exe
Set-Content -Path (Join-Path $stage 'vinesync.portable') -Value 'E2E isolation marker' -Encoding ASCII

# ---------- 准备源/目标与配置 ----------
$src = Join-Path $test 'src'
$dst = Join-Path $test 'dst'
New-Item -ItemType Directory -Path $src, $dst -Force | Out-Null
[IO.File]::WriteAllText((Join-Path $src 'a.txt'), 'aaa', [Text.UTF8Encoding]::new($false))
[IO.File]::WriteAllText((Join-Path $src 'b.txt'), 'bbb', [Text.UTF8Encoding]::new($false))
New-Item -ItemType Directory -Path (Join-Path $src 'sub') -Force | Out-Null
[IO.File]::WriteAllText((Join-Path $src 'sub\c.txt'), 'ccc', [Text.UTF8Encoding]::new($false))

$cfg = [ordered]@{
  pairs             = @([ordered]@{
    id = 'p1'; name = 'E2E'; source = $src; dest = $dst
    enabled = $true; include = @(); exclude = @()
  })
  deletePolicy      = 'versioned'
  trigger           = [ordered]@{ mode = 'interval'; intervalMinutes = 1; debounceMs = 500 }
  hashVerify        = $false
  notifyOnFinish    = $false
  confirmBeforeSync = $false
  versionKeepDays   = 30
  theme             = 'dark'
  language          = 'en'
  autostart         = $true
}
New-Item -ItemType Directory -Path $data -Force | Out-Null
[IO.File]::WriteAllText(
  (Join-Path $data 'config.json'),
  ($cfg | ConvertTo-Json -Depth 5),
  [Text.UTF8Encoding]::new($false)
)
Ok 'seeded isolated config (portable_data in .testdata, language=en, interval=1min)'

# ---------- 启动并等待定时触发 ----------
$null = Start-Process -FilePath $exe -PassThru
Ok "app started, waiting ${WaitSeconds}s for interval trigger..."
Start-Sleep -Seconds $WaitSeconds

# ---------- 单实例：第二实例应退出且仅剩 1 个进程 ----------
$p2 = Start-Process -FilePath $exe -PassThru
$exited = $p2.WaitForExit(6000)
$procs = @(Get-Process vinesync -EA SilentlyContinue)
if (-not $exited) { Fail "second instance did not exit (processes: $($procs.Count))" }
if ($procs.Count -ne 1) { Fail "expected 1 process, got $($procs.Count)" }
Ok 'single instance (second launch exited, 1 process remains)'

# ---------- 停止（只停本测试启动的隔离实例） ----------
Get-Process vinesync -EA SilentlyContinue |
  Where-Object { $_.Path -and ($_.Path -ieq $exe) } | Stop-Process -Force -EA SilentlyContinue
Start-Sleep -Seconds 2

# ---------- 断言：目标镜像 ----------
foreach ($f in @('a.txt', 'b.txt', 'sub\c.txt')) {
  $s = Join-Path $src $f
  $d = Join-Path $dst $f
  if (!(Test-Path $d)) { Fail "missing mirrored file: $f" }
  $bs = [IO.File]::ReadAllBytes($s) -join ','
  $bd = [IO.File]::ReadAllBytes($d) -join ','
  if ($bs -ne $bd) { Fail "content mismatch: $f" }
}
Ok 'dst mirrors src (a.txt, b.txt, sub/c.txt)'

if (!(Test-Path (Join-Path $data 'state.db'))) { Fail 'state.db missing in isolated portable_data' }
if (!(Test-Path $dst)) { Fail 'dst missing' }

# ---------- 断言：数据库行（interval 触发 + 英文消息） ----------
$dbText = [Text.Encoding]::GetEncoding(28591).GetString(
  [IO.File]::ReadAllBytes((Join-Path $data 'state.db'))
)
if ($dbText -notmatch 'E2E')   { Fail 'pair name E2E not found in state.db' }
if ($dbText -notmatch 'interval') { Fail 'no interval trigger row in sync_log' }
if ($dbText -notmatch 'added') { Fail 'English message ("added ...") not found - msg i18n broken?' }
Ok 'sync_log: interval trigger + English summary message'

# ---------- 断言：开机自启动注册表 ----------
$runVals = @((Get-ItemProperty $runKey -EA SilentlyContinue).PSObject.Properties |
  Where-Object { $_.Name -like '*vinesync*' })
if ($runVals.Count -eq 0) { Fail 'autostart HKCU Run value missing' }
Ok ("autostart registry present: " + (($runVals | ForEach-Object Name) -join ', '))
Restore-RunKey
Ok 'autostart registry restored to pre-test state'

# ---------- 断言：真实 %APPDATA% 数据目录从未被触碰 ----------
$realAfter = if (Test-Path $appdataDir) {
  (Get-Item $appdataDir).LastWriteTimeUtc.ToString('o')
} else { '<absent>' }
if ($realAfter -ne $realBefore) { Fail "real app data dir was modified: $appdataDir" }
Ok "real %APPDATA% data untouched ($realAfter)"

# ---------- 清理（仅本测试自己的产物） ----------
Get-Process vinesync -EA SilentlyContinue |
  Where-Object { $_.Path -and ($_.Path -ieq $exe) } | Stop-Process -Force -EA SilentlyContinue
Remove-Item $test -Recurse -Force -EA SilentlyContinue

Write-Host ''
Write-Host 'E2E PASS (isolated)' -ForegroundColor Green
exit 0
