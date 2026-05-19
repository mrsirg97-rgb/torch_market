#!/usr/bin/env tsx
//
// Run fund_migration_wsol + migrate_to_dex for a bonded token using a
// file-based hot wallet. Uses torchsdk.buildMigrateTransaction to assemble
// the Raydium-flavored tx.
//
// Defaults to DRY-RUN. Pass --commit to actually send.
//
// Usage:
//   tsx scripts/migrate.ts <MINT>
//                          [--keypair PATH]   (default: ~/.config/solana/id.json)
//                          [--rpc URL]        (default: $RPC_URL or mainnet)
//                          [--commit]

import fs from "fs";
import os from "os";
import path from "path";
import {
  Connection,
  Keypair,
  PublicKey,
  VersionedTransaction,
} from "@solana/web3.js";
import { buildMigrateTransaction } from "../clawhub/lib/torchsdk";

const PROGRAM_ID = new PublicKey("8hbUkonssSEEtkqzwM7ZcZrD9evacM92TcWSooVF4BeT");
const BONDING_CURVE_SEED = Buffer.from("bonding_curve");

function parseArgs(argv: string[]) {
  const args = argv.slice(2);
  const opts: {
    mint?: string;
    keypair: string;
    rpc: string;
    commit: boolean;
  } = {
    keypair: path.join(os.homedir(), ".config/solana/id.json"),
    rpc: process.env.RPC_URL ?? "https://api.mainnet-beta.solana.com",
    commit: false,
  };
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a === "--keypair") opts.keypair = args[++i];
    else if (a === "--rpc") opts.rpc = args[++i];
    else if (a === "--commit") opts.commit = true;
    else if (a.startsWith("--")) throw new Error(`unknown flag: ${a}`);
    else if (!opts.mint) opts.mint = a;
    else throw new Error(`unexpected positional arg: ${a}`);
  }
  if (!opts.mint) throw new Error("missing <MINT> argument");
  return opts as typeof opts & { mint: string };
}

function loadKeypair(p: string): Keypair {
  const expanded = p.startsWith("~") ? path.join(os.homedir(), p.slice(1)) : p;
  const bytes = JSON.parse(fs.readFileSync(expanded, "utf8"));
  return Keypair.fromSecretKey(Uint8Array.from(bytes));
}

function decodeBondingCurveFlags(data: Buffer) {
  return {
    bonding_complete: data[120] === 1,
    vote_finalized: data[153] === 1,
    vote_result_return: data[154] === 1,
    migrated: data[155] === 1,
  };
}

async function main() {
  const opts = parseArgs(process.argv);
  const conn = new Connection(opts.rpc, "confirmed");
  const mint = new PublicKey(opts.mint);
  const payer = loadKeypair(opts.keypair);

  console.log("network    :", opts.rpc);
  console.log("mint       :", mint.toBase58());
  console.log("payer      :", payer.publicKey.toBase58(), `(file ${opts.keypair})`);
  console.log("mode       :", opts.commit ? "COMMIT (will send)" : "dry-run (simulate only)");
  console.log("");

  const [bondingCurve] = PublicKey.findProgramAddressSync(
    [BONDING_CURVE_SEED, mint.toBuffer()],
    PROGRAM_ID,
  );
  const bcAcct = await conn.getAccountInfo(bondingCurve);
  if (!bcAcct) throw new Error("bonding_curve not found at " + bondingCurve.toBase58());
  const pre = decodeBondingCurveFlags(Buffer.from(bcAcct.data));
  console.log("bonding_curve PDA:", bondingCurve.toBase58());
  console.log("  bonding_complete  :", pre.bonding_complete);
  console.log("  vote_finalized    :", pre.vote_finalized);
  console.log("  vote_result_return:", pre.vote_result_return);
  console.log("  migrated          :", pre.migrated);
  console.log("");

  if (!pre.bonding_complete) throw new Error("bonding not complete");
  if (!pre.vote_finalized) throw new Error("vote not finalized — run resolve-legacy-vote first");
  if (pre.migrated) throw new Error("already migrated");

  const bal = await conn.getBalance(payer.publicKey, "confirmed");
  console.log("payer SOL balance :", (bal / 1e9).toFixed(4));
  if (bal < 1_500_000_000) {
    console.warn("WARNING: payer < 1.5 SOL — Raydium pool creation may fail");
  }

  console.log("\nbuilding migration tx...");
  const result = await buildMigrateTransaction(conn, {
    mint: mint.toBase58(),
    payer: payer.publicKey.toBase58(),
  });
  const tx = result.transaction as VersionedTransaction;
  console.log("message       :", result.message);

  // Simulation: replaceRecentBlockhash lets us simulate without first sending.
  const sim = await conn.simulateTransaction(tx, {
    sigVerify: false,
    replaceRecentBlockhash: true,
  });
  console.log("\nsimulation logs (last 25):");
  for (const line of (sim.value.logs ?? []).slice(-25)) console.log("  " + line);
  if (sim.value.err) {
    console.error("\nsimulation FAILED:", JSON.stringify(sim.value.err));
    process.exit(1);
  }
  console.log("\nsimulation OK. compute used:", sim.value.unitsConsumed);

  if (!opts.commit) {
    console.log("\nDRY-RUN — not sent. Re-run with --commit to broadcast.");
    return;
  }

  // Fresh blockhash + sign + send.
  const { blockhash, lastValidBlockHeight } =
    await conn.getLatestBlockhash("confirmed");
  tx.message.recentBlockhash = blockhash;
  tx.sign([payer]);

  console.log("\nsending...");
  const txid = await conn.sendRawTransaction(tx.serialize());
  console.log("signature:", txid);
  console.log("explorer :", `https://solscan.io/tx/${txid}`);

  await conn.confirmTransaction(
    { signature: txid, blockhash, lastValidBlockHeight },
    "confirmed",
  );

  const postAcct = await conn.getAccountInfo(bondingCurve, "confirmed");
  const post = decodeBondingCurveFlags(Buffer.from(postAcct!.data));
  console.log("\npost-state:");
  console.log("  migrated          :", post.migrated);
  if (!post.migrated) {
    console.error("WARNING: migrated flag did not flip — check the tx");
    process.exit(1);
  }
  console.log("\n✓ migration complete");
}

main().catch((e) => {
  console.error("error:", e.message ?? e);
  process.exit(1);
});
