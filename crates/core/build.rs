//! 把 `assets/libchecker/rules.zip`（由 `scripts/sync-libchecker-rules.py` 从上游拉取）
//! 复制到 OUT_DIR 供 `include_bytes!` 内嵌；文件不存在时内嵌空数据，库识别退化为「未内置规则」。

use std::path::PathBuf;

fn main() {
    let src = PathBuf::from("assets/libchecker/rules.zip");
    let out =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("libchecker-rules.zip");
    println!("cargo:rerun-if-changed={}", src.display());
    match std::fs::read(&src) {
        Ok(bytes) => std::fs::write(&out, bytes).expect("write bundled rules"),
        Err(_) => {
            println!(
                "cargo:warning=未找到 LibChecker 规则包 {}，库识别将不可用；先执行 scripts/sync-libchecker-rules.py",
                src.display()
            );
            std::fs::write(&out, []).expect("write empty rules");
        }
    }
}
