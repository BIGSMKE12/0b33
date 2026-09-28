# Merchant Earnings Accounting

`SubscriptionVault` tracks merchant earnings as an internal per-merchant, per-token ledger that
is independent from individual subscription records. Every fund movement is reflected
atomically so the ledger can be reconciled deterministically by off-chain indexers.

## Model

- Each successful charge (`charge_subscription`, `charge_usage`, `charge_one_off`) debits a
  subscription's `prepaid_balance` by its `amount`.
- The same amount (net of any protocol fee) is credited to
  `MerchantBalance[(merchant, token)]` in the same storage write — there is no window where
  a subscription has been debited but the merchant has not been credited.
- Merchant balances are tracked per `(merchant, token)` bucket so multi-token vaults stay
  cleanly separated.
- The contract stores a persistent merchant earnings balance map keyed by
  `DataKey::MerchantBalance(merchant, token)` with spendable `i128` earnings.
- A `TokenEarnings` struct records the full accrual / withdrawal / refund history for each
  bucket, enabling deterministic reconciliation without re-reading event logs.

## Data Structures

```
TokenEarnings {
    accruals: AccruedTotals {
        interval: i128,   // sum of all interval-charge credits
        usage:    i128,   // sum of all usage-charge credits
        one_off:  i128,   // sum of all one-off-charge credits
    },
    withdrawals: i128,    // sum of all merchant withdrawals from this bucket
    refunds:     i128,    // sum of all merchant-initiated subscriber refunds
}

TokenReconciliationSnapshot {
    token:               Address,
    total_accruals:      i128,   // interval + usage + one_off
    total_withdrawals:   i128,
    total_refunds:       i128,
    computed_balance:    i128,   // total_accruals − total_withdrawals − total_refunds
}
```

## Canonical Invariant

For every `(merchant, token)` bucket the following must hold at all times:

```
MerchantBalance[(merchant, token)]
    == accruals.interval + accruals.usage + accruals.one_off
     − withdrawals
     − refunds
```

`get_reconciliation_snapshot(merchant)` returns a `TokenReconciliationSnapshot` per token
where `computed_balance` is exactly the right-hand side. Indexers can cross-check it against
`get_merchant_balance_by_token(merchant, token)` to detect any drift.

## Protocol Fee Handling

When a protocol fee is configured (via `set_protocol_fee`), the subscription charge is split:

- **Merchant receives** `charge_amount * (10_000 − fee_bps) / 10_000` (net amount)
- **Treasury receives** `charge_amount * fee_bps / 10_000` (fee amount)

Both amounts are credited via the same `credit_merchant_balance_for_token` path, so
both the merchant and the treasury address have fully reconciled `TokenEarnings` records.
The merchant's `accruals.interval` records the **net** amount credited to them, not the gross
charge amount.

## Charge Flow (Atomic Credit)

```
charge_one / charge_usage_one
  ├─ debit subscription.prepaid_balance  (write Sub record)
  ├─ credit merchant MerchantBalance     (set_merchant_balance)
  ├─ update TokenEarnings.accruals.*     (set_merchant_token_earnings)
  └─ emit `charged` / `usage_charged` event
```

All four operations happen in the same Soroban invocation frame. If any step fails the
entire transaction reverts atomically.

## Withdrawal Behavior

`withdraw_merchant_funds(merchant, amount)` / `withdraw_merchant_funds_for_token(merchant, token, amount)`:

1. Validates `merchant.require_auth()` and blocklist status.
2. Validates `amount > 0` and `MerchantBalance >= amount`.
3. Checks vault's actual token custody balance ≥ amount.
4. **EFFECTS** (before any external call):
   - Decrements `MerchantBalance[(merchant, token)]` by `amount`.
   - Increments `TokenEarnings.withdrawals` by `amount`.
   - Decrements `TotalAccounted[(token)]` by `amount`.
   - Emits `MerchantWithdrawalEvent` with topic `("withdrawn", merchant, token)`.
5. **INTERACTIONS**: Calls `token.transfer(contract → merchant, amount)`.

The topic contains the token address as the third element so indexers can efficiently
filter withdrawal events per token without decoding the payload.

## Scheduled Payout Behavior

`flush_payouts(merchant)` implements automatic payout scheduling:

1. Reads `PayoutSchedule[(merchant)]` (cadence_seconds, min_payout, last_payout_at).
2. If no schedule is set (cadence_seconds == 0 and min_payout == 0), returns 0 (no-op).
3. Checks cadence eligibility: `now >= last_payout_at + cadence_seconds`.
   - If cadence is not elapsed, returns `Error::IntervalNotElapsed`.
