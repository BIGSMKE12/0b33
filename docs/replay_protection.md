# Replay Protection and Idempotency for Charges

This document describes how the subscription vault prevents double-charging and how off-chain billing engines should integrate with it.

## Overview

Charge operations (`charge_subscription` and, internally, each item in `batch_charge`) are protected against:

1. **Replay**: Charging the same billing period more than once.
2. **Idempotent retries**: Allowing the same logical charge to be submitted multiple times (e.g. network retry) without double-debiting.

Storage usage is kept bounded: one period index and one ring buffer of up to 64 idempotency key hashes per subscription.

## Mechanisms

### Period-based key (always on)

- For each subscription we record the **last charged billing period** as `period_index = now / interval_seconds` (integer division).
- Before charging we require that the current period has not already been charged. If it has, the contract returns `Error::Replay`.
- After a successful charge we store the current `period_index` for that subscription.
- **Storage**: One `u64` per subscription (key: `DataKey::ChargedPeriod(subscription_id)`).

### Optional idempotency key (caller-provided)

Three entrypoints accept an optional idempotency key:

| Entrypoint | Domain constant | Domain value |
|---|---|---|
| `charge_subscription` | `DOMAIN_CHARGE_INTERVAL` | 5 |
| `deposit_funds` | `DOMAIN_DEPOSIT_FUNDS` | 6 |
| `charge_one_off` | `DOMAIN_CHARGE_ONEOFF` | 7 |

- Each entrypoint accepts an `idem_key: Option<BytesN<32>>`.
- If the caller supplies a key and we have already recorded the **hash** of `(domain, subscription_id, key)` for this subscription, we return the success variant without modifying state (idempotent no-op).
- If the key is new, the normal checks run, the operation executes, and the hashed key is appended to the ring buffer.

#### Key hashing

The raw caller-supplied key is **never stored directly**. Instead, the contract computes:

```
hash = SHA256(domain || subscription_id || raw_key)
```

where `domain` is a 4-byte big-endian `u32`, `subscription_id` is a 4-byte big-endian `u32`, and `raw_key` is the 32-byte caller-supplied value. This ensures:

- The same raw key used on two different entrypoints produces different on-chain fingerprints.
- The same raw key used on two different subscriptions produces different fingerprints.
- No raw key material is visible in storage to indexers.

The three entrypoint domain values are pairwise distinct (`5`, `6`, `7`) and are asserted as unique by `test_domain_constants_are_unique` in `src/nonce.rs`.

#### Ring buffer

- Hashes are stored in an `IdemRingBuffer` struct capped at `IDEM_HISTORY = 64` entries per subscription.
- Each entry stores `(hash, inserted_at_timestamp)`. On lookup, entries older than the configured TTL are skipped and treated as absent, regardless of ring position.
- The admin configures the contract-wide TTL in seconds with `set_idempotency_ttl`; zero is rejected. It defaults to `IDEM_TTL_SECS = 7 days`, and `get_idempotency_ttl` returns the active value.
- When a new key is written, entries at or beyond the TTL are removed from that subscription's stored buffer. If the buffer is still full, its oldest live entry is evicted.
- **Storage**: One `IdemRingBuffer` per subscription (key: `DataKey::IdemKey(subscription_id)`).
- **Combined guarantee**: An attacker must submit 64 distinct charges to the same subscription within the configured TTL to cycle out a single hash. Choose a TTL that matches the application's retry policy.

### Batch charge

- `batch_charge(subscription_ids)` does **not** take idempotency keys. Each subscription is charged with period-based replay protection only. Duplicate IDs in the list are processed independently (each may succeed or fail per period/balance/interval).

### Recovery operation namespace

`recover_stranded_funds` uses a **completely separate** replay-protection namespace from all charge-path idempotency keys:

