#!/usr/bin/env python3
"""
Mini Browser 图标生成脚本
生成正确的 .ico (Windows)、.icns (macOS) 和 PNG 图标文件

用法:
    python3 generate_icons.py
"""

import os
import sys
import io
import struct
import subprocess
from pathlib import Path

SCRIPT_DIR = Path(__file__).parent
os.chdir(SCRIPT_DIR)

def ensure_pillow():
    """确保 Pillow 已安装"""
    try:
        from PIL import Image
        return Image
    except ImportError:
        print("正在安装 Pillow...")
        subprocess.check_call([sys.executable, "-m", "pip", "install", "Pillow", "-q"])
        from PIL import Image
        return Image

def get_source_image(Image):
    """获取源图像 — 优先从 SVG 生成，避免使用过时的 PNG"""
    svg_path = SCRIPT_DIR / "icon.svg"
    if svg_path.exists():
        import subprocess, io, sys
        # 尝试用 npx tauri icon 转换 SVG
        print(f"使用源图像: icon.svg")
        result = subprocess.run(
            [sys.executable, "-m", "pip", "list"],
            capture_output=True, text=True
        )
        # 回退: 用 PIL 直接打开无法处理 SVG，需要外部工具
        # 先尝试用 tauri CLI 生成
        result = subprocess.run(
            ["npx", "tauri", "icon", str(svg_path), "--output", str(SCRIPT_DIR)],
            capture_output=True, text=True,
            cwd=SCRIPT_DIR.parent.parent  # 回到项目根目录
        )
        if result.returncode == 0:
            # 从生成的 PNG 加载
            png_path = SCRIPT_DIR / "icon.png"
            if png_path.exists():
                img = Image.open(png_path)
                return img
        # fallback: 尝试其他工具...

    # 最后尝试 512x512.png
    source_path = SCRIPT_DIR / "512x512.png"
    if source_path.exists():
        print(f"使用源图像: 512x512.png")
        return Image.open(source_path)

    raise FileNotFoundError("找不到源图像，请确保存在 icon.svg 或 512x512.png")

def generate_png_icons(Image, source):
    """生成标准尺寸的 PNG 图标"""
    print("\n=== 生成 PNG 图标 ===")
    sizes = [32, 64, 128, 256, 512]
    retina_sizes = {"128x128@2x": 256}

    for size in sizes:
        output_path = SCRIPT_DIR / f"{size}x{size}.png"
        resized = source.resize((size, size), Image.LANCZOS)
        resized.save(output_path, "PNG", optimize=True)
        print(f"  ✅ {size}x{size}.png")

    for name, size in retina_sizes.items():
        output_path = SCRIPT_DIR / f"{name}.png"
        resized = source.resize((size, size), Image.LANCZOS)
        resized.save(output_path, "PNG", optimize=True)
        print(f"  ✅ {name}.png")