4. Iterates over all tokens in `MerchantTokens[(merchant)]`.
5. For each token:
   - Reads `MerchantBalance[(merchant, token)]`.
   - **If balance is zero, skips the token** (no transfer, no event, no withdrawal record).
   - If balance < min_payout, skips the token.
   - Otherwise, calls `flush_merchant_token(merchant, token, payout_address, balance)`.
6. Updates `last_payout_at` to current timestamp.
7. Emits `ScheduledPayoutEvent` with the count of tokens successfully paid.
8. Returns the count of tokens paid.

**Zero-Balance Behavior**: When a merchant's earned balance for a token is exactly zero,
the scheduled payout logic silently skips that token. No transfer is attempted, no
withdrawal record is created, and the token is not counted in the payout event. This
prevents unnecessary on-chain operations and ensures that only meaningful payouts are
processed. If all tokens have zero balance, `flush_payouts` returns 0 and no
`ScheduledPayoutEvent` is emitted.

## Refund Behavior

`merchant_refund(merchant, subscriber, token, amount)`:

1. Validates `merchant.require_auth()`.
2. Validates `amount > 0` and `MerchantBalance[(merchant, token)] >= amount`.
3. **EFFECTS** (before any external call):
   - Decrements `MerchantBalance[(merchant, token)]` by `amount`.
   - Increments `TokenEarnings.refunds` by `amount`.
   - Decrements `TotalAccounted[(token)]` by `amount`.
   - Emits `MerchantRefundEvent`.
4. **INTERACTIONS**: Calls `token.transfer(contract → subscriber, amount)`.

## Invariants

1. For each successful charge, `subscription.prepaid_balance` decreases by exactly
   `charge_amount` and `MerchantBalance[(merchant, token)]` increases by exactly
   `merchant_amount` (= `charge_amount − fee_amount`) in the same transaction.
2. For each successful withdrawal, `MerchantBalance[(merchant, token)]` decreases by exactly
   the withdrawn amount AND `TokenEarnings.withdrawals` increases by the same amount
   (reconciliation invariant is preserved).
3. For each successful refund, `MerchantBalance[(merchant, token)]` decreases by exactly
   the refunded amount AND `TokenEarnings.refunds` increases by the same amount.
4. Merchant balances are isolated by `(merchant, token)` — debiting one bucket never
   affects another.
5. `TotalAccounted[(token)]` tracks all tokens under custody. It increases on subscriber
   deposits and decreases on merchant withdrawals, merchant refunds, and subscriber fund
   withdrawals. The difference `token.balance(contract) − TotalAccounted[(token)]` is the
   recoverable stranded balance.
6. **Reconciliation**: `MerchantBalance = total_accruals − total_withdrawals − total_refunds`.
   This must always be true and is verified by `get_reconciliation_snapshot`.
7. **Property-test invariant**: random sequences of charges, withdrawals, and cancellations
   are exercised by `prop_merchant_earnings_sum_equals_total_accounted` so that the sum of
   all merchant-bucket balances for a token remains equal to `TotalAccounted[(token)]` after
   every mutation.

## Storage Layout

| Key | Storage Tier | Value |
|-----|-------------|-------|
| `DataKey::MerchantBalance(merchant, token)` | instance | `i128` — spendable balance |
| `DataKey::MerchantEarnings(merchant, token)` | instance | `TokenEarnings` — accrual ledger |
| `DataKey::MerchantTokens(merchant)` | instance | `Vec<Address>` — known token list |
| `DataKey::TotalAccounted(token)` | instance | `i128` — total custody tracking |

## Reporting & Indexers

| Entrypoint | Returns |
|-----------|---------|
| `get_merchant_balance_by_token(merchant, token)` | `i128` — spendable balance |
| `get_merchant_token_earnings(merchant, token)` | `TokenEarnings` — full accrual record |
| `get_merchant_total_earnings(merchant)` | `Vec<(Address, TokenEarnings)>` — all tokens |
| `get_reconciliation_snapshot(merchant)` | `Vec<TokenReconciliationSnapshot>` — cross-check values |

Off-chain indexers should listen to:
- `charged` events — interval charge credit (`amount` = gross charge)
- `usage_charged` events — usage charge credit
- `one_off_charged` events — one-off charge credit
- `protocol_fee_charged` events — fee routing (contains `merchant_amount` and `fee_amount`)
- `withdrawn` events (topic: `("withdrawn", merchant, token)`) — withdrawal debit
- `merchant_refund` events — refund debit

## Security Notes

- All arithmetic uses checked operations (`checked_add`, `checked_sub`) returning
  `Error::Overflow` / `Error::Underflow` instead of panicking or wrapping.
- `validate_non_negative` rejects negative credit amounts before any state is written.
- Withdrawals and refunds follow the **Checks-Effects-Interactions** pattern: all state
  mutations (balance, earnings, accounting) are persisted before the external token transfer.
  If the external call reverts, the entire transaction reverts atomically.