| Property | Charge-path idem keys | Recovery keys |
|---|---|---|
| Storage key | `DataKey::IdemKey(subscription_id)` | `DataKey::Recovery(recovery_id)` |
| Value type | `IdemRingBuffer` (ring of hashes + timestamps) | `bool` (flag) |
| Lookup | Ring scan with TTL check | Direct key presence (`has`) |
| Key format | `SHA256(domain_u32 \|\| sub_id_u32 \|\| raw_32bytes)` | Caller-supplied `String` |
| Discriminant | 8 (instance tier) | 15 (persistent tier) |

These are different `DataKey` enum variants with different on-chain discriminants. A raw 32-byte value used as a `charge_subscription` idempotency key and a `recovery_id` string derived from the same bytes exist in entirely independent storage slots. Neither can block the other:

- Consuming a charge idem key does not affect the `DataKey::Recovery` namespace.
- Consuming a `recovery_id` does not affect any subscription's `DataKey::IdemKey` ring buffer.

This separation is enforced by the type system (different `DataKey` variants) and verified by `test_charge_and_recovery_keys_are_namespace_separated` in `src/test_idempotency_keys.rs`.

## Integrator responsibilities

1. **Use one idempotency key per billing event.** For a given subscription and billing period, use a single stable key (e.g. derived from `subscription_id` + period start or from your job id). Retries with the same key are safe; using a new key for the same period will be rejected as `Replay` once the period was already charged.

2. **Do not reuse keys across periods or entrypoints.** Use a new key for each new billing period so that the next charge is not mistaken for a replay of the previous period. The domain-separated hashing scheme prevents cross-entrypoint collisions, but best practice is still to use unique keys per event.

3. **Handle `Error::Replay`.** If you receive `Replay`, the charge for that period was already applied (by this or a previous request). Treat as success for reporting; do not retry with a different key for the same period.

4. **Handle idempotent no-op.** If you receive `Ok` but did not observe the corresponding on-chain event (e.g. your indexer missed it), the operation still succeeded. The contract does not re-emit events on idempotent matches; verify against the ring buffer off-chain if needed.

5. **Optional but recommended:** Persist idempotency keys in your billing engine (e.g. per subscription and period) so that retries use the same key.

6. **Retry window.** Idempotency entries expire after the configured duration (7 days by default). Retries must use the same key within that window. After expiry or after 64 newer operations on the same subscription, the key may be processed as a fresh operation.

## Required parameters and behavior (Rustdoc summary)

- **`charge_subscription(env, subscription_id, idem_key)`**
  - `idem_key`: `Option<BytesN<32>>`. Use `Some(key)` for safe retries; use `None` for period-only protection.
  - Returns `Ok(ChargeExecutionResult::Charged)` on success or idempotent match (same key already processed).
  - Returns `Err(Error::Replay)` if this billing period was already charged (and the call did not match a stored idempotency key).

- **`deposit_funds(env, subscription_id, subscriber, amount, idem_key)`**
  - `idem_key`: `Option<BytesN<32>>`. Use `Some(key)` for safe retries.
  - Returns `Ok(())` on success or idempotent match.

- **`charge_one_off(env, subscription_id, merchant, amount, idem_key)`**
  - `idem_key`: `Option<BytesN<32>>`. Use `Some(key)` for safe retries.
  - Returns `Ok(())` on success or idempotent match.

## Residual risks and mitigations

- **Clock skew / timestamp manipulation:** Period is derived from ledger timestamp. Validators set ledger time; contract does not rely on caller-provided time. Mitigation: trust the network's ledger timestamp.
- **Unbounded growth:** Only one period index and one `IdemRingBuffer` (≤ 64 entries × (32 + 8) bytes = ~2,560 bytes per subscription) are stored. No unbounded growth from replay protection.
- **Key collision:** If an integrator reuses the same 32-byte key for two different billing periods on the same entrypoint, the second period's charge would be treated as idempotent (return Ok without charging). Mitigation: derive keys from period (e.g. include period start or index in the key).
- **Ring buffer eviction:** A retry delayed beyond the configured TTL (or after 64 newer operations on the same subscription) will miss the ring buffer and execute as a fresh charge. Use `None` or keep retries within the TTL window.
- **Cross-entrypoint safety:** Domain separation in the hash prevents the same raw key from replaying across `charge_subscription`, `deposit_funds`, and `charge_one_off`.

