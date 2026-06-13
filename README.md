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
| macOS | `src-tauri/target/release/bundle/dmg/mini-browser_1.0.0_x64.dmg` |
| Windows | `src-tauri/target/release/bundle/msi/mini-browser_1.0.0_x64.msi` |
| Windows | `src-tauri/target/release/bundle/nsis/mini-browser_1.0.0_x64-setup.exe` |
| Linux | `src-tauri/target/release/bundle/deb/mini-browser_1.0.0_amd64.deb` |
| Linux | `src-tauri/target/release/bundle/appimage/mini-browser_1.0.0_amd64.AppImage` |

注意：平台打包必须在该平台上执行，不支持交叉编译。

## 架构

Mini Browser 使用**嵌入 webview 方案**：

- 每个标签页是嵌入主窗口的 `tauri::Webview`（通过 `Window::add_child()` 创建）
- 与 React UI 在同一 OS 窗口内，不是独立子窗口
- 坐标直接使用 viewport 坐标（`getBoundingClientRect()`），无需屏幕坐标转换
- 内容 div 使用 CSS `margin` + `border` + `border-radius` 实现留白效果
- LRU 淘汰：最多保留 3 个后台 webview，超限销毁最久未使用的
- 标签切换时 `webview.hide()`/`webview.show()` + 重设位置

技术栈：React + TypeScript + Vite（前端），Tauri v2 + Rust（后端），系统原生 WebView（WebKit / WebView2）

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