/**
 * Anchor event parsing for torch_market transactions.
 *
 * UIs can call `parseLoanRepaidEvents(tx)` / `parseShortClosedEvents(tx)`
 * after a successful repay / close_short to learn whether the position was
 * fully settled. When the corresponding `fully_repaid` / `fully_closed` flag
 * is true, the program also closes the position PDA and refunds rent — so the
 * UI should re-render "no active loan/short" rather than "zeroed loan/short".
 */

import { BorshEventCoder, BN, Idl } from '@coral-xyz/anchor'
import { ParsedTransactionWithMeta, PublicKey } from '@solana/web3.js'
import idl from './torch_market.json'

const coder = new BorshEventCoder(idl as Idl)

export interface LoanRepaidEvent {
  mint: PublicKey
  user: PublicKey
  sol_repaid: BN
  interest_paid: BN
  collateral_returned: BN
  fully_repaid: boolean
}

export interface ShortClosedEvent {
  mint: PublicKey
  user: PublicKey
  tokens_returned: BN
  interest_paid_tokens: BN
  sol_returned: BN
  fully_closed: boolean
}

/**
 * Extract every emitted event with the given name from a parsed transaction.
 * Returns [] if the tx is null, has no logs, or contains no matching events.
 *
 * Anchor events are written to `Program data: <base64>` log lines.
 * BorshEventCoder decodes the discriminator + payload.
 */
export function parseEvents<T = unknown>(
  tx: ParsedTransactionWithMeta | null | undefined,
  eventName: string,
): T[] {
  const logs = tx?.meta?.logMessages
  if (!logs?.length) return []
  const events: T[] = []
  for (const log of logs) {
    const m = /^Program data: (.+)$/.exec(log)
    if (!m) continue
    try {
      const decoded = coder.decode(m[1])
      if (decoded && decoded.name === eventName) {
        events.push(decoded.data as T)
      }
    } catch {
      // Not every "Program data" line is one of our events.
    }
  }
  return events
}

export const parseLoanRepaidEvents = (tx: ParsedTransactionWithMeta | null | undefined) =>
  parseEvents<LoanRepaidEvent>(tx, 'LoanRepaid')

export const parseShortClosedEvents = (tx: ParsedTransactionWithMeta | null | undefined) =>
  parseEvents<ShortClosedEvent>(tx, 'ShortClosed')
