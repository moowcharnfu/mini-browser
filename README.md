# Mini Browser

轻量级跨平台浏览器 - Tauri v2 + React + 系统原生 WebView

## 前置要求

| 平台 | 依赖 |
|------|------|
| **macOS** | Node.js 18+, Rust, Xcode Command Line Tools (`xcode-select --install`) |
| **Windows** | Node.js 18+, Rust, WebView2 (Win10 1803+ 已内置) |
| **Linux** | Node.js 18+, Rust, `sudo apt install libwebkit2gtk-4.1-dev build-essential libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev` |
```
  #rust & tauri中国源
  export RUSTUP_DIST_SERVER=https://mirrors.ustc.edu.cn/rust-static && export RUSTUP_UPDATE_ROOT=https://mirrors.ustc.edu.cn/rust-static/rustup && curl --proto '=https' --tlsv1.2 -sSf   https://sh.rustup.rs | sh
  ```

### 安装依赖

```bash
npm install
```

## 1. 构建工程

### 开发模式（热重载 + 增量编译）

```bash
npm run tauri dev
```

- Vite 开发服务器运行在 `http://localhost:1421`
- Rust 增量编译，修改代码后自动重启
- 首次启动较慢（编译 Rust 依赖），后续修改秒级重编

### 生产构建（Release）

```bash
npm run tauri build
```

执行：
1. Vite 构建前端（`dist/`）
2. Rust 全量编译 release binary
3. 生成平台安装包（见下节）

构建产物位于：`src-tauri/target/release/`

### 仅编译 Rust 二进制（不做打包）

```bash
cargo build --release -j $(sysctl -n hw.ncpu)
```

产物：`src-tauri/target/release/mini-browser`

### 单独运行前端（无 Tauri）

```bash
npm run dev
```

仅启动 Vite 开发服务器，不启动 Tauri 窗口。用于调试 React UI 布局。

### 运行测试

```bash
npm run test:unit      # 单元测试（Vitest）
npm run test:e2e       # E2E 测试（Playwright）
npm run test:performance # 性能测试
npm run test           # 全部测试
```

## 2. 打包应用

`npm run tauri build` 会根据当前操作系统自动生成对应平台的安装包。

### macOS

产物路径：
```
src-tauri/target/release/bundle/macos/mini-browser.app
src-tauri/target/release/bundle/dmg/mini-browser_1.0.0_x64.dmg
```

- `.app` — 可直接拖到 Applications 文件夹运行
- `.dmg` — 磁盘映像安装包，分发给用户

### Windows

产物路径：
```
src-tauri/target/release/bundle/msi/mini-browser_1.0.0_x64.msi
src-tauri/target/release/bundle/nsis/mini-browser_1.0.0_x64-setup.exe
```

- `.msi` — Windows Installer 包
- `.exe` — NSIS 安装程序

注意：Windows 打包**必须在 Windows 上执行**（交叉编译不支持）。

### Linux

产物路径：
```
src-tauri/target/release/bundle/deb/mini-browser_1.0.0_amd64.deb
src-tauri/target/release/bundle/appimage/mini-browser_1.0.0_amd64.AppImage
```

- `.deb` — Debian/Ubuntu 安装包
- `.AppImage` — 便携式应用镜像

注意：Linux 打包**必须在 Linux 上执行**。

### 关于图标

图标源文件及生成脚本位于 `src-tauri/icons/`：

```bash
# 修改 SVG 源文件后重新生成所有平台图标
cd src-tauri/icons
python3 generate_icons.py
```

## 常见问题

- **构建很慢** — Rust 全量编译需要 3-8 分钟。开发时用 `npm run tauri dev`（增量编译更快）。修改 `tauri.conf.json` 会触发全量重编。
- **某些网站打不开** — 网站设置了 X-Frame-Options 时，但系统原生 WebView 可通过正常导航绕过。
- **国内网络慢** — 设置 npm 淘宝镜像：`npm config set registry https://registry.npmmirror.com`
