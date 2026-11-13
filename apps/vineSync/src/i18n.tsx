import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";

export type Lang = "zh" | "en";

/** 中文为键的基准字典（en 必须覆盖全部键，由类型约束） */
const zh = {
  // ---- 通用 ----
  brandSub: "文件夹同步备份",
  navHome: "主页",
  navHistory: "历史",
  navSettings: "设置",
  themeToggleTitle: "切换深浅色主题",
  statusSyncingQueued: "同步中 · 已排队",
  statusSyncing: "同步中",
  statusIdle: "空闲",
  btnSyncNow: "立即同步",
  btnSyncing: "同步中…",

  // ---- 主页 ----
  folderPairs: "文件夹对",
  enabledCount: "{on} / {total} 已启用",
  emptyHint: "配置 D 盘 → F 盘的备份任务",
  btnNewPair: "新建文件夹对",
  emptyNoPairs: "还没有文件夹对",
  emptyNoPairsDesc1: "创建第一组备份任务，例如 D:\\文档 → F:\\备份\\文档，",
  emptyNoPairsDesc2: "之后可手动、定时或实时自动同步。",
  scanScanning: "正在扫描",
  scanSource: "源目录",
  scanTarget: "目标目录",
  scanHash: "哈希校验",
  scanFmt: "已扫描 {n} 个文件",
  etaT: "剩余约 {t}",
  durSec: "{n} 秒",
  durMin: "{m} 分 {s} 秒",
  durHour: "{h} 小时 {m} 分",

  // ---- 提示与确认 ----
  errLoadState: "读取应用状态失败",
  toastTargetUnavailable: "目标盘不可用",
  toastSyncIssue: "同步存在问题",
  toastSyncDone: "同步完成",
  toastNoChanges: "没有需要同步的变更",
  errPreview: "生成同步预览失败",
  errStartSync: "启动同步失败",
  toastSaved: "设置已保存",
  toastSaveFailed: "保存失败",
  confirmDeletePair:
    "确定删除文件夹对「{name}」吗？\n只会移除配置，不会删除任何磁盘文件。",
  confirmDeleteTitle: "删除文件夹对",
  confirmClearHistory: "确定清空全部同步历史记录吗？",
  confirmClearTitle: "清空历史",
  toastHistoryCleared: "历史记录已清空",
  toastClearFailed: "清空失败",

  // ---- 文件夹对卡片 ----
  cardUnnamed: "未命名",
  cardToggleOff: "点击停用",
  cardToggleOn: "点击启用",
  tagSrc: "源",
  tagDst: "目标",
  chipNeverSynced: "尚未同步",
  chipAdded: "新增",
  chipUpdated: "更新",
  chipDeleted: "删除",
  chipVersion: "版本",
  chipFailed: "失败",
  chipNoChange: "上次同步无变更",
  filterLabel: "过滤器",
  filterInc: " 包含{n}",
  filterExc: " 排除{n}",
  cardLastSync: "上次同步：{time}",
  cardIdleHint: "准备好后点击「同步」，或等待定时触发",
  btnSync: "同步",
  btnPreview: "预览",
  titleEdit: "编辑",
  titleDelete: "删除",

  // ---- 预览 ----
  previewTitle: "同步预览",
  previewWarnTarget:
    "一个或多个目标盘当前不可用，无法同步。插入磁盘后会由下一次触发自动补同步。",
  sumAdded: "新增 {n}",
  sumUpdated: "更新 {n}",
  sumDeleted: "删除 {n}",
  sumVersioned: "版本备份 {n}",
  sumSuppressed: "策略保留 {n}",
  sumTransfer: "传输 {bytes}",
  suppressedNotice: "当前删除策略为「只增改不删」，有 {n} 个文件的删除被跳过。",
  previewNoChanges: "没有检测到需要同步的变更。",
  previewTruncated: "仅显示前 {shown} 条，共 {total} 条；确认后将同步全部变更。",
  btnCancel: "取消",
  btnConfirmSync: "确认同步（{n} 项）",

  // ---- 编辑器 ----
  editorTitleNew: "新建文件夹对",
  editorTitleEdit: "编辑文件夹对",
  pickSourceTitle: "选择源目录（D 盘）",
  pickDestTitle: "选择目标目录（F 盘）",
  editorPickDirs: "请选择源目录与目标目录",
  labelName: "名称",
  phName: "例如：文档备份",
  labelSource: "源目录（只读，同步的发起方）",
  phSource: "D:\\文档",
  labelDest: "目标目录（备份写入方）",
  phDest: "F:\\备份\\文档",
  btnBrowse: "浏览",
  labelInclude: "包含过滤器（留空 = 全部；每行一条，支持通配符）",
  helpInclude: "只同步匹配的文件；子目录仍会遍历，命中的文件才会纳入。",
  labelExclude: "排除过滤器（每行一条，匹配的目录整棵跳过）",
  helpExclude:
    "顶层的「版本备份」及 $RECYCLE.BIN、System Volume Information 等系统目录由程序保留，自动跳过，不参与同步；不可访问的子树仅跳过、不会导致目标副本被删。",
  labelFilterTest: "过滤器测试（输入相对路径，即时显示是否参与同步）",
  phFilterTest: "项目A\\报告.docx",
  btnTestFilter: "测试",
  labelPairInterval: "定时同步间隔覆盖（分钟，留空 = 使用全局设置）",
  phPairInterval: "留空 = 全局",
  helpPairInterval: "仅在全局触发模式为「按间隔」时生效；计时从上次该文件夹对定时触发算起",
  btnSaving: "保存中…",
  btnSave: "保存",

  // ---- 设置 ----
  settingsTitle: "设置",
  settingsHint: "删除策略、触发方式、通知与主题",
  settingsDirty: "有未保存的更改",
  btnRevert: "还原",
  btnSaveSettings: "保存设置",
  secPolicy: "删除策略",
  polA_t: "A · 镜像 + 版本备份",
  polA_d: "被删除或覆盖的文件带时间戳移入「版本备份」目录，防误删且不弄乱主备份（推荐）",
  polB_t: "B · 纯镜像",
  polB_d: "目标与源完全一致，删除会直接传播到目标，不做任何备份",
  polC_t: "C · 只增改不删",
  polC_d: "新增与更新正常同步，删除不传播；覆盖前自动版本备份，保留全部历史",
  secTrigger: "同步触发方式",
  modeManual_t: "手动",
  modeManual_d: "仅在点击「立即同步」时执行",
  modeInterval_t: "定时",
  modeInterval_d: "应用常驻后台，按设定间隔自动同步（默认每 10 分钟）",
  modeRealtime_t: "实时监控",
  modeRealtime_d: "监听源目录变动（ReadDirectoryChangesW），静默防抖后自动同步",
  intervalT: "定时间隔",
  intervalD: "仅在「定时」模式下生效，范围 1 ~ 1440 分钟",
  unitMin: "分钟",
  debounceT: "实时防抖时间",
  debounceD: "目录静默这段时间后才执行同步，避免编辑过程中频繁触发；范围 0.2 ~ 600 秒",
  unitSec: "秒",
  secBehavior: "行为",
  previewFirstT: "手动同步前先预览",
  previewFirstD: "展示新增 / 更新 / 删除 / 版本备份清单，确认后执行；定时与实时同步始终直接执行",
  notifyT: "同步结果系统通知",
  notifyD: "通过系统通知与托盘气泡提醒同步完成 / 出错（无变更时静默）",
  hashT: "哈希校验（SHA-256）",
  hashD: "对大小与修改时间都一致的文件追加哈希比对，可发现同尺寸同时间的隐性损坏；大目录会明显变慢",
  copyThreadsT: "复制线程数",
  copyThreadsD:
    "并行复制的线程数，1 为顺序执行；大目录 / SSD / NAS 建议 4 ~ 8，机械盘可降至 2",
  unitThreads: "线程",
  verifyAfterCopyT: "复制后深度校验",
  verifyAfterCopyD:
    "复制完成后回读目标文件与源比对哈希；不一致自动重试。更可靠但大文件会明显变慢，默认关闭",
  diffScanT: "快速扫描（差分）",
  diffScanD:
    "目标端清单改用上次成功同步的快照推导，跳过目标目录遍历，大目录预览与同步更快；代价是目标端外部手工增删在下次完整扫描前不可见，默认关闭",
  afterSyncT: "同步后动作",
  afterSyncD: "本轮全部文件夹对同步成功（无失败、目标盘可用）后执行",
  afterSyncNone: "无",
  afterSyncScript: "执行脚本",
  afterSyncShutdown: "关机",
  afterSyncScriptPathT: "脚本路径",
  afterSyncScriptPathD: "支持 .ps1 / .bat / .cmd / 可执行文件；失败仅记录日志，不影响同步结果",
  afterSyncScriptPh: "D:\\backup\\after-sync.ps1",
  afterSyncShutdownWarn: "同步全部成功后将延迟 60 秒关机；如需取消请执行 shutdown /a",
  autostartT: "开机自启动",
  autostartD: "登录 Windows 后自动启动 vineSync，并在后台执行定时与实时同步",
  secVersion: "版本备份",
  versionOnReplaceT: "覆盖前备份旧文件",
  versionOnReplaceD: "更新时先把被覆盖的旧文件移入版本备份；策略 B（纯镜像）始终不备份",
  versionOnDeleteT: "源删除时移入版本备份",
  versionOnDeleteD: "关闭后，策略 A 对源端删除改为直接删除目标副本（不再留档）",
  keepDaysT: "保留天数",
  keepDaysD: "超过该天数的版本备份文件将被清理（目标目录下的「版本备份」文件夹）",
  versionMaxT: "每文件最多保留份数",
  versionMaxD: "0 = 不限；超出份数时自动删除该文件最旧的历史版本（与保留天数独立生效）",
  versionTsT: "版本时间戳格式",
  versionTsD:
    "chrono 格式串，用于版本备份文件名；%Y-%m-%d_%H-%M-%S（默认）→ 2026-10-09_02-15-30",
  unitCount: "份",
  unitDay: "天",
  btnCleaning: "清理中…",
  btnCleanNow: "立即清理",
  cleanupDone: "版本备份清理完成",
  cleanupDoneDesc: "已删除 {n} 个超过 {days} 天的备份文件",
  cleanupFail: "清理失败",
  secAppearance: "外观",
  themeT: "主题",
  themeD: "深色 / 浅色界面，随时切换",
  thSystem: "跟随系统",
  thDark: "深色",
  thLight: "浅色",
  secGeneral: "通用",
  langT: "语言",
  langD: "界面显示语言，保存后同步结果与系统通知一并切换",
  langZh: "中文",
  langEn: "English",
  secStorage: "数据目录",
  storageD: "配置与同步状态（SQLite）的保存位置",
  dataDirLabel: "数据目录：{path}",
  portableBadge: "便携模式",
  portableDesc: "数据随程序目录（portable_data），可在 U 盘上直接运行",
  exeLabel: "程序位置：{path}",
  secAbout: "关于",
  aboutT: "vineSync {ver}",
  aboutD: "现代化的 Windows 文件夹同步备份工具 · Tauri 2 + React + Rust · VineTurtleStudio",
  chipMirror: "单向镜像 D → F",

  // ---- 历史 ----
  historyTitle: "同步历史",
  historyCount: "共 {n} 条记录",
  historyHint: "每次同步的结果与变更数量",
  btnRefresh: "刷新",
  btnPause: "暂停",
  btnResume: "继续",
  pausedChip: "已暂停",
  btnClearHistory: "清空历史",
  btnExport: "导出 CSV",
  btnExporting: "导出中…",
  exportFailed: "导出失败",
  historyEmpty: "暂无同步记录",
  historyEmptyDesc: "执行一次同步后，这里会显示时间、变更数量与失败原因。",
  thTime: "时间",
  thPair: "文件夹对",
  thTrigger: "触发",
  thStatus: "状态",
  thAdded: "新增",
  thUpdated: "更新",
  thDeleted: "删除",
  thVersion: "版本",
  thFailed: "失败",
  thMessage: "说明",
  legendTrigger: "触发：手动 = 点击同步按钮；定时 = 后台计划；实时 = 目录监听",
  legendStatus: "状态：部分失败 = 部分文件同步成功但存在错误",

  // ---- 状态 / 触发 / 变更类型 ----
  st_ok: "成功",
  st_partial: "部分失败",
  st_failed: "失败",
  st_skipped: "已跳过",
  tr_manual: "手动",
  tr_interval: "定时",
  tr_realtime: "实时",
  tr_queued: "排队",
  kind_add: "新增",
  kind_update: "更新",
  kind_delete: "删除",
  kind_versioned: "版本备份",
};