---

## Admin-operation nonce scheme

Privileged admin operations (`batch_charge` and `rotate_admin`) carry an additional layer of replay protection through an explicit, domain-separated, monotonic nonce scheme.

### Design

| Property | Value |
|---|---|
| Nonce type | `u64` (unsigned, monotonic) |
| Per-signer | One counter per `(signer: Address, domain: u32)` pair |
| Storage | Persistent storage, key `DataKey::AdminNonce(Address, u32)` |
| Initial value | `0` (absent key treated as `0`) |
| Enforcement | Caller provides the *current* stored value; contract checks equality, then atomically increments |
| Error on mismatch | `Error::NonceAlreadyUsed` (code `1038`) |

### Domain constants

```rust
pub const DOMAIN_BATCH_CHARGE: u32 = 0;   // label: "batch"
pub const DOMAIN_ADMIN_ROTATION: u32 = 1;  // label: "adm_rot"
```

Domain separation ensures that a nonce consumed in one operation cannot interfere with another. The labels appear in the emitted event topic so indexers can filter by domain.

### Nonce consumption flow

```
caller → batch_charge(ids, nonce)
  1. require_stored_admin_auth()   // auth check first – fails fast on wrong signer
  2. check_and_advance(admin, DOMAIN_BATCH_CHARGE, nonce)
        a. read stored nonce (default 0)
        b. assert provided == stored  → Error::NonceAlreadyUsed if not
        c. write stored + 1
        d. emit NonceConsumedEvent
  3. … rest of charge logic
```

### Emitted event

Every successful nonce consumption emits a `NonceConsumedEvent`:

```rust
pub struct NonceConsumedEvent {
    pub signer:    Address,  // the admin address that consumed the nonce
    pub domain:    u32,      // DOMAIN_BATCH_CHARGE or DOMAIN_ADMIN_ROTATION
    pub nonce:     u64,      // the consumed (previous) nonce value
    pub timestamp: u64,      // ledger timestamp at consumption
}
```

Event topic: `("nonce_consumed", signer, domain_label)` where `domain_label` is the human-readable symbol (`"batch"` or `"adm_rot"`).

### Off-chain integration

Use `get_admin_nonce(signer, domain) -> u64` to read the expected nonce before submitting a transaction:

```rust
// Pseudocode
let next_nonce = client.get_admin_nonce(&admin, DOMAIN_BATCH_CHARGE);
client.batch_charge(&subscription_ids, &next_nonce);
```

To prevent races, integrate this with a serialised job queue or use optimistic concurrency: if `NonceAlreadyUsed` is returned, re-read the nonce and retry.

### TTL expiry race window

> **Important:** This section describes a scenario where the nonce storage key expires between transaction submission and ledger inclusion. Integrators must account for this to avoid losing replay protection.

#### How Soroban persistent storage TTL works

Every entry in Soroban persistent storage has a ledger-based time-to-live (TTL). When a key's TTL reaches zero, the entry is **evicted**. A subsequent read returns `None`, which `check_and_advance` treats as `unwrap_or(0)` — the nonce silently resets to `0`.

`AdminNonce` entries are written by `check_and_advance` but **no `extend_ttl` call is made on the nonce key at write time**. The TTL of each nonce entry is therefore determined entirely by the network's minimum persistent entry TTL at the time of the write, and is not renewed by subsequent nonce reads (e.g. `get_admin_nonce`).

#### The race window

```
time ──────────────────────────────────────────────────────────────▶
          T0                    T1                 T2
          │                     │                  │
   tx submitted          nonce key TTL         tx included
  (nonce = N read,        reaches 0             in ledger
   tx built with          (key evicted,         (nonce read
   max_time = T2)          stored = 0)           returns 0 ✓)
```

