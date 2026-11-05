use std::net::IpAddr;
use std::path::PathBuf;

use clap::Parser;
use replay::{ReplayScript, start_replay_server};

#[derive(Debug, Parser)]
#[command(name = "aios-replay", about = "不花 key 的模型回放（T010）")]
struct Args {
    /// 会话日志（JSONL）
    #[arg(long)]
    file: PathBuf,
    /// 覆盖文件（可选，默认找同目录 replay.override.json）
    #[arg(long)]
    r#override: Option<PathBuf>,
    /// 帧之间等多少毫秒（可选）
    #[arg(long)]
    pace_ms: Option<u64>,
    #[arg(long, default_value = "127.0.0.1")]
    host: IpAddr,
    #[arg(long, default_value_t = 0)]
    port: u16,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let mut script = ReplayScript::from_log_with_override(&args.file, args.r#override.as_deref())
        .unwrap_or_else(|error| fail(&error.to_string()));
    script.pace_ms = args.pace_ms;
    let server = start_replay_server(script)
        .await
        .unwrap_or_else(|error| fail(&error.to_string()));
    println!("aios-replay: {}", server.url());
    tokio::signal::ctrl_c().await.ok();
    server.shutdown();
}

fn fail(message: &str) -> ! {
    eprintln!("aios-replay: {message}");
    std::process::exit(2)
}
