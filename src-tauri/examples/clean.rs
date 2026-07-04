//! Debug: delete capture rows from the real (encrypted) DB.
//! Usage:
//!   cargo run --example clean -- <capture-id> [<capture-id> ...]   delete specific captures
//!   cargo run --example clean -- --all                              delete ALL captures

fn main() -> anyhow::Result<()> {
    smart_clipboard_lib::db::register_vec_extension();
    let data_dir = dirs_path();
    let conn = smart_clipboard_lib::db::open(&data_dir.join("smart-clipboard.db"))?;

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: cargo run --example clean -- <capture-id> [<capture-id> ...]");
        eprintln!("       cargo run --example clean -- --all");
        std::process::exit(1);
    }

    if args.iter().any(|a| a == "--all") {
        let n = smart_clipboard_lib::db::delete_all_captures(&conn)?;
        println!("deleted {n} captures (all)");
        return Ok(());
    }

    for id in &args {
        smart_clipboard_lib::db::delete_capture(&conn, id)?;
        println!("deleted {id}");
    }
    Ok(())
}

fn dirs_path() -> std::path::PathBuf {
    let appdata = std::env::var("APPDATA").expect("APPDATA");
    std::path::PathBuf::from(appdata).join("com.adamy.smart-clipboard")
}
