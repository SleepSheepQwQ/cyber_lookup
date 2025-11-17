import os
import json
import sys
import argparse
from os import DirEntry 
from typing import Dict, Any

# 常见文本文件扩展名列表
TEXT_EXTENSIONS = ('.txt', '.md', '.py', '.html', '.css', '.js', 
                   '.json', '.csv', '.xml', '.yaml', '.yml', '.java', 
                   '.c', '.cpp', '.h', '.sh', '.log', '.gitignore', '.cfg', '.ini',
                   'Dockerfile', 'LICENSE')

# 新增：用于跟踪当前递归深度
MAX_DEPTH = 0 

def get_dir_structure_and_preview(root_dir: str, max_chars: int = 100, ignore_hidden: bool = True) -> Dict[str, Any]:
    """
    递归遍历指定目录，获取其结构和文本文件的前N个字符。
    """
    
    global MAX_DEPTH
    MAX_DEPTH = 0
    
    # 路径标准化
    absolute_root_dir = os.path.abspath(root_dir)
    if not os.path.isdir(absolute_root_dir):
        print(f"Error: Directory not found at '{root_dir}' -> '{absolute_root_dir}'", file=sys.stderr)
        return {}

    def scan_directory(current_path: str, is_root: bool, depth: int) -> Dict[str, Any]:
        """递归扫描函数：新增 depth 参数用于实时追踪"""
        
        global MAX_DEPTH
        MAX_DEPTH = max(MAX_DEPTH, depth)
        
        # 实时打印：显示当前正在处理的目录和递归深度
        indent = '  ' * depth
        print(f"{indent}--> Entering: {os.path.basename(current_path)} (Depth: {depth})", file=sys.stderr)
        
        # 确定当前节点的名称 (逻辑不变)
        if is_root:
            node_name = os.path.basename(absolute_root_dir.rstrip(os.path.sep)) or absolute_root_dir
        else:
            node_name = os.path.basename(current_path)

        node_data = {
            "name": node_name,
            "type": "directory",
            "children": []
        }

        try:
            with os.scandir(current_path) as entries:
                for entry in entries:
                    
                    if ignore_hidden and entry.name.startswith('.'):
                        continue
                    
                    full_path = entry.path

                    # 目录递归处理
                    if entry.is_dir(follow_symlinks=False):
                        # 递归调用，并增加深度
                        child_node = scan_directory(full_path, is_root=False, depth=depth + 1)
                        node_data["children"].append(child_node)
                        
                    # 文件处理 (略) ...
                    elif entry.is_file(follow_symlinks=False):
                        
                        try:
                            stat_info = entry.stat()
                            file_size = stat_info.st_size
                        except Exception:
                            file_size = -1
                            
                        file_info = {
                            "name": entry.name,
                            "type": "file",
                            "size_bytes": file_size,
                            "preview": None
                        }
                        
                        is_text_candidate = entry.name.lower().endswith(TEXT_EXTENSIONS)

                        if is_text_candidate and file_size > 0:
                            try:
                                with open(full_path, 'r', encoding='utf-8') as f:
                                    content = f.read(max_chars + 1)
                                    preview_text = content[:max_chars]
                                    
                                    if len(content) > max_chars:
                                        preview_text += "..."
                                        
                                    file_info["preview"] = preview_text
                                    
                            except UnicodeDecodeError:
                                file_info["preview"] = f"[Binary or non-UTF-8 file, size: {file_size} bytes]"
                            except Exception as e:
                                file_info["preview"] = f"[Error reading file: {str(e)}]"
                                
                        else:
                            file_info["preview"] = f"[Non-text/Binary file, size: {file_size} bytes]"
                            
                        node_data["children"].append(file_info)

        except PermissionError:
            node_data["error"] = "Permission denied to access this directory. (无法访问)"
            print(f"{indent}<-- Error: Permission denied in {os.path.basename(current_path)}", file=sys.stderr)
        except Exception as e:
            node_data["error"] = f"An unexpected error occurred: {str(e)}"
            print(f"{indent}<-- Error: Unexpected error in {os.path.basename(current_path)}", file=sys.stderr)

        print(f"{indent}<-- Exiting: {os.path.basename(current_path)} (Depth: {depth})", file=sys.stderr)
        return node_data

    print(f"🔍 开始扫描目录: {absolute_root_dir}", file=sys.stderr)
    result = scan_directory(absolute_root_dir, is_root=True, depth=0)
    print(f"\n✨ 扫描完成，最大递归深度达到 {MAX_DEPTH} 层。", file=sys.stderr)
    return result

# --- 终端运行部分 ---

if __name__ == "__main__":
    
    parser = argparse.ArgumentParser(
        description="递归扫描指定目录结构并预览文本文件内容。",
        epilog="示例：\n  python sc_debug.py --include-hidden  # 深度扫描，包含隐藏目录\n  python sc_debug.py ../my_code"
    )
    
    parser.add_argument("path", type=str, nargs='?', default='.', 
                        help="要扫描的目录路径。如果省略，默认扫描当前目录 ('.')。")
    parser.add_argument("--max-chars", type=int, default=100, help="文本预览的最大字符数 (默认: 100)")
    parser.add_argument("--include-hidden", action="store_true", help="包含以点开头的隐藏文件和目录 (默认: 忽略)")
    
    args = parser.parse_args()

    # 调用函数获取结构
    structure_data = get_dir_structure_and_preview(
        root_dir=args.path, 
        max_chars=args.max_chars, 
        ignore_hidden=not args.include_hidden
    )

    # ... (输出JSON的逻辑不变，此处省略以聚焦核心问题)
    if structure_data:
        try:
            abs_target_dir = os.path.abspath(args.path)
            base_name = os.path.basename(abs_target_dir.rstrip(os.path.sep)) or "root"
            output_filename = f"{base_name}_structure.json"
            
            with open(output_filename, 'w', encoding='utf-8') as f:
                json.dump(structure_data, f, indent=4, ensure_ascii=False)
                
            print(f"\n✅ 结构化数据已保存到 '{output_filename}' 文件中。", file=sys.stderr)
            
        except Exception as e:
            print(f"\n❌ 写入 JSON 文件时发生错误: {str(e)}", file=sys.stderr)
            print("\n--- 原始结构化数据 (打印根节点) ---", file=sys.stderr)
            print(json.dumps(structure_data, indent=4, ensure_ascii=False)[:500] + "...", file=sys.stderr)
