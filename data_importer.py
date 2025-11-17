# data_importer.py
# 运行此脚本以解析 TXT 文件并将数据导入 SQLite 数据库 (uid_phone_map.db)
# 注意：此脚本需要在您的 Termux 环境中运行。

import sqlite3
import os
import re
import glob
import sys

# --- 配置 ---
DATABASE_FILE = "uid_phone_map.db"
DATA_DIR = "/storage/emulated/0/脚本的备份"
# --- 数据库操作 ---

def setup_database(conn: sqlite3.Connection):
    cursor = conn.cursor()
    cursor.execute("""
        CREATE TABLE IF NOT EXISTS user_mapping (
            uid TEXT PRIMARY KEY NOT NULL,
            phone_number TEXT NOT NULL
        );
    """)
    cursor.execute("""
        CREATE INDEX IF NOT EXISTS idx_phone_number ON user_mapping (phone_number);
    """)
    conn.commit()
    print("✅ Database schema created successfully.")

def parse_and_import_data():
    print(f"🚀 Connecting to database: {DATABASE_FILE}")
    conn = None
    try:
        conn = sqlite3.connect(DATABASE_FILE)
        setup_database(conn)
        
        all_files = glob.glob(os.path.join(DATA_DIR, 'qb*.txt'))
        if not all_files:
            print(f"⚠️ No 'qb*.txt' files found in {DATA_DIR}. Database will be empty.")
            # 尝试从当前目录查找，以防路径设置错误
            all_files = glob.glob('qb*.txt')
            if not all_files:
                print("⚠️ Checked current directory too, still no data files found.")
                return

        total_records = 0
        batch_data = []
        batch_size = 50000 
        
        for file_path in all_files:
            print(f"⏳ Processing file: {os.path.basename(file_path)}...")
            
            # 使用 errors='ignore' 避免编码问题导致文件读取中断
            with open(file_path, 'r', encoding='utf-8', errors='ignore') as f:
                content = f.read()
                # 匹配 <PHONE>----<UID>
                records = re.findall(r'(\d+)----(\d+)', content)
                
                for phone, uid in records:
                    # 关键：反转键值对 (UID, Phone)
                    batch_data.append((uid, phone)) 
                    total_records += 1
                    
                    if len(batch_data) >= batch_size:
                        conn.executemany("INSERT OR REPLACE INTO user_mapping (uid, phone_number) VALUES (?, ?)", batch_data)
                        conn.commit()
                        print(f"   > Imported {total_records} records so far...")
                        batch_data = []

        if batch_data:
            conn.executemany("INSERT OR REPLACE INTO user_mapping (uid, phone_number) VALUES (?, ?)", batch_data)
            conn.commit()

        print(f"🎉 Import complete! Total records imported: {total_records}")

    except sqlite3.Error as e:
        print(f"❌ SQLite Error: {e}")
        sys.exit(1)
    except FileNotFoundError:
        print(f"❌ Directory not found: {DATA_DIR}")
        sys.exit(1)
    finally:
        if conn:
            conn.close()

if __name__ == "__main__":
    parse_and_import_data()