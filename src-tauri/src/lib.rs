// Tusk — 应用入口
// 仅保留 tauri 启动与 command 注册；实现按 模型/状态/驱动 拆分在子模块
mod db;
mod models;
mod state;

use std::collections::HashMap;

use tokio::sync::Mutex;

use db::pg::*;
use state::AppState;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            conns: Mutex::new(HashMap::new()),
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            query,
            list_databases,
            list_tables,
            list_columns,
            paginate_table,
            save_connection,
            list_connections,
            delete_connection,
            connect_saved,
            update_cell,
            delete_row,
            insert_row,
            export_csv,
            explain_query,
            create_table,
            drop_table,
            alter_table,
            duplicate_table,
            insert_row_vals,
            export_sql,
            write_text_file,
            create_view,
            drop_view,
            list_indexes,
            compare_schemas,
            execute_sql,
            sync_data,
            open_url,
            check_update,
            download_update,
            install_update,
            get_download_dir,
            create_database,
            drop_database,
            import_csv
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