- A blocklisted merchant address cannot call `withdraw_merchant_funds*`. Accumulated
  earnings are preserved and released upon admin unblock.
- Merchants can only withdraw their own `(merchant, token)` bucket; the balance lookup
  is keyed by the caller address, so cross-merchant withdrawal is structurally impossible.

---

# Design Proposal (#158 / #198): Distinguishing Pending from Settled Merchant Funds

> Status: proposal. This section does **not** describe behavior that is implemented yet; it
> is the design requested by issue #158/#198. It is written to be reviewed before any
> contract change is made.

## 1. Problem

Today `MerchantBalance[(merchant, token)]` is a **single** `i128` that is credited the moment
a charge is applied (see *Charge Flow* above) and is immediately spendable via
`withdraw_merchant_funds*`. The same number therefore mixes two economically different
things:

- **Settled funds** — charges that are final and can never be reversed.
- **Pending funds** — charges that have been *applied* but not yet *finalized* (for example,
  a charge that is still within a same-ledger window, or one that an off-chain settlement /
  dispute process could still reverse).

Because withdrawals read the same field, a withdrawal that lands **after a charge in the same
ledger** can draw on funds that are not yet economically final. If that charge is later
reversed (refund, dispute win, settlement rollback), the merchant has already withdrawn more
than they were entitled to and the vault is left short — an unbounded, hard-to-audit loss.

The invariant `MerchantBalance = accruals − withdrawals − refunds` is not violated by this
race; it is simply computed over a balance that was never safe to spend in the first place.

## 2. Race analysis

Soroban orders transactions within a ledger, so this is **not** a classic parallel
read-modify-write race. It is a *finality* race:

1. Ledger `N`, tx A: `charge_subscription` credits `MerchantBalance += X`.
2. Ledger `N`, tx B: `withdraw_merchant_funds(X)` reads the balance, passes the
   `balance >= amount` check, and transfers `X` out.
3. Ledger `N` (or later): the charge is reversed → the vault must return `X` to the
   subscriber, but the merchant already withdrew it.

The window is "same ledger as the credit" only if charges are final at ledger close. If
finality is later (settlement, dispute, or an explicit finalize step), the window is longer
and the exposure is larger. The design must be correct for **both** cases, so it is expressed
in terms of a settlement step rather than a fixed ledger count.

## 3. Proposed model: two buckets

Split each `(merchant, token)` balance into a **pending** and a **settled** bucket. Only
`settled` is withdrawable.

```text
MerchantPendingBalance[(merchant, token)] : i128   // credited on charge, NOT withdrawable
MerchantSettledBalance[(merchant, token)] : i128   // withdrawable
```

Keep `TokenEarnings` as the append-only accrual ledger, but extend it so the reconciliation
identity still holds:

```text
TokenEarnings {
    accruals: AccruedTotals { interval, usage, one_off },   // unchanged
    settled:     i128,   // cumulative amount moved pending -> settled
    withdrawals: i128,   // cumulative settled amount withdrawn (unchanged meaning)
    refunds:     i128,   // cumulative refunds (unchanged)
}
```

Invariants:

```text
pending_balance  == accruals.interval + accruals.usage + accruals.one_off
                  − TokenEarnings.settled
                  − TokenEarnings.refunds_from_pending            (see §5)
settled_balance  == TokenEarnings.settled
                  − TokenEarnings.withdrawals
```

`pending_balance + settled_balance` equals today's `MerchantBalance`, so the total is
unchanged and the existing reconciliation sum still holds. `get_merchant_balance_by_token`
can continue to return `pending + settled` for backward compatibility, with two new
accessors for the split (see §7).

### Charge flow (unchanged shape, new destination)

```
charge_*  ├─ debit subscription.prepaid_balance
          ├─ pending += net_amount           (was: MerchantBalance += net_amount)
          └─ accruals.<kind> += net_amount
```

### Withdrawal flow (settled-only)

```
withdraw_merchant_funds*  ├─ require settled_balance >= amount   (was: balance >= amount)
                          ├─ settled -= amount
                          ├─ withdrawals += amount
                          └─ transfer contract -> merchant
```

A charge applied in the same ledger only ever increases `pending`, so a same-ledger
withdrawal simply cannot see it — the race is removed by construction, not by timing.

## 4. Settlement rules (pending → settled)

Because the model is expressed as a bucket move, the *trigger* is a policy choice that can
be made without changing the rest of the design. Options, in increasing strictness:

