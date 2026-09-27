//! Integration tests for the expiry window on the two-step admin rotation
//! proposal (`admin.rs` — issue #194 / `#154`).
//!
//! `propose_admin` stores an `AdminProposal { new_admin, proposed_at, expires_at }`
//! with `expires_at = proposed_at + PROPOSAL_WINDOW_SECS` (7 days), and
//! `claim_admin_role` must refuse a proposal once that window has elapsed. This
//! file pins the whole boundary:
//!
//! * the window that actually gets stored (and is announced in the event);
//! * the exact claim boundary — `now == expires_at` is still claimable,
//!   `now > expires_at` is not;
//! * the stale proposal being dropped when an expired claim is attempted;
//! * an expired proposal no longer blocking `propose_admin`, so the admin can
//!   rotate again without having to call `cancel_admin_proposal` first. That is
//!   the behaviour `docs/admin_rotation.md` has always documented as
//!   "…until the first is claimed, cancelled, **or expires**", but which the
//!   implementation did not deliver: `propose_admin` returned
//!   `ProposalAlreadyExists` for any stored proposal, expired or not, which left
//!   a stale proposal blocking rotation indefinitely.
//!
//! These live under `tests/` rather than next to the contract because
//! `src/test_admin_rotation_two_step.rs` is never `mod`-declared in `lib.rs`
//! (the only two declared test modules are `test_datakey_layout` and
//! `test_merchant_tags`), so `cargo test --all` never compiles it. Anything
//! added there would never run.

#![cfg(test)]

extern crate alloc;

use soroban_sdk::{
    testutils::{Address as _, Events, Ledger as _},
    Address, Env, IntoVal, Symbol, TryFromVal, Val, Vec,
};
use subscription_vault::{
    AdminProposalCreatedEvent, Error, SubscriptionVault, SubscriptionVaultClient,
};

/// Ledger time every test starts from.
const T0: u64 = 1_000_000;
/// `PROPOSAL_WINDOW_SECS` — how long a pending proposal stays claimable.
const WINDOW: u64 = 7 * 24 * 60 * 60;
/// `ADMIN_PROPOSAL_COOLDOWN_SECS` — minimum age before a proposal can be claimed.
const COOLDOWN: u64 = 24 * 60 * 60;

// `PROPOSAL_WINDOW_SECS` and `ADMIN_PROPOSAL_COOLDOWN_SECS` are private to
// `admin.rs`, so the code values are pinned here to make a silent drift visible.
const EXPECTED_WINDOW: u64 = 604_800;
const EXPECTED_COOLDOWN: u64 = 86_400;

// -- Helpers ------------------------------------------------------------------

fn setup() -> (Env, SubscriptionVaultClient<'static>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    (env, client, admin)
}

fn advance(env: &Env, seconds: u64) {
    let now = env.ledger().timestamp();
    env.ledger().set_timestamp(now + seconds);
}

/// Decode the payload of the first event whose leading topic is `name`.
fn event_payload(env: &Env, name: &str) -> Option<Val> {
    let target: Val = Symbol::new(env, name).into_val(env);
    let all = env.events().all();
    for i in 0..all.len() {
        let (_, topics, data): (Address, Vec<Val>, Val) = all.get(i).unwrap();
        if topics.get(0) == Some(target.clone()) {
            return Some(data);
        }
    }
    None
}

// -- Constants ----------------------------------------------------------------

#[test]
fn proposal_window_and_cooldown_are_the_documented_durations() {
    assert_eq!(WINDOW, EXPECTED_WINDOW, "7 days");
    assert_eq!(COOLDOWN, EXPECTED_COOLDOWN, "24 hours");
    assert!(COOLDOWN < WINDOW, "a proposal must be claimable before it expires");
}

// -- The window that gets stored ----------------------------------------------

#[test]
fn proposal_stores_a_seven_day_expiry_window() {
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);

    let p = client
        .get_admin_proposal()
        .expect("a proposal must be stored");
    assert_eq!(p.new_admin, new_admin);
    assert_eq!(p.proposed_at, T0);
    assert_eq!(p.expires_at, T0 + WINDOW);
}

