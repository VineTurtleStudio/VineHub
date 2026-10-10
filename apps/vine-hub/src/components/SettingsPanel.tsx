import type { PluginView } from "../types";

interface Props {
  pluginsDir: string;
  plugins: PluginView[];
  onRescan: () => void;
  onSelectPlugin: (id: string) => void;
}

export function SettingsPanel({ pluginsDir, plugins, onRescan, onSelectPlugin }: Props) {
  const available = plugins.filter((p) => p.available).length;
  const broken = plugins.filter((p) => !p.available).length;

  return (
    <section className="detail">
      <header className="detail-header">
        <div>
          <h1>设置</h1>
          <div className="detail-subtitle">VineHub 宿主配置</div>
        </div>
        <div className="detail-actions">
          <button className="btn" onClick={onRescan}>
            重新扫描插件
          </button>
        </div>
      </header>

      <div className="card">
        <h2>插件目录</h2>
        <div className="field">
          <label>当前扫描目录</label>
          <code className="path">{pluginsDir || "（未就绪）"}</code>
        </div>
        <p className="hint">
          开发期指向仓库内 <code>plugins/</code>；安装版默认为{" "}
          <code>%APPDATA%\\VineHub\\plugins</code>，可在数据库{" "}
          <code>settings[plugins.dir]</code> 覆盖。
        </p>
      </div>

      <div className="card">
        <h2>插件总览</h2>
        <p className="hint">
          共 {plugins.length} 个：可用 {available}，不可用 {broken}。单个插件失败不影响
          其他插件；点击条目可进入详情配置 exe 路径覆盖。
        </p>
        <table className="plugin-table">
          <thead>
            <tr>
              <th>名称</th>
              <th>ID</th>
              <th>版本</th>
              <th>状态</th>
            </tr>
          </thead>
          <tbody>
            {plugins.map((p) => (
              <tr key={p.id} onClick={() => onSelectPlugin(p.id)}>
                <td>{p.name}</td>
                <td>
                  <code>{p.id}</code>
                </td>
                <td>v{p.version}</td>
                <td>
                  <span className={"badge " + (p.available ? "ok" : "warn")}>
                    {p.available ? "可用" : (p.statusReason ?? "不可用")}
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="card">
        <h2>关于</h2>
        <p className="hint">
          VineHub v0.1.0 · 插件规范 v1 · API 1.0 · identifier{" "}
          <code>com.vineturtle.vinehub</code>
        </p>
      </div>
    </section>
  );
}
