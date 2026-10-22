fn main() {
    for p in std::env::args().skip(1) {
        let b = std::fs::read(&p).unwrap_or_default();
        match cadaddy_dwgcodec::read(&b) {
            Ok(_) => println!("ok"),
            Err(e) => println!("err {e}"),
        }
    }
}
