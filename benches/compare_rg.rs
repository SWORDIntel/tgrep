// tgrep vs ripgrep comparison runner
// TODO: implement paired benchmark with result equality check
//
// 1. Run tgrep and rg with identical options, thread counts, output sinks
// 2. Verify result equality first
// 3. Ten alternating trials per query
// 4. Report median, slowest, source bytes read, peak RSS, index size
//
// Usage: compare_rg <pattern> <path> [trials]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <pattern> <path> [trials]", args[0]);
        std::process::exit(2);
    }
    let pattern = &args[1];
    let path = &args[2];
    let trials: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);

    eprintln!("pattern: {}", pattern);
    eprintln!("path: {}", path);
    eprintln!("trials: {}", trials);

    // TODO: implement
    todo!("benchmark runner")
}
