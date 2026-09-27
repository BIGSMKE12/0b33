# Fee Routing — Walkthrough with Numeric Examples

## Overview

On every successful charge (interval, usage, or one-off), the vault splits the
gross charge amount into a **merchant net** and a **protocol fee** credited to
the treasury.

```
gross  =  net  +  fee
```

The fee is a percentage of the gross, expressed in **basis points** (bps).
One basis point = 0.01 %, so:

| Fee (bps) | Fee (%) |
|-----------|---------|
| 0         | 0 %     |
| 250       | 2.50 %  |
| 1 000     | 10 %    |
| 10 000    | 100 %   |

---

## The Formula

The fee is computed with a single integer operation:

```
fee  =  gross × fee_bps / 10 000          (floor division)
net  =  gross − fee
```

The division is **integer floor division** — the remainder stays with the
merchant.  This is the **deterministic rounding rule** and guarantees
conservation on every charge.

Floor division alone can round a fee down to **zero** on a micro-charge, which
would leave the protocol with no fee at all on that charge.  So the contract adds
a **minimum fee of one base unit** whenever `fee_bps > 0` and a treasury is
configured:

```
fee  =  max(gross × fee_bps / 10 000, 1)
```

The floor is applied to the fee in the **settlement token**, before the fee is
credited to the treasury, and it never changes `net = gross − fee`.  Its
consequences are covered under
[Dust and the minimum-fee floor](#dust-and-the-minimum-fee-floor).

---

## Concrete Examples

### Example 1: 6-decimal token (USDC)

**Setup:** `fee_bps = 250` (2.50 %), charge of **100 USDC** (100 × 10⁶ = 100 000 000).

| Step | Calculation | Raw (i128) | Display |
|------|-------------|------------|---------|
| Gross | | 100 000 000 | 100.000 000 USDC |
| Fee | `100_000_000 × 250 / 10_000` | 2 500 000 | 2.500 000 USDC |
| Net | `100_000_000 − 2_500_000` | 97 500 000 | 97.500 000 USDC |

**Check:** `97 500 000 + 2 500 000 = 100 000 000` ✓

---

### Example 2: 2-decimal token

**Setup:** `fee_bps = 250` (2.50 %), charge of **100.00** tokens
(100 × 10² = 10 000).

| Step | Calculation | Raw (i128) | Display |
|------|-------------|------------|---------|
| Gross | | 10 000 | 100.00 |
| Fee | `10_000 × 250 / 10_000` | 250 | 2.50 |
| Net | `10_000 − 250` | 9 750 | 97.50 |

**Check:** `9 750 + 250 = 10 000` ✓

---

### Example 3: 7-decimal token

**Setup:** `fee_bps = 250` (2.50 %), charge of **100.000 000 0** tokens
(100 × 10⁷ = 1 000 000 000).

| Step | Calculation | Raw (i128) | Display |
|------|-------------|------------|---------|
| Gross | | 1 000 000 000 | 100.000 000 0 |
| Fee | `1_000_000_000 × 250 / 10_000` | 25 000 000 | 2.500 000 0 |
| Net | `1_000_000_000 − 25_000_000` | 975 000 000 | 97.500 000 0 |

**Check:** `975 000 000 + 25 000 000 = 1 000 000 000` ✓

---

## The Rounding Party (Edge Cases)

Because the fee uses integer floor division, not every amount divides evenly.
The **remainder always stays with the merchant**.

### Example 4: Non-divisible amount (6-decimal)

**Setup:** `fee_bps = 250`, charge of **1 USDC** (1 000 000 raw).

```
fee   = 1_000_000 × 250 / 10_000
      = 250_000_000 / 10_000
      = 25_000              (exact — 0.025 000 USDC)

net   = 1_000_000 − 25_000
      = 975_000             (0.975 000 USDC)
```

### Example 5: Non-divisible with remainder

**Setup:** `fee_bps = 333` (3.33 %), charge of **1 USDC** (1 000 000 raw).

```
fee   = 1_000_000 × 333 / 10_000
      = 333_000_000 / 10_000
      = 33_300              (floor — 0.033 300 USDC)

net   = 1_000_000 − 33_300
      = 966_700             (0.966 700 USDC)

check: 33_300 + 966_700 = 1_000_000 ✓
```

The exact mathematical result of 1 000 000 × 3.33 % would be **33 333.33**,
but integer arithmetic truncates the fractional .33 remainder.  The merchant
receives **one extra raw unit** — rounding always favors the merchant.

### Example 6: Very small charge with high fee

**Setup:** `fee_bps = 10 000` (100 %), charge of **1 unit** (1 raw).

```
fee   = 1 × 10_000 / 10_000
      = 10_000 / 10_000
      = 1

net   = 1 − 1
      = 0
```

The entire amount goes to the treasury; the merchant receives 0.

### Example 7: Sub-unit charge hits the minimum-fee floor

**Setup:** `fee_bps = 250` (2.50 %), charge of **1 unit** (1 raw) — a sub-cent
amount in a 6-decimal token, with a treasury configured.

```
fee   = 1 × 250 / 10_000
      = 250 / 10_000
      = 0                  (floor)

fee   = max(0, 1)          (minimum-fee floor)
      = 1

net   = 1 − 1
      = 0
```

Without the floor the fee would truncate to zero and the merchant would keep the
whole unit.  With it the treasury collects **1 raw unit** and the merchant net is
0 — conservation still holds (`0 + 1 = 1`).  The single unit is negligible
(e.g. 1 raw = 0.000 001 USDC) but it is now accounted for instead of silently
dropped from the fee split.

If `fee_bps == 0`, or if no treasury address is configured, the floor does not
apply, the fee stays 0, and the merchant keeps the whole gross.

---

## Dust and the Minimum-Fee Floor

### The problem

`fee = gross × fee_bps / 10 000` is integer floor division.  On a charge small
enough that the fee floors to **zero** — anything below `10 000 / fee_bps` base
units, e.g. a 1-unit charge at 2.50 % — the protocol would collect nothing.  The
remainder is not stored anywhere and there is no per-charge record of the
shortfall, so the amount is simply dropped and the sum of merchant payouts plus
protocol fees drifts below the sum of charged amounts.

### The strategy: round the fee up to one base unit

Rather than maintain a dust pool, which would need per-token accounting,
extra storage and its own withdrawal path, the contract rounds a sub-unit fee
**up to 1 base unit**:

```
fee = max(gross × fee_bps / 10 000, 1)
```

This applies wherever a fee is charged — interval, usage and one-off charges
(`charge_one`, `charge_usage_one`, `do_charge_one_off` in
`charge_core.rs`).

The floor is conditional, so it never invents a fee where the protocol has
decided not to collect one:

| Condition | Result |
|-----------|--------|
| `fee_bps == 0` | `fee = 0`; merchant keeps the full gross |
| `fee_bps > 0`, **no treasury configured** | `fee = 0`; merchant keeps the full gross |
| `fee_bps > 0`, treasury configured, `floor(...) >= 1` | `fee = floor(gross × fee_bps / 10 000)` (unchanged) |
| `fee_bps > 0`, treasury configured, `floor(...) == 0` | `fee = 1`, `net = gross − 1` |

### Invariants that still hold

- **Conservation is untouched:** `net = gross − fee`, so `net + fee == gross` on
  every charge, including charges that hit the floor.  The floor moves one unit
  from the merchant to the treasury; it never creates or destroys value.
- **The floor only raises a fee, never lowers it.**  A charge whose percentage
  fee already reaches one base unit is unaffected.
- **At most one base unit is recovered per charge**, so the correction stays
  economically negligible next to the charged amount.

### Regression coverage

`contracts/subscription_vault/src/test_fee_routing_dust.rs` pins the
`net + fee == gross` invariant for small values — including a 1-unit charge at
`fee_bps = 1` and at `MAX_FEE_BIPS` — and asserts that a modeled
`max(floor(gross × fee_bps / 10 000), 1)` fee is at least one base unit whenever
`fee_bps > 0`.

---

## Edge Case: Fee at Zero

**Setup:** `fee_bps = 0`, any charge amount.

```
fee   = gross × 0 / 10_000 = 0
net   = gross
```

The full gross is credited to the merchant.  No `ProtocolFeeChargedEvent` is
emitted.  This is the default (disabled) state — see
[`docs/protocol_fees.md`](protocol_fees.md).

---

## Edge Case: Fee at MAX_FEE_BIPS (10 000)

**Setup:** `fee_bps = 10 000` (100 %), charge of **100 USDC**.

```
fee   = 100_000_000 × 10_000 / 10_000
      = 100_000_000 × 1
      = 100_000_000        (100 %)

net   = 0
```

The entire gross amount is credited to the treasury.  The merchant receives
nothing.  This extreme setting is useful only for on-chain treasury sweeps
or testing.

---

## Cross-Token Fee Routing & Fee-Token Override

The current contract routes all fees in the **same token** as the charge — the
subscription's settlement token.  If a subscription charges in USDC, the
protocol fee is also credited in USDC.

There is **no fee-token override** mechanism in the current version.  A future
upgrade may add a per-merchant `fee_token` address that allows the treasury to
receive fees in a different token from the settlement token.  In that design:

```
gross (settlement token)      →  merchant receives gross − fee
fee   (converted at oracle)   →  treasury receives fee value in fee_token
```

Until that feature lands, all routing uses a single settlement denomination.

---

## Events

### `ProtocolFeeChargedEvent`

Emitted on every charge where `fee > 0`.  The event carries the exact
on-chain amounts so indexers can reconstruct the split.

| Field            | Type      | Description                          |
|------------------|-----------|--------------------------------------|
| `subscription_id` | `u32`    | Subscription that was charged        |
| `merchant`       | `Address` | Merchant receiving the net amount    |
| `token`          | `Address` | Settlement token                     |
| `fee_amount`     | `i128`    | Fee credited to treasury             |
| `treasury`       | `Address` | Treasury address receiving the fee   |
| `timestamp`      | `u64`     | Ledger timestamp                     |

### `ProtocolFeeConfiguredEvent`

Emitted when the admin calls `set_protocol_fee`.

| Field       | Type      | Description              |
|-------------|-----------|--------------------------|
| `admin`     | `Address` | Admin who changed the fee |
| `treasury`  | `Address` | Fee recipient address    |
| `fee_bps`   | `u32`     | Fee in basis points      |
| `timestamp` | `u64`     | Ledger timestamp         |

---

## Accounting Invariants

1. **Conservation:** `gross == net + fee` on every charge.  Verified in the
   code by subtracting the fee from gross, never by adding.

2. **Rounding favors the merchant, except at the minimum-fee floor:**
   Integer floor division keeps any remainder from the fee calculation in the
   merchant's net.  The one exception is the minimum-fee floor: when `fee_bps > 0`
   and a treasury is configured, a fee that would floor below 1 base unit is
   raised to exactly 1.  The treasury therefore receives
   `max(floor(gross × fee_bps / 10 000), 1)` — never a share of the remainder, and
   never more than one extra unit on the charges that hit the floor.  See
   [Dust and the minimum-fee floor](#dust-and-the-minimum-fee-floor).

3. **Fee is computed from discounted amount:** When a coupon is applied,
   the protocol fee is computed from the **post-discount** payable amount,
   not the gross pre-discount amount.  See
   [`docs/protocol_fees.md`](protocol_fees.md) for details.

4. **No treasury → no fee:** If `fee_bps > 0` but no treasury address is
   configured, the full gross is credited to the merchant.  This prevents
   silent fund loss.

---

## Security Notes

- Fee computation uses **pure integer arithmetic** with no external calls —
  no reentrancy risk.
- The treasury balance accrues identically to merchant balances and is
  subject to the same withdrawal controls.
- `fee_bps > 10_000` is rejected at configuration time (`InvalidInput`).
- The fee is computed from the **gross** charge amount, not from the
  merchant's net — preventing fee-on-fee compounding.

---

## References

- [`docs/protocol_fees.md`](protocol_fees.md) — Full protocol fee specification
- [`docs/merchant_earnings.md`](merchant_earnings.md) — How merchant balances work
- [`docs/governance/authoring.md`](governance/authoring.md) — Governance proposals for changing fee parameters
- `contracts/subscription_vault/src/test_fee_routing_dust.rs` — Dust / minimum-fee floor regression tests
- `contracts/subscription_vault/src/charge_core.rs` — Interval and usage charge fee logic
- `contracts/subscription_vault/src/subscription.rs` — One-off charge fee logic
- `contracts/subscription_vault/src/admin.rs` — Fee configuration (`set_protocol_fee`)
- `contracts/subscription_vault/src/types.rs` — Event structs, `MAX_FEE_BIPS`

