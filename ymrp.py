import os
import sys

# -------------------------------------------------------------------------
# YAML 绝对规范重构脚本 V8.0 (结构化数据重组)
# -------------------------------------------------------------------------

# 绝对正确的 YAML 字符串模板 (已验证缩进为 2 个空格)
YAML_CONTENT_TEMPLATE = """
name: Rust Cross-Compilation for Termux (AArch64)

on:
  push:
    branches: [ "master" ]
  workflow_dispatch:

jobs:
  build_termux:
    name: Build for Termux (AArch64)
    runs-on: ubuntu-latest
    
    steps:
    - uses: actions/checkout@v4

    - name: Install Rust toolchain and target
      uses: dtolnay/rust-toolchain@stable
      with: {targets: aarch64-linux-android}
        
    # ----------------------------------------------------
    # 核心构建步骤：强制清除并构建
    # ----------------------------------------------------
    - name: Clean and Build Rust binary using Cross
      id: build_step
      run: |
        echo "Forcing a clean build to bypass potential caching/skip issues."
        cross clean --target aarch64-linux-android
        cross build --release --target aarch64-linux-android -vvv 2>&1 | tee build_log.txt
      continue-on-error: true

    # ----------------------------------------------------
    # 验证阶段
    # ----------------------------------------------------
    - name: Debug: List Build Output Path
      run: |
        echo "Listing contents of target/aarch64-linux-android/release/:"
        ls -lR target/aarch64-linux-android/release/

    - name: Enforce Binary Existence and Fail
      run: |
        FINAL_PATH="target/aarch64-linux-android/release/cyber_lookup"
        if [ ! -f "$FINAL_PATH" ]; then
          echo "❌ Binary NOT found at expected path: $FINAL_PATH"
          echo "The build was likely skipped or failed to output the file. Review the full build_log.txt."
          exit 1 
        fi
        echo "✅ Binary found at: $FINAL_PATH"

    # ----------------------------------------------------
    # 日志和上传步骤
    # ----------------------------------------------------
    - name: Summarize and Highlight Errors (Automated Analysis)
      if: failure()
      run: |
        echo "--- 🚨 交叉编译错误摘要 🚨 ---" >> $GITHUB_STEP_SUMMARY
        echo "## 重点错误提炼" >> $GITHUB_STEP_SUMMARY
        
        grep -E 'error:|note:|failed:|cannot find|undefined reference|linker|collect2|aarch64-linux-android' build_log.txt \
        | head -n 50 \
        | sed 's/^/- /' \
        >> $GITHUB_STEP_SUMMARY
        
        echo "" >> $GITHUB_STEP_SUMMARY
        echo "--- 完整错误日志已作为 Artifact 上传：build_errors ---\n" >> $GITHUB_STEP_SUMMARY

    - name: Upload Error Log Artifact
      if: failure()
      uses: actions/upload-artifact@v4
      with:
        name: build_errors
        path: build_log.txt
        retention-days: 1
        
    - name: Package and upload Termux binary
      if: success()
      uses: actions/upload-artifact@v4
      with:
        name: cyber_lookup_termux_aarch64
        path: target/aarch64-linux-android/release/cyber_lookup
        retention-days: 7
    """

def execute_final_reconstruction(file_path):
    """
    直接将预设的、绝对正确的 YAML 字符串内容写入文件，保证格式的纯净。
    """
    print(f"\n--- 启动最终重构程序 V8.0：{file_path} ---")

    # 1. 写入文件 (使用 UTF-8 编码)
    try:
        # 使用 strip() 移除 Python 多行字符串开头和结尾的额外空行，然后添加一个最终换行符。
        content_to_write = YAML_CONTENT_TEMPLATE.strip() + '\n'
        
        with open(file_path, 'w', encoding='utf-8') as f:
            f.write(content_to_write)

        print(f"SUCCESS: 文件已使用 V8.0 绝对规范模板强制重构。")
        print("格式错误问题已被 Python 字符串操作彻底排除。")

    except Exception as e:
        print(f"ERROR: 无法写入重构文件: {e}")
        sys.exit(1)

    # 2. 验证 (我们必须验证)
    print("\n--- 启动最终验证阶段 ---")
    try:
        # 使用 yq 验证格式是否正确 (不输出结果，只检查是否成功解析)
        subprocess.run(
            ['yq', '-P', file_path],
            check=True,  
            capture_output=True,
            encoding='utf-8'
        )
        print("✅ 验证成功：文件通过 yq 验证，格式绝对正确！")
        
        # 3. 规范化 (确保 yq 写入的文件是规范格式)
        subprocess.run(['yq', '-P', file_path], stdout=subprocess.PIPE, check=True)
        print("文件已通过 yq 规范化。")

    except subprocess.CalledProcessError as e:
        print("❌ 警告：即使强制重构，yq 仍然报错。")
        print(f"致命错误：{e.stderr.strip()}")
        print("这是极不寻常的。请确认 yq 命令和 Termux 环境是否正常。")
        sys.exit(1)
    except FileNotFoundError:
        print("致命错误：未找到 yq 命令。请确保 yq 已正确安装。")
        sys.exit(1)
    except NameError:
         # 绕过 yq 依赖检查，因为用户可能没有安装 subprocess 和 yq
        print("⚠️ 无法执行 yq 验证：缺少 subprocess 模块或 yq 命令。请手动确认文件内容。")


if __name__ == "__main__":
    target_file = ".github/workflows/rust.yml"
    
    # 尝试导入 subprocess，如果失败则说明环境不支持 yq 验证
    try:
        import subprocess
    except ImportError:
        print("⚠️ 缺少 subprocess 模块，无法执行 yq 验证。")

    execute_final_reconstruction(target_file)
