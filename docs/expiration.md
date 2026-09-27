# Subscription Expiration Semantics

This document defines the boundary semantics for subscription expiration in the subscription vault contract.

## Overview

Subscriptions support optional expiration via two independent bounds:
1. **Wall-clock expiration** (`expires_at: Option<u64>`) — timestamp-based expiration
2. **Ledger-sequence expiration** (`expires_at_ledger: Option<u32>`) — block-height-based expiration

Either bound being met (or both) causes the subscription to transition to the `Expired` status.

## Boundary Semantics

### Wall-Clock Expiration

A subscription is considered **expired** when:

```rust
current_timestamp >= expires_at
```

This is an **inclusive** boundary check. The moment the ledger timestamp reaches the `expires_at` value, the subscription is expired.

**Examples:**
- If `expires_at = 1000` and `current_timestamp = 999`: **Not expired** (charge succeeds)
- If `expires_at = 1000` and `current_timestamp = 1000`: **Expired** (charge rejected)
- If `expires_at = 1000` and `current_timestamp = 1001`: **Expired** (charge rejected)

### Ledger-Sequence Expiration

A subscription is considered **expired** when:

```rust
current_ledger_sequence >= expires_at_ledger
```

This is also an **inclusive** boundary check, mirroring the wall-clock semantics.

**Examples:**
- If `expires_at_ledger = 500` and `current_sequence = 499`: **Not expired**
- If `expires_at_ledger = 500` and `current_sequence = 500`: **Expired**
- If `expires_at_ledger = 500` and `current_sequence = 501`: **Expired**

### Combined Bounds

When both bounds are set, the subscription expires when **either** condition is met (logical OR):

```rust
is_expired = (current_timestamp >= expires_at) || (current_ledger_sequence >= expires_at_ledger)
```

The **earliest** bound to be reached triggers the expiration.

## Implementation Reference

The boundary check is implemented in `types.rs`:

```rust
pub fn is_expired(&self, current_time: u64, current_ledger: u32) -> bool {
    if let Some(exp) = self.expires_at {
        if current_time >= exp {
            return true;
        }
    }
    if let Some(exp_ledger) = self.expires_at_ledger {
        if current_ledger >= exp_ledger {
            return true;
        }
    }
    false
}
```

## Operations Affected by Expiration

When a subscription is expired (either bound met), the following operations are **rejected** with `Error::SubscriptionExpired`:

1. **`charge_subscription`** — No charges can be processed
2. **`deposit_funds`** — No funds can be added
3. **`charge_usage`** — Usage-based charging is blocked
4. **`cancel_subscription`** — Cannot cancel an expired subscription (cleanup instead)

### Operations Still Permitted

The following operations remain available after expiration:

1. **`withdraw_subscriber_funds`** — Subscribers can withdraw remaining prepaid balance
2. **`cleanup_subscription`** — Transitions the subscription to `Archived` status
3. **`get_subscription`** — Query operations continue to work

## Creation-Time Validation

Subscriptions cannot be created with an expiration already in the past or equal to the current time:

- `expires_at <= current_timestamp` → **Rejected** with `Error::InvalidExpiration`
- `expires_at_ledger <= current_sequence` → **Rejected** with `Error::InvalidExpiration`

The minimum valid expiration is:
- `expires_at >= current_timestamp + 1` (one second in the future)
- `expires_at_ledger >= current_sequence + 1` (one block in the future)

This prevents "zombie" subscriptions that are born already expired and can never be charged.

## Rationale

### Why Inclusive Boundary?

The inclusive boundary (`>=`) provides clear semantics:
- **No ambiguity**: The exact moment the clock reaches the expiration time, the subscription expires
- **Indexer-friendly**: Off-chain indexers can compute the exact block/timestamp when expiration occurs
- **Simpler reasoning**: Users understand "expires at T" to mean "stops working at T"

### Why Two Expiration Bounds?

Different use cases benefit from different expiration models:

1. **Wall-clock** (`expires_at`):
   - Real-world time-based subscriptions (e.g., "30-day trial")
   - Suitable when time is the relevant constraint

2. **Ledger-sequence** (`expires_at_ledger`):
   - Block-height-based expiration for on-chain coordination
   - Suitable for governance or time-locked operations where ledger progression matters

Having both allows users to choose the appropriate model or combine them for defense-in-depth.

## Test Coverage

Boundary behavior is verified in `test_expiration.rs`:

- `test_charge_at_exact_expiration_boundary_rejected` — Charge at `timestamp == expires_at` is rejected
- `test_deposit_at_exact_expiration_boundary_rejected` — Deposit at boundary is rejected
- `test_charge_rejected_when_ledger_bound_met` — Ledger-sequence expiration works identically
- `test_both_bounds_set_ledger_fires_first` — Earliest bound wins when both are set
- `test_both_bounds_set_wall_clock_fires_first` — Wall-clock can fire before ledger bound

## Summary

- **Expiration boundary**: `current_time >= expires_at` (inclusive)
- **Ledger boundary**: `current_sequence >= expires_at_ledger` (inclusive)
- **At the exact boundary timestamp/sequence**: subscription is **expired**
- **Operations rejected**: charge, deposit, charge_usage, cancel
- **Operations permitted**: withdraw, cleanup, query
- **Creation validation**: expiration must be strictly in the future