export type Key = keyof typeof zh;
type TVars = Record<string, string | number>;
export type TFn = (key: Key, vars?: TVars) => string;

const en: typeof zh = {
  brandSub: "Folder sync backup",
  navHome: "Home",
  navHistory: "History",
  navSettings: "Settings",
  themeToggleTitle: "Toggle dark/light theme",
  statusSyncingQueued: "Syncing · queued",
  statusSyncing: "Syncing",
  statusIdle: "Idle",
  btnSyncNow: "Sync now",
  btnSyncing: "Syncing…",

  folderPairs: "Pairs",
  enabledCount: "{on} / {total} enabled",
  emptyHint: "Configure D: → F: backup tasks",
  btnNewPair: "New pair",
  emptyNoPairs: "No pairs yet",
  emptyNoPairsDesc1: "Create your first backup task, e.g. D:\\Docs → F:\\Backup\\Docs,",
  emptyNoPairsDesc2: "then sync manually, on a schedule, or in real time.",
  scanScanning: "Scanning",
  scanSource: "source",
  scanTarget: "target",
  scanHash: "hash check",
  scanFmt: "{n} files scanned",
  etaT: "ETA ~{t}",
  durSec: "{n}s",
  durMin: "{m}m {s}s",
  durHour: "{h}h {m}m",

  errLoadState: "Failed to load app state",
  toastTargetUnavailable: "Target drive unavailable",
  toastSyncIssue: "Sync has issues",
  toastSyncDone: "Sync finished",
  toastNoChanges: "No changes to sync",
  errPreview: "Failed to build sync preview",
  errStartSync: "Failed to start sync",
  toastSaved: "Settings saved",
  toastSaveFailed: "Save failed",
  confirmDeletePair:
    'Delete pair "{name}"?\nOnly the configuration is removed; no disk files are deleted.',
  confirmDeleteTitle: "Delete pair",
  confirmClearHistory: "Clear all sync history records?",
  confirmClearTitle: "Clear history",
  toastHistoryCleared: "History cleared",
  toastClearFailed: "Clear failed",

  cardUnnamed: "Untitled",
  cardToggleOff: "Click to disable",
  cardToggleOn: "Click to enable",
  tagSrc: "Src",
  tagDst: "Dst",
  chipNeverSynced: "Never synced",
  chipAdded: "Added",
  chipUpdated: "Updated",
  chipDeleted: "Deleted",
  chipVersion: "Versioned",
  chipFailed: "Failed",
  chipNoChange: "No changes last time",
  filterLabel: "Filter",
  filterInc: " include {n}",
  filterExc: " exclude {n}",
  cardLastSync: "Last sync: {time}",
  cardIdleHint: 'Click "Sync" when ready, or wait for a scheduled trigger',
  btnSync: "Sync",
  btnPreview: "Preview",
  titleEdit: "Edit",
  titleDelete: "Delete",

  previewTitle: "Sync preview",
  previewWarnTarget:
    "One or more target drives are unavailable. Plug the disk in and the next trigger will catch up automatically.",
  sumAdded: "Added {n}",
  sumUpdated: "Updated {n}",
  sumDeleted: "Deleted {n}",
  sumVersioned: "Versioned {n}",
  sumSuppressed: "Policy kept {n}",
  sumTransfer: "Transfer {bytes}",
  suppressedNotice:
    'Delete policy is "add & update only": deletion of {n} file(s) was skipped.',
  previewNoChanges: "No changes detected.",
  previewTruncated:
    "Showing first {shown} of {total} change(s); confirming will sync all of them.",
  btnCancel: "Cancel",
  btnConfirmSync: "Confirm sync ({n} items)",

  editorTitleNew: "New pair",
  editorTitleEdit: "Edit pair",
  pickSourceTitle: "Choose source directory (D:)",
  pickDestTitle: "Choose destination directory (F:)",
  editorPickDirs: "Please choose both source and destination directories",
  labelName: "Name",
  phName: "e.g. Documents backup",
  labelSource: "Source (read-only, sync origin)",
  phSource: "D:\\Docs",
  labelDest: "Destination (backup target)",
  phDest: "F:\\Backup\\Docs",
  btnBrowse: "Browse",
  labelInclude: "Include filters (empty = all; one per line, wildcards supported)",
  helpInclude:
    "Only matching files are synced; subdirectories are still traversed and matched files included.",
  labelExclude: "Exclude filters (one per line; matched directories are pruned)",
  helpExclude:
    'Top-level "版本备份" (version backup) folders and system folders such as $RECYCLE.BIN / System Volume Information are reserved and skipped automatically. Unreadable subtrees are skipped and never cause destination copies to be deleted.',
  labelFilterTest: "Filter tester (enter a relative path to see if it will be synced)",
  phFilterTest: "ProjectA\\report.docx",
  btnTestFilter: "Test",
  labelPairInterval: "Interval override (minutes, empty = use global setting)",
  phPairInterval: "empty = global",
  helpPairInterval:
    "Applies only when the global trigger mode is \"interval\"; counted from this pair's last scheduled run",
  btnSaving: "Saving…",
  btnSave: "Save",

  settingsTitle: "Settings",
  settingsHint: "Delete policy, triggers, notifications and theme",
  settingsDirty: "Unsaved changes",
  btnRevert: "Revert",
  btnSaveSettings: "Save settings",
  secPolicy: "Delete policy",
  polA_t: "A · Mirror + version backup",
  polA_d:
    "Deleted or overwritten files are moved to the version-backup directory with timestamps (recommended)",
  polB_t: "B · Pure mirror",
  polB_d: "Destination matches the source exactly; deletions propagate with no backup",
  polC_t: "C · Add & update only",
  polC_d:
    "Adds and updates sync normally, deletions are skipped; overwrites are versioned first",
  secTrigger: "Sync trigger",
  modeManual_t: "Manual",
  modeManual_d: "Runs only when you click Sync now",
  modeInterval_t: "Scheduled",
  modeInterval_d: "Runs in the background at the set interval (default every 10 min)",
  modeRealtime_t: "Real-time",
  modeRealtime_d:
    "Watches the source directory (ReadDirectoryChangesW) and syncs after the quiet period",
  intervalT: "Interval",
  intervalD: "Used in scheduled mode only, range 1 – 1440 minutes",
  unitMin: "min",
  debounceT: "Real-time debounce",
  debounceD:
    "Sync runs after the directory stays quiet this long; range 0.2 – 600 seconds",
  unitSec: "s",
  secBehavior: "Behavior",
  previewFirstT: "Preview before manual sync",
  previewFirstD:
    "Shows the add / update / delete / version list for confirmation; scheduled and real-time syncs always run directly",
  notifyT: "System notification on finish",
  notifyD: "Notify via system toast and tray bubble on completion or errors (silent if unchanged)",
  hashT: "Hash verification (SHA-256)",
  hashD:
    "Extra hash comparison when size and mtime match, catching same-size corruption; noticeably slower on large trees",
  copyThreadsT: "Copy threads",
  copyThreadsD:
    "Threads for parallel copying; 1 = sequential. 4–8 suits large trees / SSD / NAS; lower to 2 for spinning disks",
  unitThreads: "threads",
  verifyAfterCopyT: "Deep verify after copy",
  verifyAfterCopyD:
    "Re-read the destination and compare hashes with the source after copying; mismatches retry automatically. More reliable but slower for large files; off by default",
  diffScanT: "Fast scan (differential)",
  diffScanD:
    "Derive the destination listing from the last successful sync snapshot instead of walking the destination tree; much faster previews and syncs on large trees. External manual changes on the destination stay invisible until the next full scan; off by default",
  afterSyncT: "After-sync action",
  afterSyncD:
    "Runs after every enabled pair syncs successfully (no failures, target available)",
  afterSyncNone: "None",
  afterSyncScript: "Run script",
  afterSyncShutdown: "Shut down",
  afterSyncScriptPathT: "Script path",
  afterSyncScriptPathD:
    ".ps1 / .bat / .cmd / executable supported; failures are logged and never affect the sync result",
  afterSyncScriptPh: "D:\\backup\\after-sync.ps1",
  afterSyncShutdownWarn:
    "Shuts down 60 seconds after a fully successful sync; run shutdown /a to cancel",
  autostartT: "Launch at startup",
  autostartD:
    "Start vineSync automatically after signing in to Windows and run scheduled/real-time syncs in the background",
  secVersion: "Version backup",
  versionOnReplaceT: "Back up old file before overwrite",
  versionOnReplaceD:
    "Move the replaced file into version backup before overwrite; policy B (mirror) never backs up",
  versionOnDeleteT: "Version backups on source deletion",
  versionOnDeleteD:
    "When off, policy A deletes the target copy directly instead of keeping it as a version",
  keepDaysT: "Keep days",
  keepDaysD:
    'Version backups older than this are cleaned (the "版本备份" folder in the destination)',
  versionMaxT: "Max versions per file",
  versionMaxD:
    "0 = unlimited; the oldest history versions beyond the count are removed (applies independently of keep days)",
  versionTsT: "Version timestamp format",
  versionTsD:
    "chrono format string for version backup filenames; %Y-%m-%d_%H-%M-%S (default) → 2026-10-09_02-15-30",
  unitCount: "copies",
  unitDay: "d",
  btnCleaning: "Cleaning…",
  btnCleanNow: "Clean now",
  cleanupDone: "Version backup cleanup finished",
  cleanupDoneDesc: "Removed {n} backup file(s) older than {days} days",
  cleanupFail: "Cleanup failed",
  secAppearance: "Appearance",
  themeT: "Theme",
  themeD: "Dark / light interface, switch anytime",
  thSystem: "System",
  thDark: "Dark",
  thLight: "Light",
  secGeneral: "General",
  langT: "Language",
  langD: "Interface language; sync results and notifications follow after saving",
  langZh: "中文",
  langEn: "English",
  secStorage: "Data directory",
  storageD: "Where configuration and sync state (SQLite) are stored",
  dataDirLabel: "Data directory: {path}",
  portableBadge: "Portable",
  portableDesc: "Data lives next to the app (portable_data); run it from a USB drive",
  exeLabel: "App path: {path}",
  secAbout: "About",
  aboutT: "vineSync {ver}",
  aboutD:
    "Modern one-way folder sync backup for Windows · Tauri 2 + React + Rust · VineTurtleStudio",
  chipMirror: "One-way mirror D → F",

  historyTitle: "Sync history",
  historyCount: "{n} records",
  historyHint: "Result and change counts of each sync",
  btnRefresh: "Refresh",
  btnPause: "Pause",
  btnResume: "Resume",
  pausedChip: "Paused",
  btnClearHistory: "Clear history",
  btnExport: "Export CSV",
  btnExporting: "Exporting…",
  exportFailed: "Export failed",
  historyEmpty: "No sync records yet",
  historyEmptyDesc:
    "After one sync, time, change counts and failure reasons appear here.",
  thTime: "Time",
  thPair: "Pair",
  thTrigger: "Trigger",
  thStatus: "Status",
  thAdded: "Added",
  thUpdated: "Updated",
  thDeleted: "Deleted",
  thVersion: "Versioned",
  thFailed: "Failed",
  thMessage: "Message",
  legendTrigger:
    "Trigger: manual = sync button; scheduled = background timer; realtime = directory watcher",
  legendStatus: "Status: partial = some files synced but errors occurred",

  st_ok: "OK",
  st_partial: "Partial",
  st_failed: "Failed",
  st_skipped: "Skipped",
  tr_manual: "Manual",
  tr_interval: "Scheduled",
  tr_realtime: "Realtime",
  tr_queued: "Queued",
  kind_add: "Add",
  kind_update: "Update",
  kind_delete: "Delete",
  kind_versioned: "Versioned",
};

