//! Prints the fin-core JSON Schema to stdout.

fn main() {
    let schema = fin_core::json_schema();
    println!("{}", serde_json::to_string_pretty(&schema).expect("schema serializes"));
}
