#!/usr/bin/env ts-node
//
// Calls torch_market.resolve_legacy_vote(result) on mainnet for a stranded
// pre-V36 token (bonding_complete=true, vote_finalized=false). Sets
// vote_result_return = result (false → burn at migration, true → treasury_lock).
//
// Defaults to DRY-RUN. Pass --commit to actually send.
//
// Usage:
//   ts-node scripts/resolve-legacy-vote.ts <MINT>
//                                          [--result burn|return]   (default: burn)
//                                          [--keypair PATH]         (file-based signer; default: ~/.config/solana/id.json)
//                                          [--ledger]               (use Ledger hardware wallet instead of --keypair)
//                                          [--ledger-path PATH]     (BIP-44 derivation path; default: 44'/501'/0')
//                                          [--rpc URL]              (default: $RPC_URL or mainnet)
//                                          [--commit]               (default: dry-run / simulate)

import fs from "fs";
import os from "os";
import path from "path";
import { createHash } from "crypto";
import {
  Connection,
  Keypair,
  PublicKey,
  Transaction,
  TransactionInstruction,
} from "@solana/web3.js";

const PROGRAM_ID = new PublicKey("8hbUkonssSEEtkqzwM7ZcZrD9evacM92TcWSooVF4BeT");
const GLOBAL_CONFIG_SEED = Buffer.from("global_config");
const BONDING_CURVE_SEED = Buffer.from("bonding_curve");

const ixDisc = (name: string) =>
  createHash("sha256").update(`global:${name}`).digest().subarray(0, 8);
const RESOLVE_LEGACY_VOTE_DISC = ixDisc("resolve_legacy_vote");

interface Signer {
  publicKey: PublicKey;
  signTransaction(tx: Transaction): Promise<Transaction>;
}

function parseArgs(argv: string[]) {
  const args = argv.slice(2);
  const opts: {
    mint?: string;
    result: "burn" | "return";
    keypair: string;
    ledger: boolean;
    ledgerPath: string;
    rpc: string;
    commit: boolean;
  } = {
    result: "burn",
    keypair: path.join(os.homedir(), ".config/solana/id.json"),
    ledger: false,
    ledgerPath: "44'/501'/0'",
    rpc: process.env.RPC_URL ?? "https://api.mainnet-beta.solana.com",
    commit: false,
  };
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a === "--result") {
      const v = args[++i];
      if (v !== "burn" && v !== "return") throw new Error("--result must be 'burn' or 'return'");
      opts.result = v;
    } else if (a === "--keypair") opts.keypair = args[++i];
    else if (a === "--ledger") opts.ledger = true;
    else if (a === "--ledger-path") opts.ledgerPath = args[++i];
    else if (a === "--rpc") opts.rpc = args[++i];
    else if (a === "--commit") opts.commit = true;
    else if (a.startsWith("--")) throw new Error(`unknown flag: ${a}`);
    else if (!opts.mint) opts.mint = a;
    else throw new Error(`unexpected positional arg: ${a}`);
  }
  if (!opts.mint) throw new Error("missing <MINT> argument");
  return opts as typeof opts & { mint: string };
}

function loadFileSigner(p: string): Signer {
  const expanded = p.startsWith("~") ? path.join(os.homedir(), p.slice(1)) : p;
  const bytes = JSON.parse(fs.readFileSync(expanded, "utf8"));
  const kp = Keypair.fromSecretKey(Uint8Array.from(bytes));
  return {
    publicKey: kp.publicKey,
    signTransaction: async (tx) => {
      tx.sign(kp);
      return tx;
    },
  };
}

async function loadLedgerSigner(derivationPath: string): Promise<Signer> {
  // Imported dynamically so file-keypair users don't pay the USB/HID load cost.
  const TransportNodeHid = (await import("@ledgerhq/hw-transport-node-hid")).default;
  const Solana = (await import("@ledgerhq/hw-app-solana")).default;
  const transport = await TransportNodeHid.create();
  const ledger = new Solana(transport);
  // hw-app-solana wants the path with the m/ prefix stripped and "'" hardened markers.
  const pathBytes = derivationPath;
  const { address } = await ledger.getAddress(pathBytes);
  const pubkey = new PublicKey(address);
  console.log("ledger ready — confirm signing on the device when prompted.");
  return {
    publicKey: pubkey,
    signTransaction: async (tx) => {
      const message = tx.serializeMessage();
      const { signature } = await ledger.signTransaction(pathBytes, message);
      tx.addSignature(pubkey, signature);
      return tx;
    },
  };
}

function decodeBondingCurve(data: Buffer) {
  let o = 8;
  const mint = new PublicKey(data.subarray(o, o + 32)); o += 32;
  o += 32; // creator
  o += 8 * 4; // virtual/real reserves
  const vote_vault_balance = data.readBigUInt64LE(o); o += 8;
  o += 8; // permanently_burned_tokens
  const bonding_complete = data.readUInt8(o) === 1; o += 1;
  o += 8 + 8 + 8 + 8; // slot + 3 vote counts
  const vote_finalized = data.readUInt8(o) === 1; o += 1;
  const vote_result_return = data.readUInt8(o) === 1; o += 1;
  const migrated = data.readUInt8(o) === 1; o += 1;
  return { mint, vote_vault_balance, bonding_complete, vote_finalized, vote_result_return, migrated };
}

