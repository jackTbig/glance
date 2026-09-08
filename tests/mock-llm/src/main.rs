//! 命令行起假服务：mock-llm --port 18080
fn main() {
    let mut port: u16 = 18080;
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--port") {
        port = args.get(i + 1).and_then(|p| p.parse().ok()).unwrap_or(port);
    }
    let s = mock_llm::spawn(port);
    println!("listening on {}", s.base_url);
    loop {
        std::thread::park();
    }
}