1. At `T0`, the off-chain caller reads `nonce = N` via `get_admin_nonce`.
2. A transaction is built carrying `nonce = N` and submitted with `max_time = T2`.
3. Before the transaction is included in a ledger, the `AdminNonce` key's TTL expires at `T1`. The key is evicted; stored value is effectively `0`.
4. At `T2` the transaction is included. `check_and_advance` reads `0`, the provided nonce is `N`. If `N = 0` the check passes and the operation executes. If `N > 0` the check fails with `NonceAlreadyUsed`, but the *next* transaction built with `nonce = 0` would succeed — replay protection is broken for that domain until the counter advances past `0` again.

The critical consequence is that **an attacker who captured a prior transaction carrying `nonce = 0` (the very first operation ever submitted for that domain) can replay it after the key has been evicted**, because the stored value resets to `0` and the check passes.

#### Recommendation: set `max_time` shorter than the nonce key TTL

The simplest mitigation is to ensure the transaction's validity window is always shorter than the remaining TTL of the nonce key:

```
max_time - now  <<  remaining_TTL(AdminNonce key)
```

**Practical guidance:**

- Soroban persistent entries have a network-configured minimum TTL (currently 4,096 ledgers on Mainnet ≈ ~5.7 hours at 5 s/ledger). After a nonce write, the key lives for at least this long.
- Set `max_time` to **no more than 30 minutes** in the future. This gives the transaction plenty of inclusion time (Stellar's typical inclusion time is seconds to minutes) while staying well within the ~5.7-hour minimum TTL.
- If your integration requires longer validity windows, call `extend_ttl` on the `AdminNonce` key before or immediately after writing, bumping it to a value longer than your maximum intended `max_time`.

| Parameter | Recommended value | Rationale |
|---|---|---|
| `max_time` (tx validity) | ≤ 30 minutes | Short enough that any reasonable TTL outlasts it |
| Nonce key TTL | ≥ 2× `max_time` | Leaves margin for clock skew and network delays |
| TTL bump (if needed) | Target ≥ 24 hours | Covers operational retries and maintenance windows |

#### What happens if the race is hit

- If nonce `N > 0` at the time of eviction, the in-flight transaction fails with `NonceAlreadyUsed` (because stored resets to `0`, not `N`). No double-execution occurs, but the transaction must be rebuilt with `nonce = 0`.
- If nonce `N = 0` (first-ever operation for that signer/domain) and the key is evicted then the transaction is included, the operation executes normally. A replay of the same transaction would now be blocked because the stored nonce is `1`.
- The most dangerous case is if `N = 0`, the operation executed, the key later expires, and a captured copy of the original `nonce = 0` transaction is resubmitted. To prevent this, either keep the TTL alive (above) or accept that `nonce = 0` operations carry slightly elevated replay risk and use short `max_time` windows.

### Security properties

| Threat | Mitigation |
|---|---|
| Cross-ledger replay | Nonce is monotonic; replaying any past transaction fails with `NonceAlreadyUsed` |
| Out-of-order submission | Only the exact stored value is accepted; skipping nonce values is rejected |
| Cross-domain replay | Domain tag is part of storage key; batch_charge nonce and rotate_admin nonce are fully independent |
| Cross-signer replay | Signer address is part of storage key; each admin has its own counter |
| Nonce overflow | `checked_add(1)` panics (transaction aborted) rather than wrapping to 0 |
| Auth bypass via nonce manipulation | Auth check (`require_admin_auth`) runs *before* nonce check; invalid signers are rejected without advancing any counter |
| TTL expiry mid-flight | Keep `max_time` ≤ 30 min and/or extend nonce key TTL to outlast the validity window (see TTL expiry race window above) |

### Storage layout

```
Persistent storage:
  DataKey::AdminNonce(Address, 0) → u64   (batch_charge nonce for address)
  DataKey::AdminNonce(Address, 1) → u64   (rotate_admin nonce for address)
```

Nonce entries are stored in **persistent** storage so they survive ledger TTL extension and contract upgrades. Growth is bounded: one `u64` entry per `(signer, domain)` pair. In practice this means at most two entries per admin address (one per domain).