function decodeGlobalConfigAuthority(data: Buffer): PublicKey {
  return new PublicKey(data.subarray(8, 40));
}

async function main() {
  const opts = parseArgs(process.argv);

  const conn = new Connection(opts.rpc, "confirmed");
  const signer = opts.ledger
    ? await loadLedgerSigner(opts.ledgerPath)
    : loadFileSigner(opts.keypair);
  const mint = new PublicKey(opts.mint);

  console.log("network    :", opts.rpc);
  console.log("signer     :", signer.publicKey.toBase58(),
    opts.ledger ? `(ledger @ ${opts.ledgerPath})` : `(file ${opts.keypair})`);
  console.log("mint       :", mint.toBase58());
  console.log("tiebreak   :", opts.result, opts.result === "burn"
    ? "(vote_result_return=false → tokens burn at migration)"
    : "(vote_result_return=true  → tokens transfer to treasury_lock at migration)");
  console.log("mode       :", opts.commit ? "COMMIT (will send)" : "dry-run (simulate only)");
  console.log("");

  const [globalConfig] = PublicKey.findProgramAddressSync([GLOBAL_CONFIG_SEED], PROGRAM_ID);
  const [bondingCurve] = PublicKey.findProgramAddressSync(
    [BONDING_CURVE_SEED, mint.toBuffer()],
    PROGRAM_ID,
  );

  const gcAcct = await conn.getAccountInfo(globalConfig);
  if (!gcAcct) throw new Error("global_config not found at " + globalConfig.toBase58());
  const onChainAuthority = decodeGlobalConfigAuthority(Buffer.from(gcAcct.data));
  if (!onChainAuthority.equals(signer.publicKey)) {
    throw new Error(
      `signer ${signer.publicKey.toBase58()} is not the program authority ` +
      `(on-chain authority: ${onChainAuthority.toBase58()})`,
    );
  }

  const bcAcct = await conn.getAccountInfo(bondingCurve);
  if (!bcAcct) throw new Error("bonding_curve not found at " + bondingCurve.toBase58());
  const pre = decodeBondingCurve(Buffer.from(bcAcct.data));
  console.log("bonding_curve PDA      :", bondingCurve.toBase58());
  console.log("  bonding_complete    :", pre.bonding_complete);
  console.log("  vote_finalized      :", pre.vote_finalized);
  console.log("  vote_result_return  :", pre.vote_result_return);
  console.log("  migrated            :", pre.migrated);
  console.log("  vote_vault_balance  :", pre.vote_vault_balance.toString());
  console.log("");

  if (!pre.bonding_complete) throw new Error("token is not bonded — refuses to run");
  if (pre.vote_finalized) throw new Error("vote_finalized is already true — nothing to do");
  if (pre.migrated) throw new Error("token already migrated — nothing to do");

  const resultBool = opts.result === "return";
  const data = Buffer.concat([RESOLVE_LEGACY_VOTE_DISC, Buffer.from([resultBool ? 1 : 0])]);
  const ix = new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: signer.publicKey, isSigner: true, isWritable: false },
      { pubkey: globalConfig, isSigner: false, isWritable: false },
      { pubkey: mint, isSigner: false, isWritable: false },
      { pubkey: bondingCurve, isSigner: false, isWritable: true },
    ],
    data,
  });

  // Simulate unsigned (no Ledger interaction needed for dry-run).
  const simTx = new Transaction().add(ix);
  simTx.feePayer = signer.publicKey;
  const { blockhash } = await conn.getLatestBlockhash("confirmed");
  simTx.recentBlockhash = blockhash;
  const sim = await conn.simulateTransaction(simTx);
  console.log("simulation logs:");
  for (const line of sim.value.logs ?? []) console.log("  " + line);
  if (sim.value.err) {
    console.error("\nsimulation FAILED:", JSON.stringify(sim.value.err));
    process.exit(1);
  }
  console.log("\nsimulation OK.");

  if (!opts.commit) {
    console.log("\nDRY-RUN — not sent. Re-run with --commit to broadcast.");
    return;
  }

  // Sign + send. Fresh blockhash so Ledger isn't asked to sign a stale one.
  const sendTx = new Transaction().add(ix);
  sendTx.feePayer = signer.publicKey;
  const { blockhash: sendBlockhash, lastValidBlockHeight } =
    await conn.getLatestBlockhash("confirmed");
  sendTx.recentBlockhash = sendBlockhash;

  console.log("\nsigning...");
  const signed = await signer.signTransaction(sendTx);
  console.log("sending...");
  const sig = await conn.sendRawTransaction(signed.serialize());
  console.log("signature:", sig);
  console.log("explorer :", `https://solscan.io/tx/${sig}`);

  await conn.confirmTransaction(
    { signature: sig, blockhash: sendBlockhash, lastValidBlockHeight },
    "confirmed",
  );

  const postAcct = await conn.getAccountInfo(bondingCurve, "confirmed");
  const post = decodeBondingCurve(Buffer.from(postAcct!.data));
  console.log("");
  console.log("post-state:");
  console.log("  vote_finalized      :", post.vote_finalized);
  console.log("  vote_result_return  :", post.vote_result_return);
  if (!post.vote_finalized) {
    console.error("WARNING: vote_finalized did not flip — check the tx");
    process.exit(1);
  }
}

main().catch((e) => {
  console.error("error:", e.message ?? e);
  process.exit(1);
});
