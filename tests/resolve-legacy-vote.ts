import { startAnchor, ProgramTestContext } from "solana-bankrun";
import {
  Keypair,
  PublicKey,
  Transaction,
  TransactionInstruction,
  SystemProgram,
} from "@solana/web3.js";
import { createHash } from "crypto";
import { expect } from "chai";

const PROGRAM_ID = new PublicKey("8hbUkonssSEEtkqzwM7ZcZrD9evacM92TcWSooVF4BeT");
const TOKEN_2022_PROGRAM_ID = new PublicKey(
  "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
);
const GLOBAL_CONFIG_SEED = Buffer.from("global_config");
const BONDING_CURVE_SEED = Buffer.from("bonding_curve");

const acctDisc = (name: string) =>
  createHash("sha256").update(`account:${name}`).digest().subarray(0, 8);
const ixDisc = (name: string) =>
  createHash("sha256").update(`global:${name}`).digest().subarray(0, 8);

const GLOBAL_CONFIG_DISC = acctDisc("GlobalConfig");
const BONDING_CURVE_DISC = acctDisc("BondingCurve");
const RESOLVE_LEGACY_VOTE_DISC = ixDisc("resolve_legacy_vote");

interface BondingCurveFields {
  bondingComplete?: boolean;
  voteFinalized?: boolean;
  voteResultReturn?: boolean;
  migrated?: boolean;
  voteVaultBalance?: bigint;
}

function packGlobalConfig(authority: PublicKey, bump: number): Buffer {
  // 8 disc + 32 authority + 32 treasury + 32 dev_wallet + 32 deprecated +
  // 2 fee_bps + 1 paused + 8 launched + 8 volume + 1 bump = 156
  const buf = Buffer.alloc(156);
  GLOBAL_CONFIG_DISC.copy(buf, 0);
  authority.toBuffer().copy(buf, 8);
  // treasury, dev_wallet, deprecated remain zero — unused in our ix
  buf.writeUInt16LE(50, 8 + 32 * 4); // protocol_fee_bps
  buf.writeUInt8(0, 8 + 32 * 4 + 2); // paused
  buf.writeUInt8(bump, 8 + 32 * 4 + 2 + 1 + 8 + 8); // bump
  return buf;
}

function packBondingCurve(
  mint: PublicKey,
  bump: number,
  fields: BondingCurveFields,
): Buffer {
  // Layout total = 418 bytes (matches mainnet account space).
  const buf = Buffer.alloc(418);
  BONDING_CURVE_DISC.copy(buf, 0);
  let o = 8;
  mint.toBuffer().copy(buf, o); o += 32;
  // creator — junk
  Keypair.generate().publicKey.toBuffer().copy(buf, o); o += 32;
  buf.writeBigUInt64LE(69_348_007_993n, o); o += 8; // virtual_sol_reserves
  buf.writeBigUInt64LE(204_471_446_471_909n, o); o += 8; // virtual_token_reserves
  buf.writeBigUInt64LE(50_598_007_993n, o); o += 8; // real_sol_reserves
  buf.writeBigUInt64LE(198_221_446_471_909n, o); o += 8; // real_token_reserves
  buf.writeBigUInt64LE(fields.voteVaultBalance ?? 26_399_392_331_539n, o); o += 8;
  buf.writeBigUInt64LE(0n, o); o += 8; // permanently_burned_tokens
  buf.writeUInt8(fields.bondingComplete === false ? 0 : 1, o); o += 1; // default true
  buf.writeBigUInt64LE(420_696_619n, o); o += 8; // bonding_complete_slot
  buf.writeBigUInt64LE(13n, o); o += 8; // votes_return
  buf.writeBigUInt64LE(13n, o); o += 8; // votes_burn
  buf.writeBigUInt64LE(26n, o); o += 8; // total_voters
  buf.writeUInt8(fields.voteFinalized ? 1 : 0, o); o += 1;
  buf.writeUInt8(fields.voteResultReturn ? 1 : 0, o); o += 1;
  buf.writeUInt8(fields.migrated ? 1 : 0, o); o += 1;
  buf.writeUInt8(1, o); o += 1; // is_token_2022
  buf.writeBigUInt64LE(420_696_619n, o); o += 8; // last_activity_slot
  buf.writeUInt8(0, o); o += 1; // reclaimed
  // name [32], symbol [10], uri [200] — zeros are fine
  o += 32 + 10 + 200;
  buf.writeUInt8(bump, o); o += 1;
  buf.writeUInt8(255, o); o += 1; // treasury_bump
  buf.writeBigUInt64LE(50_000_000_000n, o); o += 8; // bonding_target (SPARK)
  return buf;
}