#[test]
fn created_event_announces_the_expiry() {
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);

    let payload = event_payload(&env, "admin_proposal_created").expect("event must be emitted");
    let parsed = AdminProposalCreatedEvent::try_from_val(&env, &payload).unwrap();
    assert_eq!(parsed.old_admin, admin);
    assert_eq!(parsed.new_admin, new_admin);
    assert_eq!(parsed.timestamp, T0);
    assert_eq!(parsed.expires_at, T0 + WINDOW);
}

// -- Claim boundary -----------------------------------------------------------

#[test]
fn claim_is_accepted_inside_the_window() {
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    advance(&env, COOLDOWN);

    client.claim_admin_role(&new_admin);

    assert_eq!(client.get_admin(), new_admin);
    assert!(client.get_admin_proposal().is_none(), "claim consumes the proposal");
}

#[test]
fn claim_at_the_exact_expiry_instant_is_accepted() {
    // `expires_at` is inclusive: the guard only rejects on `now > expires_at`.
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    advance(&env, WINDOW); // now == expires_at

    client.claim_admin_role(&new_admin);

    assert_eq!(client.get_admin(), new_admin);
}

#[test]
#[should_panic(expected = "Error(Contract, #14002)")] // Error::ProposalExpired
fn claim_one_second_after_expiry_is_rejected() {
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    advance(&env, WINDOW + 1);

    client.claim_admin_role(&new_admin);
}

#[test]
fn an_expired_claim_does_not_rotate_the_admin() {
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    advance(&env, WINDOW + 1);

    let res = client.try_claim_admin_role(&new_admin);
    assert!(res.is_err(), "a claim past the window must be refused");

    assert_eq!(client.get_admin(), admin, "the stored admin must not change");
}

#[test]
fn an_expired_claim_clears_the_stale_proposal() {
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    advance(&env, WINDOW + 1);
    let _ = client.try_claim_admin_role(&new_admin);

    assert!(
        client.get_admin_proposal().is_none(),
        "the rejected claim must drop the stale proposal"
    );
}

#[test]
fn reading_does_not_clean_up_an_expired_proposal() {
    let (env, client, admin) = setup();
    let new_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    advance(&env, WINDOW + 1);

    // `get_admin_proposal` is a pure read: it keeps reporting the stale entry.
    let p = client
        .get_admin_proposal()
        .expect("still readable after expiry");
    assert_eq!(p.new_admin, new_admin);
    assert_eq!(p.expires_at, T0 + WINDOW);
}

// -- Re-proposing: an expired proposal must not lock the admin out ------------

#[test]
#[should_panic(expected = "Error(Contract, #14004)")] // Error::ProposalAlreadyExists
fn propose_is_blocked_while_the_window_is_still_open() {
    let (env, client, admin) = setup();
    let first = Address::generate(&env);
    let second = Address::generate(&env);

    client.propose_admin(&admin, &first);
    advance(&env, WINDOW - 1); // one second short of expiry

    client.propose_admin(&admin, &second);
}

#[test]
#[should_panic(expected = "Error(Contract, #14004)")] // Error::ProposalAlreadyExists
fn propose_is_still_blocked_at_the_exact_expiry_instant() {
    let (env, client, admin) = setup();
    let first = Address::generate(&env);
    let second = Address::generate(&env);

    client.propose_admin(&admin, &first);
    advance(&env, WINDOW); // now == expires_at, so still claimable, so still blocking

    client.propose_admin(&admin, &second);
}

#[test]
fn propose_succeeds_once_the_previous_proposal_has_expired() {
    let (env, client, admin) = setup();
    let stale = Address::generate(&env);
    let fresh = Address::generate(&env);

    client.propose_admin(&admin, &stale);
    advance(&env, WINDOW + 1);

    let res = client.try_propose_admin(&admin, &fresh);
    assert!(
        res.is_ok(),
        "an expired proposal must not block a replacement"
    );
}

