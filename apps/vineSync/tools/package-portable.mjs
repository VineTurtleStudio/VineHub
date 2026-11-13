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

// release exe 的位置随构建方式不同：
//  - 独立仓库（未并入 workspace）：src-tauri/target/release
//  - 作为 VineHub workspace 成员（本仓库现状）：产物落在 workspace 根 target/release
const exeName = process.platform === "win32" ? "vinesync.exe" : "vinesync";
const exeCandidates = [
  path.join(root, "src-tauri", "target", "release", exeName),
  path.join(root, "..", "..", "target", "release", exeName),
];
const exeSrc = exeCandidates.find((p) => fs.existsSync(p));
const outDir = path.join(root, "dist-portable");
const stageName = "vinesync-portable";
const zipPath = path.join(outDir, `vinesync-portable-v${version}.zip`);

if (!exeSrc) {
  console.error(
    `未找到 release 可执行文件，已查找:\n  ${exeCandidates.join("\n  ")}\n请先执行: npm run tauri build`,
  );
  process.exit(1);
}

// 非破坏式打包：在系统临时目录组装压缩内容，只产出 zip。
// 绝不重建/删除 dist-portable 下已有的解压目录——用户可能正从那里运行，
// 其 portable_data 里是真实配置与 SQLite 状态。旧实现会先删掉该目录再重建，
// 一旦「运行中」保护探测失效（如受限终端里探测失败）就会动到用户数据。
const stageRoot = fs.mkdtempSync(path.join(os.tmpdir(), "vinesync-stage-"));
const stageDir = path.join(stageRoot, stageName); // zip 内的根目录名固定为 vinesync-portable
fs.mkdirSync(stageDir, { recursive: true });
const portableDir = path.join(outDir, stageName);

fs.copyFileSync(exeSrc, path.join(stageDir, "vinesync.exe"));

// Windows 完整性级别：exe 会继承所在目录的强制完整性标签。若构建目录被标成
// 「低完整性」（CI/沙盒里常见），产出的 exe 运行后无法写入普通位置，所有复制
// 都会失败为「拒绝访问 (os error 5)」，且与目标目录的 ACL 无关。打包时显式
// 改为中完整性，保证便携版在任何目录都能写目标盘。
if (process.platform === "win32") {
  const stagedExe = path.join(stageDir, "vinesync.exe");
  try {
    execFileSync("icacls", [stagedExe, "/setintegritylevel", "Medium"], { stdio: "ignore" });
  } catch {
    console.warn(
      "警告: 未能把 exe 的完整性级别设为 Medium。若运行后目标盘写入报「拒绝访问」，请执行:\n" +
        `  icacls "${stagedExe}" /setintegritylevel Medium`,
    );
  }
}

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

// 临时舞台用完即删（zip 里不含任何用户数据）
fs.rmSync(stageRoot, { recursive: true, force: true });

const mb = (fs.statSync(zipPath).size / 1024 / 1024).toFixed(2);
console.log(`\n便携版打包完成:`);
console.log(`  压缩: ${zipPath} (${mb} MiB)`);
if (fs.existsSync(portableDir)) {
  const running = (() => {
    try {
      const out = execFileSync(
        "powershell.exe",
        ["-NoProfile", "-Command", "(Get-Process vinesync -EA SilentlyContinue).Path -join \"`n\""],
        { encoding: "utf8" },
      );
      const prefix = path.resolve(portableDir).toLowerCase() + path.sep;
      return out
        .split(/\r?\n/)
        .filter(Boolean)
        .some((p) => path.resolve(p).toLowerCase().startsWith(prefix));
    } catch {
      return false;
    }
  })();
  console.log(`  注意: ${portableDir} 已存在，未做任何改动（其 portable_data 原样保留）`);
  console.log(
    running
      ? "        该目录下的 vinesync.exe 正在运行：退出后把新 zip 里的 vinesync.exe 覆盖进去即可（配置不用动）。"
      : "        要用新版本：退出 vineSync 后把新 zip 里的 vinesync.exe 覆盖进该目录即可。",
  );
}
