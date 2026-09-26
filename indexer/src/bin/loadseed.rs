// Load-test seeder: drives synthetic markets/trades/positions/messages through
// the REAL writer path (translate → upsert → reconcile), so the API serves the
// exact row shapes production would.
//
//   loadseed --markets 50 --trades 2000 --positions 50
//     bulk seed (backfill semantics, no checkpoint, no WS frames — the rooms
//     fill only from the live writer task since ws-to-indexer)
//
// DATABASE_URL from env/.env (use the WRITER role — this tool IS a writer).

use anyhow::Context;
use chrono::Utc;
use torch_indexer::contracts::{
    AnyEvent, BondingCurveTrade, DecodedEvent, MarketCreated, OpenShortEvent, TorchEvent,
};
use torch_indexer::stream::writer::write_events_no_checkpoint;

fn pk(i: u64) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[..8].copy_from_slice(&i.to_le_bytes());
    b[31] = 7; // keep distinct from all-zero default sentinel
    b
}

fn run_nonce() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn de(event: AnyEvent, slot: i64, tx_idx: i32, inner: i32, memo: Option<String>) -> DecodedEvent {
    // Nonce the signature: identical synthetic sigs across runs hit the
    // UNIQUE(signature, inner_ix_idx) idempotency key → zero inserts → zero
    // notifies (a silent no-op second run).
    DecodedEvent {
        signature: format!("loadseed_{}_{slot}_{tx_idx}_{inner}", run_nonce()),
        tx_idx,
        inner_ix_idx: inner,
        slot,
        block_time: Some(Utc::now()),
        event,
        memo,
        via_vault: false,
    }
}

fn market(i: u64) -> AnyEvent {
    AnyEvent::Torch(TorchEvent::MarketCreated(MarketCreated {
        mint: pk(1_000_000 + i),
        creator: pk(2_000_000 + i),
        name: format!("Load Market {i}"),
        symbol: format!("LOAD{i}"),
        metadata_uri: String::new(),
        is_community_token: false,
        sol_target: 100_000_000_000,
        virtual_sol_reserves: 37_500_000_000,
        virtual_token_reserves: 1_073_000_000_000_000,
    }))
}

fn trade(mint_i: u64, n: u64, memo: Option<&str>) -> (AnyEvent, Option<String>) {
    let sol_in = 50_000_000 + (n % 97) * 7_919_111;
    (
        AnyEvent::Torch(TorchEvent::BondingCurveTrade(BondingCurveTrade {
            mint: pk(1_000_000 + mint_i),
            trader: pk(3_000_000 + n % 500),
            vault: [0u8; 32],
            is_buy: n % 3 != 0,
            sol_in,
            sol_out: 0,
            tokens_in: 0,
            tokens_out: sol_in * 28_000,
            sol_to_treasury: sol_in / 100,
            sol_to_creator: sol_in / 200,
            protocol_fee: sol_in / 100,
            virtual_sol_after: 37_500_000_000 + n * 1_000_000,
            virtual_token_after: 1_073_000_000_000_000 - n * 28_000_000,
            real_sol_after: n * 1_000_000,
            real_token_after: 1_000_000_000_000_000 - n * 28_000_000,
        })),
        memo.map(|s| s.to_string()),
    )
}

fn short(mint_i: u64, n: u64) -> AnyEvent {
    AnyEvent::Torch(TorchEvent::OpenShort(OpenShortEvent {
        user: pk(4_000_000 + n),
        mint: pk(1_000_000 + mint_i),
        position_index: 0,
        collateral_sol_gross: 2_010_000_000,
        open_fee_sol: 10_000_000,
        net_collateral_sol: 2_000_000_000,
        tokens_borrowed: 999_300_000 + n,
        vault_sol: 2_000_000_000,
    }))
}

struct Args {
    markets: u64,
    trades: u64,
    positions: u64,
    mint: Option<String>,
    rate: u64,
    seconds: u64,
}

fn parse() -> Args {
    let mut a = Args {
        markets: 50,
        trades: 2000,
        positions: 50,
        mint: None,
        rate: 20,
        seconds: 15,
    };
    let mut it = std::env::args().skip(1);
    while let Some(f) = it.next() {
        match f.as_str() {
            "--markets" => a.markets = it.next().unwrap().parse().unwrap(),
            "--trades" => a.trades = it.next().unwrap().parse().unwrap(),
            "--positions" => a.positions = it.next().unwrap().parse().unwrap(),
            "--mint" => a.mint = Some(it.next().unwrap()),
            "--rate" => a.rate = it.next().unwrap().parse().unwrap(),
            "--seconds" => a.seconds = it.next().unwrap().parse().unwrap(),
            other => panic!("unknown flag {other}"),
        }
    }
    a
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let args = parse();
    let url = std::env::var("DATABASE_URL").context("DATABASE_URL")?;
    let pool = torch_indexer::db::connect(&url).await?;

    // Bulk seed.
    println!(
        "seeding {} markets, {} trades, {} shorts…",
        args.markets, args.trades, args.positions
    );
    let mut slot: i64 = 80_000_000;
    for i in 0..args.markets {
        let mut events = vec![de(market(i), slot, 0, 0, None)];
        // trades spread across markets, weighted to market 0 (the hot mint)
        let per = if i == 0 {
            args.trades / 2
        } else {
            args.trades / 2 / args.markets.max(1)
        };
        for n in 0..per {
            let (ev, memo) = trade(i, n, if n % 10 == 0 { Some("gm") } else { None });
            events.push(de(ev, slot, (n + 1) as i32, 0, memo));
        }
        if i < args.positions {
            events.push(de(short(i, i), slot, (per + 2) as i32, 0, None));
        }
        write_events_no_checkpoint(&pool, slot as u64, events).await?;
        slot += 10;
    }
    // print the hot mint for loadtest --mint
    println!("hot mint: {}", bs58::encode(pk(1_000_000)).into_string());
    Ok(())
}
