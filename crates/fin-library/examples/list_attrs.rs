//! Prints, for one directory, what the native source lists (plan 5D Task 3 Step 0): per
//! entry the type, attribute word, reparse tag, size and whether the placeholder rules mark
//! it, then the totals. Names are printed only with `--names` (they are user content: keep
//! the output local). Never opens an entry.
//!
//! `cargo run -p fin-library --example list_attrs -- <dir> [<raw component>...] [--names]`

use std::path::PathBuf;

use fin_library::source::NativeSource;
use fin_library::{RawRel, Source};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let names = args.iter().any(|a| a == "--names");
    args.retain(|a| a != "--names");
    let Some(root) = args.first().map(PathBuf::from) else {
        eprintln!("usage: list_attrs <dir> [<component>...] [--names]");
        std::process::exit(2);
    };
    let rel = RawRel::new(args[1..].iter().map(|c| c.as_bytes().to_vec()).collect()).expect("rel");
    let src = NativeSource::new(&root);
    let dir = src.open_dir(&rel).expect("open_dir");
    let entries = src.read_dir(&dir).expect("read_dir");
    let mut placeholders = 0;
    for e in &entries {
        let p = fin_library::placeholder::from_attrs(e.attrs);
        placeholders += usize::from(p.is_some());
        let shown = if names {
            String::from_utf8_lossy(&e.name).into_owned()
        } else {
            String::new()
        };
        println!(
            "{:?}\t{:?}\ttag={:?}\tsize={}\tplaceholder={:?}\tid={}\t{shown}",
            e.entry_type,
            e.attrs,
            e.reparse_tag.map(|t| format!("{t:#010x}")),
            e.size,
            p,
            e.file_id.is_some(),
        );
    }
    println!("{} entries, {placeholders} placeholders", entries.len());
}
