// Read-API load harness. Hammers each endpoint scenario with N concurrent
// workers for D seconds and reports p50/p90/p95/p99/max + rps + non-200s —
// the same shape as metadao's load-test doc, pointed at the read path.
//
//   cargo run --release --bin loadtest -- \
//     --url http://127.0.0.1:8081 --concurrency 64 --duration 10 [--mint <MINT>] [--ws]
//
// Preview gates (scratchpad-003): list/detail p99 < 250ms @ 64 conc;
// candles p99 < 500ms; zero 5xx.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;

struct Args {
    url: String,
    concurrency: usize,
    duration: u64,
    mint: Option<String>,
    ws: bool,
    ws_only: bool,
}

fn parse_args() -> Args {
    let mut a = Args {
        url: "http://127.0.0.1:8081".into(),
        concurrency: 64,
        duration: 10,
        mint: None,
        ws: false,
        ws_only: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--url" => a.url = it.next().expect("--url value"),
            "--concurrency" => a.concurrency = it.next().expect("value").parse().expect("usize"),
            "--duration" => a.duration = it.next().expect("value").parse().expect("u64"),
            "--mint" => a.mint = Some(it.next().expect("--mint value")),
            "--ws" => a.ws = true,
            "--ws-only" => { a.ws = true; a.ws_only = true; }
            other => panic!("unknown flag {other}"),
        }
    }
    a
}

#[derive(Default)]
struct Tally {
    latencies_us: Vec<u64>,
    non_200: u64,
    fivexx: u64,
}

fn pct(sorted: &[u64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx] as f64 / 1000.0
}

async fn run_scenario(
    client: &reqwest::Client,
    name: &str,
    url: String,
    concurrency: usize,
    duration: Duration,
) -> Tally {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, u16)>();

    let mut handles = Vec::new();
    for _ in 0..concurrency {
        let client = client.clone();
        let url = url.clone();
        let stop = stop.clone();
        let tx = tx.clone();
        handles.push(tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let t0 = Instant::now();
                let status = match client.get(&url).send().await {
                    Ok(r) => r.status().as_u16(),
                    Err(_) => 599,
                };
                let us = t0.elapsed().as_micros() as u64;
                let _ = tx.send((us, status));
            }
        }));
    }
    drop(tx);

    let stopper = stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(duration).await;
        stopper.store(true, Ordering::Relaxed);
    });

    let mut tally = Tally::default();
    while let Some((us, status)) = rx.recv().await {
        tally.latencies_us.push(us);
        if status != 200 {
            tally.non_200 += 1;
            if status >= 500 {
                tally.fivexx += 1;
            }
        }
    }
    for h in handles {
        let _ = h.await;
    }
    tally.latencies_us.sort_unstable();
    eprintln!("  {name}: {} reqs collected", tally.latencies_us.len());
    tally
}

// WS fan-out: K clients join one mint room; the seeder (loadseed --notify)
// embeds a nanosecond send-timestamp in each memo_text; delivery latency =
// recv_time − embedded. Run `loadseed --notify --rate ...` concurrently.
async fn run_ws(url: &str, mint: &str, clients: usize, duration: Duration) -> anyhow::Result<()> {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let ws_url = url.replace("http://", "ws://").replace("https://", "wss://") + "/events";
    let received = Arc::new(AtomicU64::new(0));
    let lat_tx = Arc::new(tokio::sync::Mutex::new(Vec::<u64>::new()));
    let mut handles = Vec::new();

    for _ in 0..clients {
        let ws_url = ws_url.clone();
        let mint = mint.to_string();
        let received = received.clone();
        let lat_tx = lat_tx.clone();
        handles.push(tokio::spawn(async move {
            let (mut ws, _) = match tokio_tungstenite::connect_async(&ws_url).await {
                Ok(ok) => ok,
                Err(e) => {
                    eprintln!("ws connect failed: {e}");
                    return;
                }
            };
            let sub = format!("{{\"subscribe\":{{\"market\":\"{mint}\"}}}}");
            let _ = ws.send(Message::Text(sub)).await;
            let deadline = Instant::now() + duration + Duration::from_secs(2);
            while Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_secs(1), ws.next()).await {
                    Ok(Some(Ok(Message::Text(text)))) => {
                        received.fetch_add(1, Ordering::Relaxed);
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(memo) = v.get("memo_text").and_then(|m| m.as_str()) {
                                if let Ok(sent_ns) = memo.parse::<u128>() {
                                    let now_ns = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap()
                                        .as_nanos();
                                    let us = ((now_ns.saturating_sub(sent_ns)) / 1000) as u64;
                                    lat_tx.lock().await.push(us);
                                }
                            }
                        }
                    }
                    Ok(Some(Ok(_))) => {}
                    Ok(Some(Err(_))) | Ok(None) => break,
                    Err(_) => {} // poll timeout, keep waiting
                }
            }
        }));
    }
    for h in handles {
        let _ = h.await;
    }
    let mut lats = lat_tx.lock().await.clone();
    lats.sort_unstable();
    println!(
        "ws fan-out: clients={clients} frames={} | delivery p50={:.1}ms p95={:.1}ms p99={:.1}ms max={:.1}ms",
        received.load(Ordering::Relaxed),
        pct(&lats, 0.50),
        pct(&lats, 0.95),
        pct(&lats, 0.99),
        pct(&lats, 1.0),
    );
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args();
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(args.concurrency)
        .timeout(Duration::from_secs(10))
        .build()?;

    // Pick a target mint (busiest market) unless given.
    let mint = match &args.mint {
        Some(m) => m.clone(),
        None => {
            let v: serde_json::Value = client
                .get(format!("{}/api/markets?limit=1", args.url))
                .send()
                .await
                .context("fetch markets for target mint — is the api up + seeded?")?
                .json()
                .await?;
            v.as_array()
                .and_then(|a| a.first())
                .and_then(|m| m.get("mint"))
                .and_then(|m| m.as_str())
                .context("no markets in DB — run loadseed first")?
                .to_string()
        }
    };
    eprintln!("target mint: {mint}");

    let scenarios = vec![
        ("markets_list", format!("{}/api/markets?limit=50", args.url)),
        ("market_detail", format!("{}/api/markets/{mint}", args.url)),
        ("trades_page", format!("{}/api/trades?mint={mint}&limit=100", args.url)),
        ("candles_1m", format!("{}/api/candles?mint={mint}&interval=1m", args.url)),
        ("positions", format!("{}/api/positions?mint={mint}&side=short", args.url)),
        ("messages", format!("{}/api/messages?mint={mint}&limit=50", args.url)),
    ];

    let duration = Duration::from_secs(args.duration);
    if !args.ws_only {
    println!(
        "| endpoint | reqs | rps | p50 ms | p90 ms | p95 ms | p99 ms | max ms | non-200 | 5xx |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for (name, url) in scenarios {
        let t = run_scenario(&client, name, url, args.concurrency, duration).await;
        let n = t.latencies_us.len();
        println!(
            "| {name} | {n} | {:.0} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {} | {} |",
            n as f64 / args.duration as f64,
            pct(&t.latencies_us, 0.50),
            pct(&t.latencies_us, 0.90),
            pct(&t.latencies_us, 0.95),
            pct(&t.latencies_us, 0.99),
            pct(&t.latencies_us, 1.0),
            t.non_200,
            t.fivexx,
        );
    }

    }
    if args.ws {
        run_ws(&args.url, &mint, args.concurrency, duration).await?;
    }
    Ok(())
}
