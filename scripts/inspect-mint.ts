import { Connection, PublicKey } from "@solana/web3.js";
import { getTransferFeeConfig, TOKEN_2022_PROGRAM_ID, unpackMint } from "@solana/spl-token";

async function main() {
  const c = new Connection(process.env.RPC_URL ?? "https://api.mainnet-beta.solana.com");
  const mintArg = process.argv[2];
  if (!mintArg) throw new Error("usage: tsx inspect-mint.ts <MINT>");
  const m = new PublicKey(mintArg);
  const info = await c.getAccountInfo(m);
  if (!info) return console.log("not found");
  const mint = unpackMint(m, info, TOKEN_2022_PROGRAM_ID);
  console.log("supply       :", mint.supply.toString());
  console.log("decimals     :", mint.decimals);
  const fee = getTransferFeeConfig(mint);
  if (fee) {
    console.log("older fee bps:", fee.olderTransferFee.transferFeeBasisPoints);
    console.log("older max   :", fee.olderTransferFee.maximumFee.toString());
    console.log("older epoch :", fee.olderTransferFee.epoch.toString());
    console.log("newer fee bps:", fee.newerTransferFee.transferFeeBasisPoints);
    console.log("newer max   :", fee.newerTransferFee.maximumFee.toString());
    console.log("newer epoch :", fee.newerTransferFee.epoch.toString());
  } else {
    console.log("no transfer fee");
  }
  const epoch = (await c.getEpochInfo()).epoch;
  console.log("current epoch:", epoch);
}
main().catch((e) => { console.error(e); process.exit(1); });