def generate_ico_file(Image, source):
    """生成多分辨率 ICO 文件（Windows） — 手动构造 ICO"""
    print("\n=== 生成 ICO 图标 ===")
    output_path = SCRIPT_DIR / "icon.ico"

    # Windows ICO 标准尺寸
    ico_sizes = [16, 32, 48, 64, 128, 256]

    # 将每个尺寸转换为 BMP 格式的 ICO 条目
    ico_entries = []
    for size in ico_sizes:
        resized = source.resize((size, size), Image.LANCZOS)
        # 转换为 BGRA 像素 (BITMAPV5HEADER style for ICO)
        pixels = list(resized.getdata())
        # ICO: XOR 掩码 (BGRA, bottom-up) + AND 掩码
        bmp_data = bytearray()
        # BITMAPINFOHEADER (40 bytes)
        bmp_data += struct.pack("<IiiHHIIiiII",
            40,           # biSize
            size,         # biWidth
            size * 2,     # biHeight (doubled for ICO XOR+AND)
            1,            # biPlanes
            32,           # biBitCount
            0,            # biCompression (BI_RGB)
            0,            # biSizeImage
            0, 0, 0, 0    # biXPels, biYPels, biClrUsed, biClrImportant
        )
        # XOR mask (BGRA pixels, bottom-up)
        for y in range(size - 1, -1, -1):
            for x in range(size):
                r, g, b, a = pixels[y * size + x]
                bmp_data += struct.pack("BBBB", b, g, r, a)
        # AND mask (1=transparent, 0=opaque), 32-bit aligned rows
        and_row_size = ((size + 31) // 32) * 4
        for _ in range(size):
            bmp_data += b'\x00' * and_row_size

        ico_entries.append((size, bytes(bmp_data)))

    # ICO header + directory entries + image data
    header = struct.pack("<HHH", 0, 1, len(ico_sizes))
    data_offset = 6 + 16 * len(ico_sizes)
    directory = b""
    image_data = b""

    for size, data in ico_entries:
        w = 0 if size >= 256 else size
        h = 0 if size >= 256 else size
        directory += struct.pack(
            "<BBBBHHII",
            w, h, 0, 0, 1, 32, len(data), data_offset
        )
        image_data += data
        data_offset += len(data)

    with open(output_path, "wb") as f:
        f.write(header + directory + image_data)

    print(f"  ✅ icon.ico (多分辨率: {ico_sizes})")

def generate_icns_file(Image, source):
    """生成 ICNS 文件（macOS）"""
    print("\n=== 生成 ICNS 图标 ===")
    output_path = SCRIPT_DIR / "icon.icns"

    # 检查 macOS 原生工具
    has_sips = subprocess.run(["which", "sips"], capture_output=True).returncode == 0
    has_iconutil = subprocess.run(["which", "iconutil"], capture_output=True).returncode == 0

    if has_sips:
        # 方法1: 使用 sips (最简单)
        temp_png = SCRIPT_DIR / "_temp_icon.png"
        source.save(temp_png, "PNG")
        result = subprocess.run(
            ["sips", "-i", str(temp_png), "--out", str(output_path)],
            capture_output=True, text=True
        )
        temp_png.unlink(missing_ok=True)
        if result.returncode == 0 and output_path.exists():
            print(f"  ✅ icon.icns (使用 sips)")
            return

    if has_iconutil:
        # 方法2: 使用 iconutil (需要 .iconset 目录)
        import shutil
        iconset_dir = SCRIPT_DIR / "_temp.iconset"
        if iconset_dir.exists():
            shutil.rmtree(iconset_dir)
        iconset_dir.mkdir()

        # 创建 iconset 结构
        mappings = [
            ("icon_16x16.png", 16),
            ("icon_32x32.png", 32),
            ("icon_32x32@2x.png", 64),
            ("icon_128x128.png", 128),
            ("icon_256x256.png", 256),
            ("icon_512x512.png", 512),
            ("icon_512x512@2x.png", 1024),
        ]

        for fname, size in mappings:
            img = source.resize((size, size), Image.LANCZOS)
            img.save(iconset_dir / fname, "PNG")

        result = subprocess.run(
            ["iconutil", "-c", "icns", str(iconset_dir), "-o", str(output_path)],
            capture_output=True, text=True
        )
        shutil.rmtree(iconset_dir, ignore_errors=True)

        if result.returncode == 0 and output_path.exists():
            print(f"  ✅ icon.icns (使用 iconutil)")
            return

    # 方法3: 使用 PIL 直接创建 ICNS
    try:
        icns_sizes = [32, 128, 256, 512]
        images = [source.resize((s, s), Image.LANCZOS) for s in icns_sizes]

        images[0].save(
            str(output_path),
            format="ICNS",
            save_all=True,
            append_images=images[1:],
        )
        print(f"  ✅ icon.icns (使用 PIL)")
    except Exception as e:
        print(f"  ❌ PIL ICNS 失败: {e}")
        raise RuntimeError("无法生成 ICNS，请手动使用 sips 或 iconutil")

def verify_icons():
    """验证所有图标"""
    from PIL import Image

    print("\n=== 验证图标 ===")
    checks = [
        ("32x32.png", (32, 32)),
        ("64x64.png", (64, 64)),
        ("128x128.png", (128, 128)),
        ("256x256.png", (256, 256)),
        ("512x512.png", (512, 512)),
        ("icon.ico", None),
        ("icon.icns", None),
    ]

    all_ok = True
    for fname, expected in checks:
        fpath = SCRIPT_DIR / fname
        if not fpath.exists():
            print(f"  ❌ {fname}: 文件缺失")
            all_ok = False
            continue

        try:
            with Image.open(fpath) as img:
                if expected and img.size != expected:
                    print(f"  ⚠️  {fname}: 尺寸 {img.size} != 预期 {expected}")
                else:
                    fmt = img.format or "未知"
                    print(f"  ✅ {fname}: {img.size[0]}x{img.size[1]} ({fmt})")
        except Exception as e:
            print(f"  ❌ {fname}: {e}")
            all_ok = False

    return all_ok

def main():
    print("=" * 50)
    print("  Mini Browser 图标生成脚本")
    print("=" * 50)

    # 确保依赖
    Image = ensure_pillow()

    # 获取源图像
    source = get_source_image(Image)

    # 生成所有图标
    generate_png_icons(Image, source)
    generate_ico_file(Image, source)
    generate_icns_file(Image, source)

    # 验证
    if verify_icons():
        print("\n✅ 所有图标生成成功！")
        print("\n配置检查 (tauri.conf.json):")
        print('  "bundle": {')
        print('    "icon": [')
        print('      "icons/32x32.png",')
        print('      "icons/128x128.png",')
        print('      "icons/128x128@2x.png",')
        print('      "icons/icon.icns",')
        print('      "icons/icon.ico"')
        print('    ]')
        print('  }')
        print("\n现在可以运行: cargo tauri build")
    else:
        print("\n⚠️  部分图标验证失败")
        sys.exit(1)

if __name__ == "__main__":
    main()
