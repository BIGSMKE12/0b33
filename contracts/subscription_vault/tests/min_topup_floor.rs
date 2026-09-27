//! Integration tests for the `min_topup` floor (`admin.rs` / `admin_api.rs` —
//! issue #195 / `#155`).
//!
//! `set_min_topup` must refuse a non-positive value with `Error::InvalidAmount`
//! (3001). A `min_topup` of `0` would turn the `BelowMinimumTopup` (5003) guard
//! in every deposit path into a no-op, letting dust deposits through. This file
//! pins both halves of that invariant:
//!
//! 1. the floor can never be *stored* as a non-positive number — neither through
//!    `init` nor through `set_min_topup`, and a rejected update leaves the
//!    previously stored floor (and the config cooldown) untouched; and
//! 2. the floor that *is* stored is actually enforced by the deposit path, on
//!    the boundary (`amount == min_topup` settles, one unit less does not), and
//!    after the floor is raised — i.e. the exact "dust deposit" scenario in the
//!    issue.
//!
//! `src/test_require_auth.rs` has unit tests for (1), but it is never
//! `mod`-declared in `lib.rs` — the only declared test modules are
//! `test_datakey_layout` and `test_merchant_tags` — so `cargo test --all` never
//! compiles or runs it. These repeat that ground as runnable integration tests
//! and add (2), which had no coverage in any test that actually executes.

#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token, Address, Env, Symbol,
};
use subscription_vault::{Error, SubscriptionVault, SubscriptionVaultClient};

/// Ledger time every test starts from.
const T0: u64 = 1_000_000;
/// Comfortably above `MIN_SUBSCRIPTION_INTERVAL_SECONDS` (60).
const INTERVAL: u64 = 30 * 24 * 60 * 60;
/// The floor `init` is seeded with.
const SEED_MIN_TOPUP: i128 = 1_000_000;
/// A fresh subscriber starts with this much spendable balance.
const SUBSCRIBER_FUNDS: i128 = 100_000_000;

// -- Helpers ------------------------------------------------------------------

fn setup() -> (
    Env,
    SubscriptionVaultClient<'static>,
    token::StellarAssetClient<'static>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let asset = env.register_stellar_asset_contract_v2(admin.clone());
    let token_admin = token::StellarAssetClient::new(&env, &asset.address());

    client.init(
        &asset.address(),
        &6,
        &admin,
        &SEED_MIN_TOPUP,
        &(7 * 24 * 60 * 60),
    );

    (env, client, token_admin, admin)
}

/// Register a funded subscriber and give them a subscription.
fn funded_subscription(
    env: &Env,
    client: &SubscriptionVaultClient,
    token_admin: &token::StellarAssetClient,
) -> (u32, Address, Address) {
    let subscriber = Address::generate(env);
    let merchant = Address::generate(env);
    token_admin.mint(&subscriber, &SUBSCRIBER_FUNDS);

    let sub_id = client.create_subscription(
        &subscriber,
        &merchant,
        &SEED_MIN_TOPUP,
        &INTERVAL,
        &false,
        &None::<i128>,   // lifetime_cap
        &None::<u64>,    // expires_at
        &None::<u32>,    // expires_at_ledger
        &None::<Symbol>, // sub_account_label
        &false,          // proration_enabled
    );

    (sub_id, subscriber, merchant)
}

fn balance(client: &SubscriptionVaultClient, sub_id: u32) -> i128 {
    client.get_subscription(&sub_id).prepaid_balance
}

// -- 1. The floor can never be stored as a non-positive number ----------------

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // Error::InvalidAmount
fn init_rejects_a_zero_floor() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let asset = env.register_stellar_asset_contract_v2(admin.clone());

    client.init(&asset.address(), &6, &admin, &0i128, &(7 * 24 * 60 * 60));
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // Error::InvalidAmount
fn init_rejects_a_negative_floor() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let asset = env.register_stellar_asset_contract_v2(admin.clone());

    client.init(&asset.address(), &6, &admin, &-1i128, &(7 * 24 * 60 * 60));
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // Error::InvalidAmount
fn set_min_topup_rejects_zero() {
    let (_env, client, _token_admin, admin) = setup();

    client.set_min_topup(&admin, &0i128);
}

#[test]
#[should_panic(expected = "Error(Contract, #3001)")] // Error::InvalidAmount
fn set_min_topup_rejects_a_negative_value() {
    let (_env, client, _token_admin, admin) = setup();

    client.set_min_topup(&admin, &-1i128);
}

#[test]
fn set_min_topup_rejects_any_non_positive_value() {
    let (_env, client, _token_admin, admin) = setup();

    for bad in [0i128, -1, -1_000_000, i128::MIN] {
        let res = client.try_set_min_topup(&admin, &bad);
        assert!(res.is_err(), "min_topup = {bad} must be rejected");
    }
}

#[test]
fn set_min_topup_accepts_the_smallest_positive_value() {
    let (_env, client, _token_admin, admin) = setup();

    let res = client.try_set_min_topup(&admin, &1i128);
    assert!(res.is_ok(), "min_topup = 1 is positive and must be accepted");
    assert_eq!(client.get_min_topup(), 1i128);
}

#[test]
fn a_rejected_update_leaves_the_stored_floor_unchanged() {
    let (_env, client, _token_admin, admin) = setup();
    let before = client.get_min_topup();
    assert_eq!(before, SEED_MIN_TOPUP);

    let res = client.try_set_min_topup(&admin, &0i128);
    assert!(res.is_err());

    assert_eq!(
        client.get_min_topup(),
        before,
        "a rejected floor update must not be persisted"
    );
}