#[test]
fn replacing_an_expired_proposal_stores_the_new_claimant_and_refreshes_the_window() {
    let (env, client, admin) = setup();
    let stale = Address::generate(&env);
    let fresh = Address::generate(&env);

    client.propose_admin(&admin, &stale);
    advance(&env, WINDOW + 1);
    client.propose_admin(&admin, &fresh);

    let p = client
        .get_admin_proposal()
        .expect("the replacement must be stored");
    assert_eq!(p.new_admin, fresh, "the stale claimant must be gone");
    assert_eq!(p.proposed_at, T0 + WINDOW + 1);
    assert_eq!(p.expires_at, T0 + WINDOW + 1 + WINDOW);
}

#[test]
fn replacing_an_expired_proposal_emits_a_fresh_created_event() {
    let (env, client, admin) = setup();
    let stale = Address::generate(&env);
    let fresh = Address::generate(&env);

    client.propose_admin(&admin, &stale);
    advance(&env, WINDOW + 1);
    client.propose_admin(&admin, &fresh);

    let payload = event_payload(&env, "admin_proposal_created").expect("event must be emitted");
    let parsed = AdminProposalCreatedEvent::try_from_val(&env, &payload).unwrap();
    assert_eq!(parsed.old_admin, admin);
    assert_eq!(parsed.new_admin, fresh);
    assert_eq!(parsed.timestamp, T0 + WINDOW + 1);
    assert_eq!(parsed.expires_at, T0 + WINDOW + 1 + WINDOW);
}

#[test]
#[should_panic(expected = "Error(Contract, #14003)")] // Error::InvalidClaimant
fn the_stale_claimant_cannot_claim_the_replacement() {
    let (env, client, admin) = setup();
    let stale = Address::generate(&env);
    let fresh = Address::generate(&env);

    client.propose_admin(&admin, &stale);
    advance(&env, WINDOW + 1);
    client.propose_admin(&admin, &fresh);
    advance(&env, COOLDOWN); // the replacement's own cooldown must have elapsed

    client.claim_admin_role(&stale);
}

#[test]
fn the_replacement_can_be_claimed_and_completes_the_rotation() {
    let (env, client, admin) = setup();
    let stale = Address::generate(&env);
    let fresh = Address::generate(&env);

    client.propose_admin(&admin, &stale);
    advance(&env, WINDOW + 1);
    client.propose_admin(&admin, &fresh);
    advance(&env, COOLDOWN);

    client.claim_admin_role(&fresh);

    assert_eq!(client.get_admin(), fresh);
}

#[test]
fn a_claim_that_already_cleared_the_expired_proposal_lets_a_new_one_through() {
    let (env, client, admin) = setup();
    let stale = Address::generate(&env);
    let fresh = Address::generate(&env);

    client.propose_admin(&admin, &stale);
    advance(&env, WINDOW + 1);
    let _ = client.try_claim_admin_role(&stale); // expiry path removes the proposal

    let res = client.try_propose_admin(&admin, &fresh);
    assert!(res.is_ok());
    assert_eq!(client.get_admin_proposal().unwrap().new_admin, fresh);
}

// -- Cancellation still clears the way (regression) ---------------------------

#[test]
fn cancelling_then_reproposing_still_works() {
    let (env, client, admin) = setup();
    let first = Address::generate(&env);
    let second = Address::generate(&env);

    client.propose_admin(&admin, &first);
    client.cancel_admin_proposal(&admin);

    let res = client.try_propose_admin(&admin, &second);
    assert!(res.is_ok());
    assert_eq!(client.get_admin_proposal().unwrap().new_admin, second);
}

#[test]
fn a_cancelled_proposal_is_removed() {
    let (env, client, admin) = setup();
    let first = Address::generate(&env);

    client.propose_admin(&admin, &first);
    client.cancel_admin_proposal(&admin);

    assert!(client.get_admin_proposal().is_none());
}

// -- Sanity: the error codes these tests pin --------------------------------

#[test]
fn proposal_error_codes_are_stable() {
    assert_eq!(Error::InvalidClaimant as u32, 14_003);
    assert_eq!(Error::ProposalExpired as u32, 14_002);
    assert_eq!(Error::ProposalAlreadyExists as u32, 14_004);
}
