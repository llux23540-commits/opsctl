//! 一次性诊断:用与 opsctl-server 完全相同的 reqwest 客户端,从一个**不同路径的
//! exe**(target/debug/examples/probe.exe)访问同一个 URL。
//!
//! 若这里能通、而 opsctl-server.exe 报 os error 10013,即可证明拦截是按程序路径生效的
//! 防火墙规则,与网络/代码无关。
//!
//!   cargo run -p opsctl-server --example probe -- http://host:port/path
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://16.162.107.45:30848/nacos/v1/console/health/readiness".into());
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .build()
        .unwrap();
    let started = Instant::now();
    match client.get(&url).send().await {
        Ok(r) => {
            let code = r.status().as_u16();
            let body = r.text().await.unwrap_or_default();
            println!(
                "OK  {} {}ms  body={:?}",
                code,
                started.elapsed().as_millis(),
                body.chars().take(60).collect::<String>()
            );
        }
        Err(e) => {
            let mut msg = e.to_string();
            let mut cur = std::error::Error::source(&e);
            while let Some(s) = cur {
                msg.push_str(" ← ");
                msg.push_str(&s.to_string());
                cur = s.source();
            }
            println!("ERR {}ms  {msg}", started.elapsed().as_millis());
        }
    }
}
