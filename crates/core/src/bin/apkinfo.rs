//! 命令行入口：开发期对照 aapt2 用；功能随 Stage 2 填充。
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: apkinfo <file.apk>");
        std::process::exit(2);
    }
    println!("apkinfo-core {} (parser not implemented yet)", env!("CARGO_PKG_VERSION"));
}
