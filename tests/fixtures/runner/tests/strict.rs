//! A custom harness that rejects unknown arguments, as a clap-based one does.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        eprintln!("strict: unknown arguments {args:?}");
        std::process::exit(2);
    }
    println!("strict harness ran");
}
