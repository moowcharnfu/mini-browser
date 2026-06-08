# Mini Browser

轻量级跨平台浏览器 - Tauri v2 + React + 系统原生 WebView

## 前置要求

| 平台 | 依赖 |
|------|------|
| **macOS** | Node.js 18+, Rust, Xcode Command Line Tools (`xcode-select --install`) |
| **Windows** | Node.js 18+, Rust, WebView2 (Win10 1803+ 已内置) |
| **Linux** | Node.js 18+, Rust, `sudo apt install libwebkit2gtk-4.1-dev build-essential libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev` |

> 国内网络加速：
> ```bash
> export RUSTUP_DIST_SERVER=https://mirrors.ustc.edu.cn/rust-static
> export RUSTUP_UPDATE_ROOT=https://mirrors.ustc.edu.cn/rust-static/rustup
> curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
> npm config set registry https://registry.npmmirror.com
> ```

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

执行：
1. Vite 构建前端（`dist/`）
2. Rust 全量编译 release binary
3. 生成平台安装包

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

Mini Browser 使用**子窗口方案**确保跨平台兼容性：

- 每个标签页是一个独立的 Tauri `WebviewWindow`（无装饰子窗口）
- 子窗口通过**屏幕绝对坐标**（`innerPosition() + getBoundingClientRect()`）精确覆盖在 content div 上
- 前端 `ResizeObserver` 实时同步 content 区域位置到 Rust 后端
- 标签切换时设置子窗口 `hide()`/`show()` + 重设位置

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
- **内容显示位置不对** — 检查窗口是否被移动过。子窗口使用屏幕坐标定位，移动主窗口后会自动跟随。