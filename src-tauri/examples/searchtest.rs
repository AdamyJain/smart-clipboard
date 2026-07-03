//! Debug: run the real retrieval path against the real DB.
//! Usage: cargo run --example searchtest -- "what blue color did I copy"

fn main() -> anyhow::Result<()> {
    let query = std::env::args().nth(1).unwrap_or("blue color".into());
    smart_clipboard_lib::db::register_vec_extension();
    let appdata = std::env::var("APPDATA")?;
    let data_dir = std::path::PathBuf::from(appdata).join("com.adamy.smart-clipboard");
    let conn = smart_clipboard_lib::db::open(&data_dir.join("smart-clipboard.db"))?;
    let model = smart_clipboard_lib::pipeline::embed::load_model(data_dir.join("models"))?;
    let t = std::time::Instant::now();
    let hits = smart_clipboard_lib::search::retrieval::search(&conn, &model, &query, None, 5)?;
    println!("query: {query:?}  ({:?})", t.elapsed());
    for h in hits {
        println!(
            "  [{:.3}] {:<10} {:<6} {}",
            h.score,
            h.entity_type,
            h.matched_by,
            h.raw_text.chars().take(50).collect::<String>()
        );
    }
    Ok(())
}
