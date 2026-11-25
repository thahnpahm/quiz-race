// Nhúng icon và thông tin phiên bản vào file .exe trên Windows (Explorer, thanh tác vụ, lối tắt).
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icon.ico").set("ProductName", "Quiz Race").set("FileDescription", "Quiz Race – Midterm Showdown");
        res.compile().expect("không nhúng được icon vào exe");
    }
    println!("cargo:rerun-if-changed=icon.ico");
}
