export function formatBytes(n: number): string {
  if (n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), units.length - 1);
  const v = n / Math.pow(1024, i);
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

export function statusClass(status: string | null | undefined): string {
  switch (status) {
    case "ok":
      return "ok";
    case "partial":
      return "partial";
    case "skipped":
      return "skipped";
    default:
      return "failed";
  }
}