| Option | Trigger | Same-ledger race closed? | Notes |
|--------|---------|:---:|-------|
| **A. Explicit finalize** | A `finalize_charge`/`settle` call marks a charge settled | ✅ once called | Gives the operator (or an off-chain settlement job) an explicit, auditable finality point. |
| **B. Ledger delay** | A charge becomes settled at the start of the next ledger (or after `k` ledgers) | ✅ | No new entrypoint; needs a record of "pending as of ledger L". |
| **C. Time delay** | Settle after `settlement_seconds` (reuse `PayoutSchedule`-style config) | ✅ | Aligns with existing cadence machinery. |

Recommendation: **Option A with Option B as the default policy.** An explicit
`settle(merchant, token)` entrypoint (auth: anyone, like `flush_payouts`) moves all pending
for a bucket whose charge ledger is strictly less than the current ledger into settled. This
is deterministic, cheap (one bucket move), and gives indexers a `settled` event to key on.
It also composes with disputes: a disputed charge stays pending and is never settled.

A `last_credit_ledger[(merchant, token)]: u32` marker (the ledger of the most recent pending
credit) is all the state needed to implement B, and is cheap to maintain on the charge path.

## 5. Refunds against pending vs settled

Refunds must not be able to move money that was never settled *out* of settled. Two cases:

- Refund of an already-settled charge → debit `settled`, increment `refunds` (today's path).
- Refund/reversal of a still-pending charge → debit `pending` and increment a new
  `refunds_from_pending`, leaving `settled` untouched.

This keeps `settled − withdrawals` non-negative at all times, which is the property that
makes over-draw impossible.

## 6. Locking-mechanism alternative (the other AC option)

If a full two-bucket split is judged too large a change for this issue, the same safety can
be obtained with a minimal **finality guard**:

- Keep `MerchantBalance` as today.
- Store `last_credit_ledger[(merchant, token)]` on every charge.
- In `withdraw_merchant_funds*`, reject with a new `Error::FundsPendingSettlement` when
  `last_credit_ledger == env.ledger().sequence()` (i.e. a credit happened in the same
  ledger), forcing withdrawal into a later ledger.

This is strictly weaker than §3–§5 (it only closes the same-ledger window, not a longer
settlement/dispute window) but it is a ~10-line change with no storage-schema split. It is
the recommended *minimal* fallback and could ship first, with the two-bucket model as the
follow-up. Either way the guard must be checked **before** any effect is written, so a
rejected withdrawal is a total no-op.

## 7. Compatibility & migration

- **Public read ABI**: `get_merchant_balance_by_token` keeps returning
  `pending + settled`. Add `get_merchant_pending_balance` and `get_merchant_settled_balance`
  for the split. No existing caller breaks.
- **Write/mutation ABI**: `charge_*`, `withdraw_*`, `merchant_refund`, `flush_payouts`
  signatures are unchanged.
- **Storage**: new keys `MerchantPendingBalance`, `MerchantSettledBalance`,
  `last_credit_ledger` (option B) — additive.
- **Migration**: on the schema bump, for every `MerchantTokens(merchant)` entry move the
  existing `MerchantBalance` into `settled` (existing balances are, by definition, the
  pre-change state and are treated as final) and initialize `pending = 0`. This is a pure
  relabeling; no funds move. Bump the schema version and add a migration test.

## 8. Updated invariants

1. `pending >= 0` and `settled >= 0` at all times.
2. `settled − withdrawals >= 0` (no over-draw).
3. `pending + settled == total_accruals − total_withdrawals − total_refunds` (the existing
   reconciliation identity, now provable per-bucket).
4. A withdrawal never increases `pending`; a charge never increases `settled` except via the
   explicit settle path.
5. `TotalAccounted[(token)]` accounting is unchanged: it tracks custody, and every transfer
   still debits it before the external call (CEI unchanged).

## 9. Test plan

- **Same-ledger**: charge then withdraw in the same ledger → withdrawal sees only settled
  (0 on first-ever charge) and must fail / be reduced; assert no over-draw.
- **Next-ledger settle**: charge, advance the ledger, settle, withdraw → succeeds.
- **Pending refund**: reverse a pending charge → `pending` decreases, `settled` unchanged.
- **Reconciliation**: property test that `pending + settled` equals the existing
  `computed_balance` after arbitrary charge/settle/withdraw/refund sequences (extends
  `prop_merchant_earnings_sum_equals_total_accounted`).
- **Migration**: a v_old balance migrates into `settled` and is immediately withdrawable.
- **Failure atomicity**: a rejected withdrawal (pending-only funds) leaves `pending`,
  `settled`, `withdrawals`, and `TotalAccounted` byte-for-byte unchanged.

## 10. Rollout / rollback

Ship behind the schema migration; there is no flag-day for callers. Rollback is a redeploy of
the previous WASM against a schema version that still carries the pre-migration balance in
`settled` — no funds need to move to roll back, because the split never changed who is
entitled to what, only *when* it may be withdrawn.