function plantMint(ctx: ProgramTestContext, mint: PublicKey) {
  // Minimal Token-2022 base mint (82 bytes): zeroed mint_authority COption,
  // zero supply, decimals=6, is_initialized=1, zeroed freeze_authority COption.
  const data = new Uint8Array(82);
  data[44] = 6; // decimals
  data[45] = 1; // is_initialized
  ctx.setAccount(mint, {
    lamports: 1_500_000,
    data,
    owner: TOKEN_2022_PROGRAM_ID,
    executable: false,
    rentEpoch: 0,
  });
}

function plantGlobalConfig(
  ctx: ProgramTestContext,
  authority: PublicKey,
): PublicKey {
  const [pda, bump] = PublicKey.findProgramAddressSync(
    [GLOBAL_CONFIG_SEED],
    PROGRAM_ID,
  );
  ctx.setAccount(pda, {
    lamports: 1_000_000_000,
    data: packGlobalConfig(authority, bump),
    owner: PROGRAM_ID,
    executable: false,
    rentEpoch: 0,
  });
  return pda;
}

function plantBondingCurve(
  ctx: ProgramTestContext,
  mint: PublicKey,
  fields: BondingCurveFields = {},
): PublicKey {
  const [pda, bump] = PublicKey.findProgramAddressSync(
    [BONDING_CURVE_SEED, mint.toBuffer()],
    PROGRAM_ID,
  );
  ctx.setAccount(pda, {
    lamports: 50_000_000_000,
    data: packBondingCurve(mint, bump, fields),
    owner: PROGRAM_ID,
    executable: false,
    rentEpoch: 0,
  });
  return pda;
}

function resolveLegacyVoteIx(
  authority: PublicKey,
  globalConfig: PublicKey,
  mint: PublicKey,
  bondingCurve: PublicKey,
  result: boolean,
): TransactionInstruction {
  const data = Buffer.concat([
    RESOLVE_LEGACY_VOTE_DISC,
    Buffer.from([result ? 1 : 0]),
  ]);
  return new TransactionInstruction({
    programId: PROGRAM_ID,
    keys: [
      { pubkey: authority, isSigner: true, isWritable: false },
      { pubkey: globalConfig, isSigner: false, isWritable: false },
      { pubkey: mint, isSigner: false, isWritable: false },
      { pubkey: bondingCurve, isSigner: false, isWritable: true },
    ],
    data,
  });
}

async function send(
  ctx: ProgramTestContext,
  ix: TransactionInstruction,
  payer: Keypair,
  signers: Keypair[] = [],
) {
  const tx = new Transaction().add(ix);
  tx.recentBlockhash = ctx.lastBlockhash;
  tx.feePayer = payer.publicKey;
  tx.sign(payer, ...signers);
  return ctx.banksClient.tryProcessTransaction(tx);
}

function readBondingCurve(data: Uint8Array) {
  const buf = Buffer.from(data);
  return {
    voteFinalized: buf.readUInt8(153) === 1,
    voteResultReturn: buf.readUInt8(154) === 1,
    migrated: buf.readUInt8(155) === 1,
  };
}

