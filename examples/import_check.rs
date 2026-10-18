//! Headless Obsidian/Markdown import check: scan a vault and commit it into a
//! store, printing counts. Usage:
//!   cargo run --example import_check -- <vault_dir> <data_dir>
use notes::import_md;
use notes::store::Store;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let vault = args.next().expect("usage: import_check <vault_dir> <data_dir>");
    let data_dir = args.next().expect("usage: import_check <vault_dir> <data_dir>");

    let store = Store::new(data_dir.into())?;
    let report = import_md::scan_vault(std::path::Path::new(&vault))?;
    let existing = store.load_all()?;
    let (added, updated, failed) = report.commit(&store, &existing);
    println!(
        "scanned {} notes -> added {added}, updated {updated}, failed {failed}",
        report.imported.len()
    );
    let all = store.load_all()?;
    println!("store now holds {} notes across {} folders", all.len(), store.folders()?.len());
    Ok(())
}