#[test]
fn a_rejected_update_does_not_consume_the_config_cooldown() {
    let (env, client, _token_admin, admin) = setup();
    env.ledger().set_timestamp(T0);

    let rejected = client.try_set_min_topup(&admin, &0i128);
    assert!(rejected.is_err());

    // Because the validation fails before `enforce_config_cooldown` runs, the
    // very next (valid) update at the same ledger time must still go through.
    let accepted = client.try_set_min_topup(&admin, &2_000_000i128);
    assert!(
        accepted.is_ok(),
        "a rejected update must not start the 6h cooldown"
    );
    assert_eq!(client.get_min_topup(), 2_000_000i128);
}

// -- 2. The stored floor is enforced by the deposit path ---------------------

#[test]
#[should_panic(expected = "Error(Contract, #5003)")] // Error::BelowMinimumTopup
fn a_dust_deposit_below_the_floor_is_rejected() {
    let (env, client, token_admin, _admin) = setup();
    let (sub_id, _subscriber, _merchant) = funded_subscription(&env, &client, &token_admin);

    client.deposit_funds(&sub_id, &1i128, &None::<u64>);
}

#[test]
#[should_panic(expected = "Error(Contract, #5003)")] // Error::BelowMinimumTopup
fn a_deposit_one_unit_below_the_floor_is_rejected() {
    let (env, client, token_admin, _admin) = setup();
    let (sub_id, _subscriber, _merchant) = funded_subscription(&env, &client, &token_admin);

    client.deposit_funds(&sub_id, &(SEED_MIN_TOPUP - 1), &None::<u64>);
}

#[test]
fn a_deposit_exactly_at_the_floor_is_accepted() {
    let (env, client, token_admin, _admin) = setup();
    let (sub_id, _subscriber, _merchant) = funded_subscription(&env, &client, &token_admin);

    client.deposit_funds(&sub_id, &SEED_MIN_TOPUP, &None::<u64>);

    assert_eq!(balance(&client, sub_id), SEED_MIN_TOPUP);
}

#[test]
fn a_deposit_above_the_floor_is_accepted() {
    let (env, client, token_admin, _admin) = setup();
    let (sub_id, _subscriber, _merchant) = funded_subscription(&env, &client, &token_admin);

    client.deposit_funds(&sub_id, &(SEED_MIN_TOPUP * 3), &None::<u64>);

    assert_eq!(balance(&client, sub_id), SEED_MIN_TOPUP * 3);
}

#[test]
fn raising_the_floor_rejects_a_previously_acceptable_dust_deposit() {
    // This is the scenario the issue describes: a deposit that the old floor
    // allowed must stop being acceptable as soon as the floor is raised, and it
    // must not silently move funds when it is refused.
    let (env, client, token_admin, admin) = setup();
    let (sub_id, _subscriber, _merchant) = funded_subscription(&env, &client, &token_admin);

    // 2_000_000 clears the seeded 1_000_000 floor, so it settles.
    client.deposit_funds(&sub_id, &2_000_000i128, &None::<u64>);
    assert_eq!(balance(&client, sub_id), 2_000_000i128);

    // Raise the floor above that amount.
    client.set_min_topup(&admin, &5_000_000i128);
    assert_eq!(client.get_min_topup(), 5_000_000i128);

    // The very same deposit is now dust and must be refused.
    let refused = client.try_deposit_funds(&sub_id, &2_000_000i128, &None::<u64>);
    assert!(
        refused.is_err(),
        "a deposit below the raised floor must be rejected"
    );

    assert_eq!(
        balance(&client, sub_id),
        2_000_000i128,
        "a rejected deposit must not move funds"
    );
}

#[test]
fn the_smallest_allowed_floor_still_guards_the_boundary() {
    let (env, client, token_admin, admin) = setup();
    let (sub_id, _subscriber, _merchant) = funded_subscription(&env, &client, &token_admin);

    client.set_min_topup(&admin, &1i128);
    assert_eq!(client.get_min_topup(), 1i128);

    // Zero is below a floor of 1 ...
    let refused = client.try_deposit_funds(&sub_id, &0i128, &None::<u64>);
    assert!(refused.is_err());

    // ... and exactly 1 clears it.
    client.deposit_funds(&sub_id, &1i128, &None::<u64>);
    assert_eq!(balance(&client, sub_id), 1i128);
}

#[test]
fn lowering_the_floor_makes_a_larger_range_of_deposits_acceptable() {
    let (env, client, token_admin, admin) = setup();
    let (sub_id, _subscriber, _merchant) = funded_subscription(&env, &client, &token_admin);

    // 1_000 is below the seeded floor ...
    let refused = client.try_deposit_funds(&sub_id, &1_000i128, &None::<u64>);
    assert!(refused.is_err());

    // ... but acceptable once the floor is lowered to 1.
    client.set_min_topup(&admin, &1i128);
    client.deposit_funds(&sub_id, &1_000i128, &None::<u64>);

    assert_eq!(balance(&client, sub_id), 1_000i128);
}

// -- Sanity: the codes these tests pin ---------------------------------------

#[test]
fn floor_error_codes_are_stable() {
    assert_eq!(Error::InvalidAmount as u32, 3_001);
    assert_eq!(Error::BelowMinimumTopup as u32, 5_003);
}
