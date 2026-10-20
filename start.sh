#!/usr/bin/env bash

# ==============================================================================
# Bento 启动脚本 (macOS Desktop Launcher)
# ==============================================================================

set -e

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DIR"

echo "🚀 ====================================================="
echo "          Bento 项目聚合管理 macOS 桌面应用            "
echo "========================================================"

# 1. 检查基础运行环境
echo "🔍 检查运行环境..."
if ! command -v node >/dev/null 2>&1; then
  echo "❌ 未检测到 Node.js，请先安装 Node.js (推荐 v18+)" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "❌ 未检测到 Rust/Cargo，Tauri 桌面应用需要 Rust 环境。" >&2
  echo "   请访问 https://rustup.rs 安装 Rust。" >&2
  exit 1
fi

# 包管理器检测
PACKAGE_MANAGER="pnpm"
if ! command -v pnpm >/dev/null 2>&1; then
  if command -v npm >/dev/null 2>&1; then
    PACKAGE_MANAGER="npm"
  else
    echo "❌ 未检测到 pnpm 或 npm，请先安装包管理器。" >&2
    exit 1
  fi
fi

# 2. 检查依赖安装
if [ ! -d "node_modules" ]; then
  echo "📦 正在安装依赖 ($PACKAGE_MANAGER install)..."
  $PACKAGE_MANAGER install
fi

# 3. 初始化 SQLite 数据库
echo "🗄️ 检查本地 SQLite 数据库 (~/.apphub/apphub.db)..."
if command -v python3 >/dev/null 2>&1; then
  python3 "$DIR/scripts/manage_project.py" list
fi

# 4. 解析启动参数
MODE="${1:-dev}"

case "$MODE" in
  dev|--dev|-d)
    echo "✨ 启动 Bento macOS 桌面应用开发模式 (Tauri dev)..."
    if [ "$PACKAGE_MANAGER" = "pnpm" ]; then
      pnpm tauri dev
    else
      npm run tauri dev
    fi
    ;;

  web|--web|-w)
    echo "🌐 仅启动 Web 前端界面 (http://localhost:1420)..."
    if [ "$PACKAGE_MANAGER" = "pnpm" ]; then
      pnpm dev
    else
      npm run dev
    fi
    ;;

  build|--build|-b)
    echo "🔨 正在构建 Bento 本地测试应用 (.app)，不用于分发..."
    if [ "$PACKAGE_MANAGER" = "pnpm" ]; then
      pnpm tauri build --config '{"bundle":{"createUpdaterArtifacts":false}}'
    else
      npm run tauri -- build --config '{"bundle":{"createUpdaterArtifacts":false}}'
    fi
    echo "🎉 本地测试构建完成！产物路径: src-tauri/target/release/bundle/macos/Bento.app"
    ;;

  help|--help|-h)
    echo "使用说明:"
    echo "  ./start.sh          # 默认启动桌面应用开发模式 (Tauri Dev)"
    echo "  ./start.sh web      # 仅启动 Web 前端调试界面"
    echo "  ./start.sh build    # 构建本地测试应用 (.app)，不用于分发"
    echo "  ./start.sh help     # 显示此帮助信息"
    ;;

  *)
    echo "⚠️ 未知模式: $MODE，默认以开发模式启动..."
    if [ "$PACKAGE_MANAGER" = "pnpm" ]; then
      pnpm tauri dev
    else
      npm run tauri dev
    fi
    ;;
esac
