# Mini Browser

轻量级跨平台浏览器 - Tauri v2 + React + 系统原生 WebView

## 前置要求

| 平台 | 依赖 |
|------|------|
| **macOS** | Node.js 18+, Rust, Xcode Command Line Tools (`xcode-select --install`) |
| **Windows** | Node.js 18+, Rust, WebView2 (Win10 1803+ 已内置) |
| **Linux** | Node.js 18+, Rust, `sudo apt install libwebkit2gtk-4.1-dev build-essential libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev` |

### 安装依赖

```bash
npm install
```

## 开发

### 开发模式（热重载 + 增量编译）

```bash
npm run tauri dev
```

- Vite 开发服务器运行在 `http://localhost:1421`
- Rust 增量编译，修改代码后自动重启
- 首次启动较慢（编译 Rust 依赖），后续修改秒级重编

### 仅启动前端（无 Tauri）

```bash
npm run dev
```

用于单独调试 React UI 布局。

## 构建

### 生产构建

```bash
npm run tauri build
```

### 仅编译 Rust 二进制

```bash
cargo build --release
```

产物：`src-tauri/target/release/mini-browser`

## 测试

```bash
npm run test:unit      # 单元测试（Vitest）
npm run test:e2e       # E2E 测试（Playwright）
npm run test           # 全部测试
```

## 构建产物

`npm run tauri build` 根据当前平台生成对应安装包：

| 平台 | 产物路径 |
|------|---------|
| macOS | `src-tauri/target/release/bundle/macos/mini-browser.app` |
| macOS | `src-tauri/target/release/bundle/dmg/mini-browser_<版本>_x64.dmg` |
| Windows | `src-tauri/target/release/bundle/msi/mini-browser_<版本>_x64.msi` |
| Windows | `src-tauri/target/release/bundle/nsis/mini-browser_<版本>_x64-setup.exe` |
| Linux | `src-tauri/target/release/bundle/deb/mini-browser_<版本>_amd64.deb` |
| Linux | `src-tauri/target/release/bundle/appimage/mini-browser_<版本>_amd64.AppImage` |

> **注意**：打包必须在**项目根目录**执行 `npm run tauri build`。在 `src-tauri/` 目录里直接跑
> `npx tauri build` 会因 `src-tauri/package.json` 的 build 脚本指向 `tauri build` 而无限递归
> （该脚本已改为防递归守卫，会直接报错退出）。

注意：平台打包必须在该平台上执行，不支持交叉编译。

## 架构

Mini Browser 使用**双架构方案**，根据平台自适应：

### macOS / Windows：嵌入 WebView 方案

每个标签页是嵌入主窗口的 `tauri::Webview`（通过 `Window::add_child()` 创建）：

- 所有标签页与 React UI 在同一 OS 窗口内
- 坐标直接使用 viewport 坐标（`getBoundingClientRect()`），无需屏幕坐标转换
- 内容 div 使用 CSS `margin` + `border` + `border-radius` 实现留白效果
- 标签切换时 `webview.hide()`/`webview.show()` + 重设位置
- LRU 淘汰：最多保留 3 个后台 webview，超限销毁最久未使用的

### Linux (Ubuntu/WebKitGTK)：工具栏注入方案

Linux 上 GTK 子 webview 有兼容性问题 + `__TAURI_INTERNALS__.invoke()` 在外部页面不可用，使用完全不同的架构：

- **主窗口由 Rust 创建**（`tauri.conf.json` 中 `create:false`）：设置伪装 User-Agent，并通过
  `on_new_window` 把 `window.open` 转成应用内新标签页
- **不创建子 webview**，只有一个主 webview 直接显示页面；React 界面在首次导航后即被页面替换
- 工具栏通过 `tauri::plugin::Builder::on_page_load` 注入，并有 **2 秒周期的兜底重注入线程**
  （部分站点 load 事件不触发 Finished；页面自行清空 body 时也能自愈）
- 所有 tab 操作通过 URL 参数传递（不用 IPC）：
  `about:blank?__mb_nav/__mb_back/__mb_forward/__mb_switch/__mb_close/__mb_new/__mb_devtools/__mb_clear_cache/__mb_autoclear`
- 历史由 Rust 侧按标签维护（`TabState`：`history` 栈 + 游标），不依赖 webview 全局历史——
  避免协议跳转残留导致后退死循环、以及多标签共用一个 webview 时的历史串台
- 导航归属用 `pending_nav` 绑定：页面加载事件到达时可能已切换标签，按归属标签记账
- 状态每次注入时写入 `window.__MB_STATE__`，脚本按签名比对决定是否重建（不打断用户输入）
- Tab ID 使用 Rust 原子计数器（起始 1000，避免与 React IPC ID 冲突）
- 应用自身页面（dev server / 自定义协议）按精确 URL 前缀识别，避免把 `localhost:PORT` 本地站点误判

技术栈：React + TypeScript + Vite（前端），Tauri v2 + Rust（后端），系统原生 WebView（WebKit / WebView2 / WebKitGTK）

### 图标

图标源文件及生成脚本位于 `src-tauri/icons/`：

```bash
cd src-tauri/icons
python3 generate_icons.py
```

## 常见问题

- **构建很慢** — Rust 全量编译需要 3-8 分钟。开发时用 `npm run tauri dev`（增量编译更快）。修改 `tauri.conf.json` 会触发全量重编。
- **某些网站打不开** — 网站设置了 X-Frame-Options 拒绝 iframe 嵌入，但系统原生 WebView 可通过正常导航绕过（非 iframe 方式）。
- **内容显示位置不对** — 检查窗口是否被移动过。嵌入 webview 使用 viewport 坐标，主窗口移动后 `resize_content_area` 会自动跟随。