const DICTS: Record<Lang, typeof zh> = { zh, en };

function translate(lang: Lang, key: Key, vars?: TVars): string {
  const s: string | undefined = DICTS[lang][key];
  if (s === undefined) return key;
  if (!vars) return s;
  let out = s;
  for (const [k, v] of Object.entries(vars)) {
    out = out.split(`{${k}}`).join(String(v));
  }
  return out;
}

interface I18nValue {
  lang: Lang;
  t: TFn;
  /** 预览语言（未保存前临时生效） */
  setPreview: (lang: Lang | null) => void;
  /** 与配置同步的基准语言 */
  setBase: (lang: Lang) => void;
}

const I18nCtx = createContext<I18nValue | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [base, setBase] = useState<Lang>("zh");
  const [preview, setPreview] = useState<Lang | null>(null);
  const lang = preview ?? base;
  const t = useCallback<TFn>((key, vars) => translate(lang, key, vars), [lang]);
  const value = useMemo(
    () => ({ lang, t, setPreview, setBase }),
    [lang, t],
  );
  return <I18nCtx.Provider value={value}>{children}</I18nCtx.Provider>;
}

export function useI18n(): I18nValue {
  const v = useContext(I18nCtx);
  if (!v) throw new Error("useI18n must be used within I18nProvider");
  return v;
}

/** 动态键（状态/触发/变更类型）的标签查询 */
function dynamicLabel(t: TFn, prefix: string, raw: string): string {
  const key = `${prefix}_${raw}` as Key;
  const s = t(key);
  return s === key ? raw : s;
}

export const statusLabel = (t: TFn, status: string | null | undefined) =>
  dynamicLabel(t, "st", status ?? "ok");

export const triggerLabel = (t: TFn, trigger: string) =>
  dynamicLabel(t, "tr", trigger);

export const kindLabel = (t: TFn, kind: string) =>
  dynamicLabel(t, "kind", kind);
