// 便携版打包：vinesync.exe + 便携标记 + 使用说明 → dist-portable/*.zip
// 用法：npm run package:portable（需先 npm run tauri build 产出 release exe）
// 安全保证：
//   - dist-portable 下的 vinesync.exe 正在运行时拒绝打包（不会杀进程、不会删数据）
//   - 已存在的 portable_data（用户数据）会保留：打包时先移出，zip 完成后原样放回
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const pkg = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
const version = pkg.version;

const exeSrc = path.join(root, "src-tauri", "target", "release", "vinesync.exe");
const outDir = path.join(root, "dist-portable");
const stageName = "vinesync-portable";
const stageDir = path.join(outDir, stageName);
const zipPath = path.join(outDir, `vinesync-portable-v${version}.zip`);

if (!fs.existsSync(exeSrc)) {
  console.error(`未找到 ${exeSrc}\n请先执行: npm run tauri build`);
  process.exit(1);
}

// 正在从 dist-portable 运行的实例 → 拒绝打包（打包会删除该目录）
function runningFrom(dir) {
  try {
    const out = execFileSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-Command",
        "(Get-Process vinesync -EA SilentlyContinue).Path -join \"`n\"",
      ],
      { encoding: "utf8" },
    );
    const lowerDir = path.resolve(dir).toLowerCase() + path.sep;
    return out
      .split(/\r?\n/)
      .filter(Boolean)
      .some((p) => path.resolve(p).toLowerCase().startsWith(lowerDir));
  } catch {
    return false; // 探测失败不阻塞打包
  }
}
if (fs.existsSync(outDir) && runningFrom(outDir)) {
  console.error(
    "拒绝打包：dist-portable 下的 vinesync.exe 正在运行。\n" +
      "请先退出 vineSync，再执行 npm run package:portable（不会强制结束你的程序）。",
  );
  process.exit(1);
}

// 保留旧的 portable_data（用户可能把这个目录当便携安装在用）
let savedData = null;
const oldData = path.join(stageDir, "portable_data");
if (fs.existsSync(oldData)) {
  savedData = fs.mkdtempSync(path.join(os.tmpdir(), "vinesync-pdata-"));
  fs.cpSync(oldData, path.join(savedData, "portable_data"), { recursive: true });
}

// 只清自己的舞台目录，绝不删除 dist-portable 下的其他内容
// （用户可能把解压出来的便携版放在 dist-portable 里使用，内含数据）
fs.rmSync(stageDir, { recursive: true, force: true });
fs.mkdirSync(stageDir, { recursive: true });

fs.copyFileSync(exeSrc, path.join(stageDir, "vinesync.exe"));
fs.writeFileSync(
  path.join(stageDir, "vinesync.portable"),
  "vineSync portable mode marker. Delete this file to store data in the user profile instead.\r\n",
  "utf8",
);

const readme = `vineSync 便携版使用说明（v${version}）
====================================
1. 解压后直接运行 vinesync.exe，无需安装。
2. 数据（配置、SQLite 同步状态、WebView2 缓存）保存在本目录 portable_data\\ 下，
   随程序一起移动或复制到 U 盘即可。
3. 删除 vinesync.portable 标记文件后，数据将改存到系统用户目录
   （%APPDATA%\\studio.vineturtle.vinesync），行为与安装版一致。
4. 需要 Windows 10/11 与 Microsoft Edge WebView2 运行时（系统通常已内置，
   缺失时可从 Microsoft 官网下载 "WebView2 Runtime" 安装）。
5. 开机自启动可在「设置 → 行为」中开启（写入当前用户注册表，仅对本机生效）。
6. 单实例：重复启动会激活已打开的主窗口。

vineSync portable edition
====================================
1. Unzip and run vinesync.exe — no installation required.
2. Data (config, SQLite sync state, WebView2 cache) lives in portable_data\\ next
   to the exe, so you can move or copy the folder to a USB drive.
3. Delete the vinesync.portable marker to store data in the user profile
   (%APPDATA%\\studio.vineturtle.vinesync) instead, same as the installed build.
4. Requires Windows 10/11 and Microsoft Edge WebView2 Runtime (preinstalled on
   most systems; install from Microsoft if missing).
5. Enable "Launch at startup" in Settings → Behavior if desired (current user).
6. Single instance: starting it again focuses the already-open window.
`;
fs.writeFileSync(path.join(stageDir, "使用说明.txt"), readme, { encoding: "utf8", flag: "w" });

// PS5.1 用 BOM 识别 UTF-8 说明文件
const txtPath = path.join(stageDir, "使用说明.txt");
const body = fs.readFileSync(txtPath);
if (!(body[0] === 0xef && body[1] === 0xbb && body[2] === 0xbf)) {
  fs.writeFileSync(txtPath, Buffer.concat([Buffer.from([0xef, 0xbb, 0xbf]), body]));
}

const psQuote = (p) => `'${p.replace(/'/g, "''")}'`;
const cmd = [
  `Compress-Archive -Force -Path ${psQuote(stageDir)} -DestinationPath ${psQuote(zipPath)}`,
].join("; ");

execFileSync(
  "powershell.exe",
  ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", cmd],
  { stdio: "inherit" },
);

// zip 已生成（不含 portable_data），把用户数据放回目录（zip 保持干净）
if (savedData) {
  fs.cpSync(path.join(savedData, "portable_data"), oldData, { recursive: true });
  fs.rmSync(savedData, { recursive: true, force: true });
}

const mb = (fs.statSync(zipPath).size / 1024 / 1024).toFixed(2);
console.log(`\n便携版打包完成:`);
console.log(`  目录: ${stageDir}`);
console.log(`  压缩: ${zipPath} (${mb} MiB)`);
if (fs.existsSync(oldData)) {
  console.log(`  已保留原有 portable_data（未打包进 zip，未被删除）`);
}