describe("resolve_legacy_vote", () => {
  let ctx: ProgramTestContext;
  let authority: Keypair;
  let globalConfig: PublicKey;

  beforeEach(async () => {
    ctx = await startAnchor("./", [], []);
    authority = Keypair.generate();
    // Fund authority from the bankrun payer.
    const fundIx = SystemProgram.transfer({
      fromPubkey: ctx.payer.publicKey,
      toPubkey: authority.publicKey,
      lamports: 5_000_000_000,
    });
    await send(ctx, fundIx, ctx.payer);
    globalConfig = plantGlobalConfig(ctx, authority.publicKey);
  });

  it("admin → result=false flips vote_finalized=true, vote_result_return=false (burn branch)", async () => {
    const mint = Keypair.generate().publicKey;
    plantMint(ctx, mint);
    const bc = plantBondingCurve(ctx, mint);

    const res = await send(
      ctx,
      resolveLegacyVoteIx(authority.publicKey, globalConfig, mint, bc, false),
      authority,
    );
    expect(res.result, `unexpected error: ${res.result}`).to.be.null;

    const acct = await ctx.banksClient.getAccount(bc);
    const state = readBondingCurve(acct!.data);
    expect(state.voteFinalized).to.equal(true);
    expect(state.voteResultReturn).to.equal(false);
  });

  it("admin → result=true sets vote_result_return=true (treasury_lock branch)", async () => {
    const mint = Keypair.generate().publicKey;
    plantMint(ctx, mint);
    const bc = plantBondingCurve(ctx, mint);

    const res = await send(
      ctx,
      resolveLegacyVoteIx(authority.publicKey, globalConfig, mint, bc, true),
      authority,
    );
    expect(res.result).to.be.null;

    const state = readBondingCurve((await ctx.banksClient.getAccount(bc))!.data);
    expect(state.voteFinalized).to.equal(true);
    expect(state.voteResultReturn).to.equal(true);
  });

  it("non-authority signer is rejected (Unauthorized)", async () => {
    const mint = Keypair.generate().publicKey;
    plantMint(ctx, mint);
    const bc = plantBondingCurve(ctx, mint);

    const attacker = Keypair.generate();
    const fund = SystemProgram.transfer({
      fromPubkey: ctx.payer.publicKey,
      toPubkey: attacker.publicKey,
      lamports: 1_000_000_000,
    });
    await send(ctx, fund, ctx.payer);

    const res = await send(
      ctx,
      resolveLegacyVoteIx(attacker.publicKey, globalConfig, mint, bc, false),
      attacker,
    );
    expect(res.result).to.not.be.null;
    const logs = (res.meta?.logMessages ?? []).join("\n");
    expect(logs).to.match(/Unauthorized/);
  });

  it("already-finalized curve is rejected (VoteAlreadyFinalized)", async () => {
    const mint = Keypair.generate().publicKey;
    plantMint(ctx, mint);
    const bc = plantBondingCurve(ctx, mint, { voteFinalized: true });

    const res = await send(
      ctx,
      resolveLegacyVoteIx(authority.publicKey, globalConfig, mint, bc, false),
      authority,
    );
    expect(res.result).to.not.be.null;
    const logs = (res.meta?.logMessages ?? []).join("\n");
    expect(logs).to.match(/VoteAlreadyFinalized/);
  });

  it("pre-bonded curve is rejected (BondingNotComplete)", async () => {
    const mint = Keypair.generate().publicKey;
    plantMint(ctx, mint);
    const bc = plantBondingCurve(ctx, mint, { bondingComplete: false });

    const res = await send(
      ctx,
      resolveLegacyVoteIx(authority.publicKey, globalConfig, mint, bc, false),
      authority,
    );
    expect(res.result).to.not.be.null;
    const logs = (res.meta?.logMessages ?? []).join("\n");
    expect(logs).to.match(/BondingNotComplete/);
  });

  it("already-migrated curve is rejected (AlreadyMigrated)", async () => {
    const mint = Keypair.generate().publicKey;
    plantMint(ctx, mint);
    const bc = plantBondingCurve(ctx, mint, { migrated: true });

    const res = await send(
      ctx,
      resolveLegacyVoteIx(authority.publicKey, globalConfig, mint, bc, false),
      authority,
    );
    expect(res.result).to.not.be.null;
    const logs = (res.meta?.logMessages ?? []).join("\n");
    expect(logs).to.match(/AlreadyMigrated/);
  });
});
