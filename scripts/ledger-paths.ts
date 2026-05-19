#!/usr/bin/env tsx
// Scans common Solana Ledger derivation paths and prints the pubkey at each.
// Usage: tsx scripts/ledger-paths.ts [--target PUBKEY]
//
// If --target is given, stops at the first match.

import { PublicKey } from "@solana/web3.js";

async function main() {
  const args = process.argv.slice(2);
  let target: string | undefined;
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--target") target = args[++i];
  }

  const TransportNodeHid = (await import("@ledgerhq/hw-transport-node-hid")).default;
  const Solana = (await import("@ledgerhq/hw-app-solana")).default;
  const transport = await TransportNodeHid.create();
  const ledger = new Solana(transport);

  const paths: string[] = [];
  // Account-level paths (most common)
  for (let i = 0; i < 8; i++) paths.push(`44'/501'/${i}'`);
  // Change-level paths (some wallets use these)
  for (let i = 0; i < 8; i++) paths.push(`44'/501'/${i}'/0'`);

  for (const p of paths) {
    const { address } = await ledger.getAddress(p);
    const pubkey = new PublicKey(address).toBase58();
    const match = target && pubkey === target ? "  ← MATCH" : "";
    console.log(`${p.padEnd(20)} ${pubkey}${match}`);
    if (target && pubkey === target) break;
  }
}

main().catch((e) => {
  console.error("error:", e.message ?? e);
  process.exit(1);
});
