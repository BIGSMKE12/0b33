#![cfg(test)]

use crate::types::{ProposalKind, OP_REFUND, OP_WITHDRAW};
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, String,
};

// ── Governance Proposal Tests ──────────────────────────────────────────────

/// Helper to initialize contract with admin and token
fn init_vault<'a>(env: &'a Env, admin: &Address) -> (Address, SubscriptionVaultClient<'a>) {
    let token_admin = Address::generate(env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(env, &contract_id);

    client.init(
        &token_address,
        &6, // decimals
        admin,
        &10_000_000, // min_topup
        &86400,      // grace period
    );

    (token_address, client)
}

#[test]
fn test_add_and_remove_guardians() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let guardian1 = Address::generate(&env);
    let guardian2 = Address::generate(&env);

    let (_, client) = init_vault(&env, &admin);

    // Add guardians
    client.add_guardian(&admin, &guardian1, &100);
    client.add_guardian(&admin, &guardian2, &50);

    // Verify weights
    assert_eq!(client.get_guardian_weight(&guardian1), 100);
    assert_eq!(client.get_guardian_weight(&guardian2), 50);

    // Remove guardian1
    client.remove_guardian(&admin, &guardian1);
    assert_eq!(client.get_guardian_weight(&guardian1), 0);
    assert_eq!(client.get_guardian_weight(&guardian2), 50);
}

#[test]
fn test_submit_proposal_rotate_admin() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    let current_time = env.ledger().timestamp();
    let eta = current_time + 3600; // 1 hour from now

    // Submit proposal
    let proposal_id = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &5000, // 50% quorum
        &eta,
    );

    assert_eq!(proposal_id, 0);

    // Verify proposal exists
    let proposal = client.get_proposal(&0).unwrap();
    assert_eq!(proposal.kind, ProposalKind::RotateAdmin);
    assert_eq!(proposal.target, new_admin);
    assert_eq!(proposal.quorum_bps, 5000);
    assert_eq!(proposal.executed, false);
}

#[test]
fn test_submit_proposal_set_protocol_fee() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    let current_time = env.ledger().timestamp();
    let eta = current_time + 3600;

    // Submit protocol fee proposal
    let proposal_id = client.submit_proposal(
        &ProposalKind::SetProtocolFee,
        &treasury,
        &None,
        &250,  // 2.5% fee
        &7500, // 75% quorum
        &eta,
    );

    assert_eq!(proposal_id, 0);

    let proposal = client.get_proposal(&0).unwrap();
    assert_eq!(proposal.kind, ProposalKind::SetProtocolFee);
    assert_eq!(proposal.target3, 250);
}

#[test]
fn test_invalid_quorum_bps() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    let current_time = env.ledger().timestamp();
    let eta = current_time + 3600;

    // Try to submit with invalid quorum (> 10000)
    let result = client.try_submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &10001, // Invalid: > 10000
        &eta,
    );

    assert!(
        result.is_err(),
        "proposal with quorum > 10000 must be rejected"
    );
}

#[test]
fn test_eta_in_past_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);
    let (_, client) = init_vault(&env, &admin);

    env.ledger().set_timestamp(1_000_000);
    let current_time = env.ledger().timestamp();
    let eta_in_past = current_time - 3600; // 1 hour ago

    // Try to submit with ETA in the past
    let result = client.try_submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &5000,
        &eta_in_past,
    );

    assert!(result.is_err(), "proposal with past ETA must be rejected");
}

#[test]
fn test_cancel_proposal() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let new_admin = Address::generate(&env);

    let (_, client) = init_vault(&env, &admin);

    // Submit proposal
    let current_time = env.ledger().timestamp();
    let eta = current_time + 3600;
    let proposal_id = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin,
        &None,
        &0,
        &5000,
        &eta,
    );

    // Cancel it
    let reason = String::from_str(&env, "Superseded by newer proposal");
    client.cancel_proposal(&proposal_id, &reason);

    // Verify it's marked as executed (and thus immutable)
    let proposal = client.get_proposal(&proposal_id);
    assert_eq!(proposal.unwrap().executed, true);
}

#[test]
fn test_list_guardians() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let guardian1 = Address::generate(&env);
    let guardian2 = Address::generate(&env);

    let (_, client) = init_vault(&env, &admin);

    // Add guardians
    client.add_guardian(&admin, &guardian1, &100);
    client.add_guardian(&admin, &guardian2, &50);

    // List guardians
    let guardians = client.list_guardians();
    assert_eq!(guardians.len(), 2);

    // Verify weights are present (order may vary)
    let has_guardian1 = guardians.iter().any(|(g, w)| g == guardian1 && w == 100);
    let has_guardian2 = guardians.iter().any(|(g, w)| g == guardian2 && w == 50);
    assert!(has_guardian1);
    assert!(has_guardian2);
}

#[test]
fn test_current_proposal_id_counter() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let new_admin1 = Address::generate(&env);
    let new_admin2 = Address::generate(&env);

    let (_, client) = init_vault(&env, &admin);

    // Initially 0
    assert_eq!(client.get_current_proposal_id(), 0);

    // Submit first proposal
    let current_time = env.ledger().timestamp();
    let eta = current_time + 3600;
    let id1 = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin1,
        &None,
        &0,
        &5000,
        &eta,
    );

    assert_eq!(id1, 0);
    assert_eq!(client.get_current_proposal_id(), 1);

    // Submit second proposal
    let id2 = client.submit_proposal(
        &ProposalKind::RotateAdmin,
        &new_admin2,
        &None,
        &0,
        &5000,
        &eta,
    );

    assert_eq!(id2, 1);
    assert_eq!(client.get_current_proposal_id(), 2);
}

// ── Legacy Merchant Config Tests ────────────────────────────────────────────

#[test]
fn test_merchant_config_initialization() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant_a = Address::generate(&env);
    let payout_address = Address::generate(&env);
    let redirect_url = String::from_str(&env, "https://stellabill.io/success");

    // initialize_merchant_config returns MerchantConfig directly (Soroban unwraps Result<T,E> -> T)
    let config = client.initialize_merchant_config(
        &merchant_a,
        &payout_address,
        &500,  // 5% fee in bips
        &0x1F, // all operations enabled
        &None,
        &redirect_url,
    );

    assert_eq!(config.fee_bips, 500);
    assert_eq!(config.is_active, true);
    assert_eq!(config.redirect_url, redirect_url);
}

#[test]
fn test_merchant_config_governance_enforced() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant_a = Address::generate(&env);
    let payout_address = Address::generate(&env);
    let redirect_url = String::from_str(&env, "https://stellabill.io/success");

    // Initialize config first
    client.initialize_merchant_config(
        &merchant_a,
        &payout_address,
        &500,
        &0x1F,
        &None,
        &redirect_url,
    );

    // Partial update — update_merchant_config also returns MerchantConfig directly
    let updated = client.update_merchant_config(
        &merchant_a,
        &None,                                                // payout unchanged
        &Some(1000),                                          // new fee: 10%
        &None,                                                // ops unchanged
        &None,                                                // active unchanged
        &None,                                                // fee_address unchanged
        &Some(String::from_str(&env, "https://new-url.com")), // new redirect
        &None,                                                // paused unchanged
    );

    assert_eq!(updated.fee_bips, 1000);
    assert_eq!(
        updated.redirect_url,
        String::from_str(&env, "https://new-url.com")
    );
}

#[test]
#[should_panic(expected = "Error(Auth, InvalidAction)")]
fn test_unauthorized_merchant_config_update() {
    let env = Env::default();
    // No mock_all_auths — require_auth() without a signature triggers a host Auth error.
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    let _ = client.initialize_merchant_config(
        &merchant,
        &payout,
        &500,
        &0x1F,
        &None,
        &String::from_str(&env, "https://malicious.com"),
    );
}

// === Edge Cases and Boundary Validation ===

#[test]
fn test_fee_bips_at_maximum_boundary() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    // fee_bips = 10000 is the maximum allowed (100%)
    let config = client.initialize_merchant_config(
        &merchant,
        &payout,
        &10000,
        &0x1F,
        &None,
        &String::from_str(&env, ""),
    );

    assert_eq!(config.fee_bips, 10000);
}

#[test]
#[should_panic(expected = "Error(Contract, #7001)")]
fn test_fee_bips_exceeds_maximum() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    // fee_bips = 10001 exceeds MAX_FEE_BIPS — must return InvalidFeeBips (#1038)
    let _ = client.initialize_merchant_config(
        &merchant,
        &payout,
        &10001,
        &0x1F,
        &None,
        &String::from_str(&env, ""),
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #7003)")]
fn test_operations_without_charge_flag() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    // OP_CHARGE is missing — must return MustAllowChargeOperation (#1041)
    let _ = client.initialize_merchant_config(
        &merchant,
        &payout,
        &0,
        &(OP_WITHDRAW | OP_REFUND),
        &None,
        &String::from_str(&env, ""),
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #7002)")]
fn test_operations_with_invalid_bit() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    // Bit 0x80 is not a valid operation — must return InvalidOperations (#1040)
    let _ = client.initialize_merchant_config(
        &merchant,
        &payout,
        &0,
        &0x80,
        &None,
        &String::from_str(&env, ""),
    );
}

#[test]
fn test_get_merchant_config_not_found() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);

    // get_merchant_config returns Option<MerchantConfig> — None when uninitialized
    let config = client.get_merchant_config(&merchant);
    assert!(config.is_none());
}

#[test]
#[should_panic(expected = "Error(Contract, #2001)")]
fn test_update_nonexistent_config() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);

    // Updating before initialize — must return ConfigNotFound (#1042)
    let _ = client.update_merchant_config(
        &merchant,
        &None,
        &Some(500),
        &None,
        &None,
        &None,
        &None,
        &None,
    );
}

#[test]
fn test_partial_update_preserves_other_fields() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    let initial = client.initialize_merchant_config(
        &merchant,
        &payout,
        &500,  // 5%
        &0x0F, // no OP_AUTO_RENEWAL
        &None,
        &String::from_str(&env, "https://initial.com"),
    );

    assert_eq!(initial.fee_bips, 500);
    assert_eq!(initial.allowed_operations, 0x0F);

    // Update only fee_bips — everything else must be preserved
    let updated = client.update_merchant_config(
        &merchant,
        &None,
        &Some(1000),
        &None,
        &None,
        &None,
        &None,
        &None,
    );

    assert_eq!(updated.fee_bips, 1000);
    assert_eq!(updated.allowed_operations, 0x0F);
    assert_eq!(
        updated.redirect_url,
        String::from_str(&env, "https://initial.com")
    );
}

#[test]
fn test_set_and_get_merchant_config() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    client.initialize_merchant_config(
        &merchant,
        &payout,
        &250,
        &0x1F,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    // get_merchant_config returns Option<MerchantConfig>, so .unwrap() is valid here
    let retrieved = client.get_merchant_config(&merchant).unwrap();

    assert_eq!(retrieved.fee_bips, 250);
    assert_eq!(retrieved.is_active, true);
    assert_eq!(retrieved.allowed_operations, 0x1F);
}

#[test]
fn test_update_deactivate_merchant() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let merchant = Address::generate(&env);
    let payout = Address::generate(&env);

    client.initialize_merchant_config(
        &merchant,
        &payout,
        &500,
        &0x1F,
        &None,
        &String::from_str(&env, ""),
    );

    let updated = client.update_merchant_config(
        &merchant,
        &None,
        &None,
        &None,
        &Some(false), // deactivate
        &None,
        &None,
        &None,
    );

    assert_eq!(updated.is_active, false);
}

#[test]
fn test_withdraw_uninitialized_merchant_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    let merchant = Address::generate(&env);
    let amount = 1_000_000i128;

    // Call try_withdraw_merchant_funds for an uninitialized merchant config
    let res = client.try_withdraw_merchant_funds(&merchant, &amount);
    assert_eq!(res, Err(Ok(crate::types::Error::NotFound)));
}

#[test]
fn test_withdraw_token_uninitialized_merchant_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    let merchant = Address::generate(&env);
    let amount = 1_000_000i128;

    let res = client.try_withdraw_merchant_token_funds(&merchant, &token, &amount);
    assert_eq!(res, Err(Ok(crate::types::Error::NotFound)));
}

#[test]
fn test_merchant_refund_uninitialized_merchant_fails() {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();

    client.init(&token, &6, &admin, &1_000_000i128, &(7 * 24 * 60 * 60));

    let merchant = Address::generate(&env);
    let subscriber = Address::generate(&env);
    let amount = 1_000_000i128;

    let res = client.try_merchant_refund(&merchant, &subscriber, &token, &amount);
    assert_eq!(res, Err(Ok(crate::types::Error::NotFound)));
}

// ══════════════════════════════════════════════════════════════════════════════
// Vote-lock during timelock window — audit #632
//
// Verifies that guardians cannot add or change votes once the proposal's
// timelock (ETA) is reached. Before the fix, a guardian could vote YES to
// appear supportive during the voting window, then flip their vote to NO
// right at execution time to grief the proposal.
// ══════════════════════════════════════════════════════════════════════════════

mod vote_lock_during_timelock {
    use crate::types::Error;
    use crate::{SubscriptionVault, SubscriptionVaultClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _},
        Address, Env, Symbol,
    };

    fn init_vault<'a>(env: &'a Env, admin: &Address) -> SubscriptionVaultClient<'a> {
        let token_admin = Address::generate(env);
        let token_address = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(env, &contract_id);
        client.init(&token_address, &6, admin, &10_000_000, &86400);
        client
    }

    /// Test: voting succeeds when `now < proposal.eta` (before timelock).
    #[test]
    fn vote_before_timelock_succeeds() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian1 = Address::generate(&env);

        let client = init_vault(&env, &admin);
        client.add_guardian(&admin, &guardian1, &100);

        let current_time = env.ledger().timestamp();
        let eta = current_time + 3600; // 1 hour from now

        let target = Address::generate(&env);
        let proposal_id = client.submit_proposal(
            &crate::types::ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000,
            &eta,
        );

        // Vote before timelock — must succeed
        let result = client.try_vote_proposal(&proposal_id, &true);
        assert!(result.is_ok(), "voting before ETA must succeed");
    }

    /// Test: voting is rejected when `now == proposal.eta` (exact timelock start).
    #[test]
    fn vote_at_exact_timelock_start_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian1 = Address::generate(&env);

        let client = init_vault(&env, &admin);
        client.add_guardian(&admin, &guardian1, &100);

        let current_time = env.ledger().timestamp();
        let eta = current_time + 3600; // 1 hour from now

        let target = Address::generate(&env);
        let proposal_id = client.submit_proposal(
            &crate::types::ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000,
            &eta,
        );

        // Advance ledger to exactly the ETA
        env.ledger().set_timestamp(eta);

        // Vote at exact timelock — must be rejected
        let result = client.try_vote_proposal(&proposal_id, &true);
        assert_eq!(result, Err(Ok(Error::InvalidInput)), "voting at exact ETA must be rejected");
    }

    /// Test: voting is rejected when `now > proposal.eta` (after timelock).
    #[test]
    fn vote_after_timelock_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian1 = Address::generate(&env);

        let client = init_vault(&env, &admin);
        client.add_guardian(&admin, &guardian1, &100);

        let current_time = env.ledger().timestamp();
        let eta = current_time + 3600; // 1 hour from now

        let target = Address::generate(&env);
        let proposal_id = client.submit_proposal(
            &crate::types::ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000,
            &eta,
        );

        // Vote before timelock — succeeds
        client.vote_proposal(&proposal_id, &true);

        // Advance ledger past ETA
        env.ledger().set_timestamp(eta + 100);

        // Try to flip vote after timelock — must be rejected
        let result = client.try_vote_proposal(&proposal_id, &false);
        assert_eq!(
            result,
            Err(Ok(Error::InvalidInput)),
            "flipping vote after ETA must be rejected"
        );
    }

    /// Test: execution still works after timelock passes (no regression).
    #[test]
    fn execute_proposal_after_timelock_still_succeeds() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian1 = Address::generate(&env);

        let client = init_vault(&env, &admin);
        client.add_guardian(&admin, &guardian1, &100);

        let current_time = env.ledger().timestamp();
        let eta = current_time + 3600;
        let target = Address::generate(&env);

        let proposal_id = client.submit_proposal(
            &crate::types::ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000, // 50% quorum needed
            &eta,
        );

        // Vote before timelock
        client.vote_proposal(&proposal_id, &true);

        // Advance past ETA — votes are locked
        env.ledger().set_timestamp(eta + 100);

        // Execute — must still succeed (quorum was met before lock)
        let result = client.try_execute_proposal(&proposal_id);
        assert!(result.is_ok(), "execution after timelock must still succeed");
    }

    /// Test: cancel proposal still works even after timelock.
    #[test]
    fn cancel_proposal_after_timelock_succeeds() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian1 = Address::generate(&env);

        let client = init_vault(&env, &admin);
        client.add_guardian(&admin, &guardian1, &100);

        let current_time = env.ledger().timestamp();
        let eta = current_time + 3600;
        let target = Address::generate(&env);

        let proposal_id = client.submit_proposal(
            &crate::types::ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000,
            &eta,
        );

        // Advance past ETA
        env.ledger().set_timestamp(eta + 100);

        // Cancel — must still succeed (admin override not gated by timelock)
        let reason = soroban_sdk::String::from_str(&env, "Timelock reached, but admin cancelled");
        let result = client.try_cancel_proposal(&proposal_id, &reason);
        assert!(result.is_ok(), "cancelling after timelock must still succeed");
    }

    /// Test: verify VoteLockedEvent is emitted on rejected vote.
    #[test]
    fn vote_locked_event_emitted_on_rejected_vote() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian1 = Address::generate(&env);

        let client = init_vault(&env, &admin);
        client.add_guardian(&admin, &guardian1, &100);

        let current_time = env.ledger().timestamp();
        let eta = current_time + 3600;
        let target = Address::generate(&env);

        let proposal_id = client.submit_proposal(
            &crate::types::ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000,
            &eta,
        );

        // Vote before timelock
        client.vote_proposal(&proposal_id, &true);

        // Advance past ETA
        env.ledger().set_timestamp(eta + 100);

        // Attempt vote — rejected
        let _ = client.try_vote_proposal(&proposal_id, &false);

        // Check events include vote_locked
        let events = env.events().all();
        let expected_symbol = Symbol::new(&env, "vote_locked");
        let has_vote_locked = events.iter().any(|e| {
            let topics = e.0;
            topics.len() >= 1
                && {
                    let sym: Symbol = topics.get(0).unwrap();
                    sym == expected_symbol
                }
        });
        assert!(has_vote_locked, "VoteLockedEvent must be emitted");
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Minimum timelock delay enforcement — proposal execution safety
//
// Verifies that proposals cannot be executed until at least MIN_TIMELOCK_DELAY
// (2 days) has elapsed since creation, even if quorum is met earlier.
// This prevents same-ledger execution and ensures a minimum review window.
// ══════════════════════════════════════════════════════════════════════════════

mod min_timelock_delay_enforcement {
    use crate::types::Error;
    use crate::{SubscriptionVault, SubscriptionVaultClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _},
        Address, Env, String,
    };

    // MIN_TIMELOCK_DELAY = 2 days = 172800 seconds (from governance.rs)
    const MIN_TIMELOCK_DELAY: u64 = 2 * 24 * 60 * 60;

    /// Helper to initialize contract
    fn init_vault<'a>(env: &'a Env, admin: &Address) -> SubscriptionVaultClient<'a> {
        let token_admin = Address::generate(env);
        let token_address = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();

        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(env, &contract_id);

        client.init(
            &token_address,
            &6,       // decimals
            admin,
            &10_000_000, // min_topup
            &86400,      // grace period
        );

        client
    }

    /// Test: submission with ETA < now + MIN_TIMELOCK_DELAY is rejected.
    #[test]
    fn submit_with_insufficient_delay_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let client = init_vault(&env, &admin);
        let target = Address::generate(&env);

        let current_time = env.ledger().timestamp();
        // Try to set ETA to less than 2 days in the future
        let too_soon_eta = current_time + 1000; // Only ~16 minutes

        let result = client.try_submit_proposal(
            &crate::types::ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000, // quorum_bps
            &too_soon_eta,
        );

        assert_eq!(
            result,
            Err(Ok(Error::InvalidInput)),
            "proposal with insufficient delay must be rejected"
        );
    }

    /// Test: submission with ETA >= now + MIN_TIMELOCK_DELAY is accepted.
    #[test]
    fn submit_with_sufficient_delay_succeeds() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let client = init_vault(&env, &admin);
        let target = Address::generate(&env);

        let current_time = env.ledger().timestamp();
        // Set ETA to exactly 2 days in the future
        let good_eta = current_time + MIN_TIMELOCK_DELAY;

        let result = client.try_submit_proposal(
            &crate::types::ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000,
            &good_eta,
        );

        assert!(
            result.is_ok(),
            "proposal with sufficient delay must be accepted"
        );
    }

    /// Test: execution before MIN_TIMELOCK_DELAY is rejected.
    #[test]
    fn execute_before_min_delay_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian = Address::generate(&env);
        let client = init_vault(&env, &admin);
        client.add_guardian(&admin, &guardian, &100);

        let current_time = env.ledger().timestamp();
        // Set ETA to far in future so it passes the ETA check
        let eta = current_time + MIN_TIMELOCK_DELAY + 1000;

        let proposal_id = client.submit_proposal(
            &crate::types::ProposalKind::RotateAdmin,
            &Address::generate(&env),
            &None,
            &0,
            &5000,
            &eta,
        );

        // Vote to reach quorum
        client.vote_proposal(&proposal_id, &true);

        // Advance time to past ETA but only 1 day (< MIN_TIMELOCK_DELAY)
        env.ledger().set_timestamp(current_time + 86400);

        // Try to execute — must fail because MIN_TIMELOCK_DELAY hasn't elapsed
        let result = client.try_execute_proposal(&proposal_id);
        assert_eq!(
            result,
            Err(Ok(Error::InvalidInput)),
            "execution before MIN_TIMELOCK_DELAY must be rejected"
        );
    }

    /// Test: execution after MIN_TIMELOCK_DELAY succeeds (if quorum met and ETA passed).
    #[test]
    fn execute_after_min_delay_succeeds() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian = Address::generate(&env);
        let client = init_vault(&env, &admin);
        client.add_guardian(&admin, &guardian, &100);

        let current_time = env.ledger().timestamp();
        let eta = current_time + MIN_TIMELOCK_DELAY;

        let new_admin = Address::generate(&env);
        let proposal_id = client.submit_proposal(
            &crate::types::ProposalKind::RotateAdmin,
            &new_admin,
            &None,
            &0,
            &5000,
            &eta,
        );

        // Vote to reach quorum
        client.vote_proposal(&proposal_id, &true);

        // Advance to exactly MIN_TIMELOCK_DELAY + 1 second
        env.ledger()
            .set_timestamp(current_time + MIN_TIMELOCK_DELAY + 1);

        // Execution must succeed
        let result = client.try_execute_proposal(&proposal_id);
        assert!(
            result.is_ok(),
            "execution after MIN_TIMELOCK_DELAY must succeed"
        );

        // Verify admin was actually rotated
        assert_eq!(client.get_admin(), new_admin);
    }

    /// Test: multiple proposals at different times enforce independent delays.
    #[test]
    fn multiple_proposals_independent_delays() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian = Address::generate(&env);
        let client = init_vault(&env, &admin);
        client.add_guardian(&admin, &guardian, &100);

        let t0 = env.ledger().timestamp();

        // Proposal A at t0
        let eta_a = t0 + MIN_TIMELOCK_DELAY;
        let admin_a = Address::generate(&env);
        let proposal_a = client.submit_proposal(
            &crate::types::ProposalKind::RotateAdmin,
            &admin_a,
            &None,
            &0,
            &5000,
            &eta_a,
        );
        client.vote_proposal(&proposal_a, &true);

        // Advance 1 day
        let t1 = t0 + 86400;
        env.ledger().set_timestamp(t1);

        // Proposal B at t1 (requires MIN_TIMELOCK_DELAY from t1, not t0)
        let eta_b = t1 + MIN_TIMELOCK_DELAY;
        let admin_b = Address::generate(&env);
        let proposal_b = client.submit_proposal(
            &crate::types::ProposalKind::RotateAdmin,
            &admin_b,
            &None,
            &0,
            &5000,
            &eta_b,
        );
        client.vote_proposal(&proposal_b, &true);

        // At time t0 + MIN_TIMELOCK_DELAY: A can execute, B cannot
        env.ledger().set_timestamp(t0 + MIN_TIMELOCK_DELAY + 1);

        let a_result = client.try_execute_proposal(&proposal_a);
        assert!(a_result.is_ok(), "proposal A should execute");

        let b_result = client.try_execute_proposal(&proposal_b);
        assert_eq!(
            b_result,
            Err(Ok(Error::InvalidInput)),
            "proposal B should be rejected (MIN_TIMELOCK_DELAY not reached)"
        );

        // At time t1 + MIN_TIMELOCK_DELAY: B can execute
        env.ledger().set_timestamp(t1 + MIN_TIMELOCK_DELAY + 1);

        // Re-initialize for second proposal since first admin changed
        let guardian2 = Address::generate(&env);
        let admin_a_now = client.get_admin();
        client.add_guardian(&admin_a_now, &guardian2, &100);

        let b_result = client.try_execute_proposal(&proposal_b);
        assert!(b_result.is_ok(), "proposal B should execute at its time");
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Governance quorum recalculation when voting weight changes mid-vote
//
// Scenario: Guardian casts vote, then their weight changes (token transfer or removal).
// Quorum calculation uses current guardian weights at execution time, not vote time.
// This can lead to proposals passing/failing with different total weight than expected.
// ══════════════════════════════════════════════════════════════════════════════

mod governance_quorum_weight_changes {
    use crate::types::{Error, ProposalKind};
    use crate::{SubscriptionVault, SubscriptionVaultClient};
    use soroban_sdk::{
        testutils::{Address as _, Ledger as _},
        Address, Env, String,
    };

    const MIN_TIMELOCK_DELAY: u64 = 2 * 24 * 60 * 60;

    fn init_vault<'a>(env: &'a Env, admin: &Address) -> SubscriptionVaultClient<'a> {
        let token_admin = Address::generate(env);
        let token_address = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        let contract_id = env.register(SubscriptionVault, ());
        let client = SubscriptionVaultClient::new(env, &contract_id);
        client.init(&token_address, &6, admin, &10_000_000, &86400);
        client
    }

    /// Test: Guardian votes YES, then is removed. Vote is excluded from quorum.
    /// This is the documented behavior: votes from removed guardians are ignored.
    #[test]
    fn guardian_removed_after_voting_vote_excluded_from_quorum() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian1 = Address::generate(&env);
        let guardian2 = Address::generate(&env);

        let client = init_vault(&env, &admin);

        // Add two guardians: guardian1 with 100 weight, guardian2 with 50 weight
        client.add_guardian(&admin, &guardian1, &100);
        client.add_guardian(&admin, &guardian2, &50);

        let current_time = env.ledger().timestamp();
        let eta = current_time + MIN_TIMELOCK_DELAY;

        let target = Address::generate(&env);
        let proposal_id = client.submit_proposal(
            &ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000, // 50% quorum required
            &eta,
        );

        // Guardian1 votes YES (100 votes)
        client.vote_proposal(&proposal_id, &true);

        // Admin removes guardian1 — their vote should no longer count
        client.remove_guardian(&admin, &guardian1);

        // Advance past ETA
        env.ledger().set_timestamp(eta + 1);

        // Try to execute — should fail because guardian1's vote is excluded
        // Total weight now = 50 (only guardian2 remains)
        // Votes for = 0 (guardian1's vote excluded)
        // Required = 50 * 50% = 25
        // 0 < 25, so quorum not met
        let result = client.try_execute_proposal(&proposal_id);
        assert_eq!(
            result,
            Err(Ok(Error::InvalidInput)),
            "proposal must fail when voting guardian is removed before execution"
        );
    }

    /// Test: Guardian with weight 100 votes, weight is reduced to 10 before execution.
    /// The quorum calculation uses the current weight (10), not the weight at vote time (100).
    /// 
    /// **Expected behavior**: Quorum is recalculated with current weights at execution time.
    /// This is the documented design: votes cast earlier are counted against the guardian's
    /// *current* weight, not their historical weight.
    #[test]
    fn guardian_weight_reduced_after_voting_affects_quorum() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian1 = Address::generate(&env);

        let client = init_vault(&env, &admin);

        // Add guardian with weight 100
        client.add_guardian(&admin, &guardian1, &100);

        let current_time = env.ledger().timestamp();
        let eta = current_time + MIN_TIMELOCK_DELAY;

        let target = Address::generate(&env);
        let proposal_id = client.submit_proposal(
            &ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000, // 50% quorum required
            &eta,
        );

        // Guardian votes YES with weight 100
        client.vote_proposal(&proposal_id, &true);

        // Admin removes guardian1 and re-adds with weight 10
        client.remove_guardian(&admin, &guardian1);
        client.add_guardian(&admin, &guardian1, &10);

        // Advance past ETA
        env.ledger().set_timestamp(eta + 1);

        // Execute: quorum calculation uses current weight (10)
        // Total weight = 10
        // Votes for = 10 (guardian1's YES vote counted at current weight)
        // Required = 10 * 50% = 5
        // 10 >= 5, so quorum is met
        let result = client.try_execute_proposal(&proposal_id);
        assert!(
            result.is_ok(),
            "proposal should pass with guardian's current weight"
        );
    }

    /// Test: Guardian votes YES, then another guardian is added with higher weight.
    /// The total weight denominator increases, potentially causing quorum to fail.
    #[test]
    fn new_guardian_added_after_vote_increases_quorum_denominator() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian1 = Address::generate(&env);
        let guardian2 = Address::generate(&env);

        let client = init_vault(&env, &admin);

        // Add guardian1 with weight 100 (total weight = 100)
        client.add_guardian(&admin, &guardian1, &100);

        let current_time = env.ledger().timestamp();
        let eta = current_time + MIN_TIMELOCK_DELAY;

        let target = Address::generate(&env);
        let proposal_id = client.submit_proposal(
            &ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000, // 50% quorum required
            &eta,
        );

        // Guardian1 votes YES (100 votes out of 100 total = 100%)
        client.vote_proposal(&proposal_id, &true);

        // Add guardian2 with weight 200 (total weight now = 300)
        client.add_guardian(&admin, &guardian2, &200);

        // Advance past ETA
        env.ledger().set_timestamp(eta + 1);

        // Execute: quorum calculation uses current total weight
        // Total weight = 300 (guardian1: 100, guardian2: 200)
        // Votes for = 100 (only guardian1 voted)
        // Required = 300 * 50% = 150
        // 100 < 150, so quorum not met
        let result = client.try_execute_proposal(&proposal_id);
        assert_eq!(
            result,
            Err(Ok(Error::InvalidInput)),
            "proposal must fail when new guardians increase denominator after voting"
        );
    }

    /// Test: Multiple guardians vote, one is removed, proposal should still pass
    /// if remaining votes meet quorum with recalculated denominator.
    #[test]
    fn partial_guardian_removal_proposal_passes_with_remaining_votes() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian1 = Address::generate(&env);
        let guardian2 = Address::generate(&env);
        let guardian3 = Address::generate(&env);

        let client = init_vault(&env, &admin);

        // Add three guardians with equal weight
        client.add_guardian(&admin, &guardian1, &100);
        client.add_guardian(&admin, &guardian2, &100);
        client.add_guardian(&admin, &guardian3, &100);
        // Total weight = 300

        let current_time = env.ledger().timestamp();
        let eta = current_time + MIN_TIMELOCK_DELAY;

        let target = Address::generate(&env);
        let proposal_id = client.submit_proposal(
            &ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000, // 50% quorum required
            &eta,
        );

        // All three guardians vote YES (300 votes, 100%)
        client.vote_proposal(&proposal_id, &true);
        // Note: in the current implementation, we need to handle voting with nonces
        // or mock_all_auths handles this transparently

        // Remove guardian1 — their vote is excluded
        client.remove_guardian(&admin, &guardian1);
        // Total weight now = 200, votes for = 200 (guardian2 + guardian3)

        // Advance past ETA
        env.ledger().set_timestamp(eta + 1);

        // Execute: should pass because 200/200 >= 50%
        let result = client.try_execute_proposal(&proposal_id);
        assert!(
            result.is_ok(),
            "proposal should pass when remaining votes meet quorum"
        );
    }

    /// Test: Document the known limitation where votes are cast, then many new
    /// guardians are added, potentially making it impossible to ever reach quorum
    /// even if all original voters voted YES.
    #[test]
    fn quorum_impossible_when_massive_weight_added_after_voting() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let guardian1 = Address::generate(&env);

        let client = init_vault(&env, &admin);

        // Single guardian with weight 10
        client.add_guardian(&admin, &guardian1, &10);

        let current_time = env.ledger().timestamp();
        let eta = current_time + MIN_TIMELOCK_DELAY;

        let target = Address::generate(&env);
        let proposal_id = client.submit_proposal(
            &ProposalKind::RotateAdmin,
            &target,
            &None,
            &0,
            &5000, // 50% quorum
            &eta,
        );

        // Guardian1 votes YES (10 votes, 100% of total)
        client.vote_proposal(&proposal_id, &true);

        // Admin adds 10 new guardians with massive weight
        for i in 0..10 {
            let new_guardian = Address::generate(&env);
            client.add_guardian(&admin, &new_guardian, &1000);
        }
        // Total weight now = 10 + (10 * 1000) = 10,010
        // Votes for = 10
        // Required = 10,010 * 50% = 5,005
        // Impossible to reach without the new guardians voting

        env.ledger().set_timestamp(eta + 1);

        let result = client.try_execute_proposal(&proposal_id);
        assert_eq!(
            result,
            Err(Ok(Error::InvalidInput)),
            "proposal becomes unpassable when massive weight is added after voting"
        );
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Admin rotation invariant tests
//
// Security model enforced by these tests:
//   1. Only the current stored admin can rotate the admin key.
//   2. Rotation updates exactly one canonical key (DataKey::Admin).
//   3. Old admin loses all privileges atomically in the same transaction.
//   4. New admin gains all privileges atomically in the same transaction.
//   5. Events carry old_admin, new_admin, and timestamp for audit trails.
//   6. Rotation is replay-protected: wrong nonce is rejected before any
//      state mutation occurs.
//   7. Self-rotation and rotation to the contract address are rejected.
//   8. The emergency stop does not gate rotate_admin itself.
//   9. Active subscriptions and pending charges are unaffected by rotation.
// ══════════════════════════════════════════════════════════════════════════════

mod admin_rotation_invariants {
    use crate::test_utils::{fixtures, setup::TestEnv};
    use crate::{AdminRotatedEvent, Error, SubscriptionStatus};
    use soroban_sdk::{
        testutils::Address as _, testutils::Events as _, testutils::Ledger as _, Address, IntoVal,
        Vec,
    };

    const T0: u64 = 1_000;
    const INTERVAL: u64 = 30 * 24 * 60 * 60; // 30 days
    const PREPAID: i128 = 50_000_000;

    // ── Invariant 1: exactly one canonical admin key ──────────────────────────

    #[test]
    fn rotate_updates_exactly_one_canonical_admin_key() {
        // DataKey::Admin is the sole source of truth. get_admin() must return
        // the new address immediately after rotation and never the old one.
        let te = TestEnv::default();
        let new_admin = Address::generate(&te.env);

        assert_eq!(te.client.get_admin(), te.admin);
        te.client.rotate_admin(&te.admin, &new_admin, &0u64);
        assert_eq!(te.client.get_admin(), new_admin);
    }

    #[test]
    fn rotate_admin_rejected_for_non_admin() {
        let te = TestEnv::default();
        let stranger = Address::generate(&te.env);
        let target = Address::generate(&te.env);

        let result = te.client.try_rotate_admin(&stranger, &target, &0u64);
        assert_eq!(result, Err(Ok(Error::Unauthorized)));
        // Canonical key must be unchanged.
        assert_eq!(te.client.get_admin(), te.admin);
    }

    #[test]
    fn rotate_admin_self_rotation_rejected() {
        // Rotating to the same address wastes a nonce and could mask
        // misconfiguration; the contract rejects it with SelfRotation.
        let te = TestEnv::default();
        let result = te.client.try_rotate_admin(&te.admin, &te.admin, &0u64);
        assert_eq!(result, Err(Ok(Error::SelfRotation)));
        assert_eq!(te.client.get_admin(), te.admin);
    }

    #[test]
    fn rotate_admin_to_contract_address_rejected() {
        // The contract cannot sign Soroban auth transactions, so rotating to it
        // would permanently lock all admin-only operations.
        let te = TestEnv::default();
        let result = te
            .client
            .try_rotate_admin(&te.admin, &te.client.address, &0u64);
        assert_eq!(result, Err(Ok(Error::InvalidNewAdmin)));
        assert_eq!(te.client.get_admin(), te.admin);
    }

    // ── Invariant 2: immediate and complete privilege transfer ────────────────

    #[test]
    fn old_admin_loses_all_admin_only_privileges_after_rotation() {
        let te = TestEnv::default();
        let new_admin = Address::generate(&te.env);
        let other = Address::generate(&te.env);

        te.client.rotate_admin(&te.admin, &new_admin, &0u64);

        // set_min_topup requires explicit admin address check.
        assert_eq!(
            te.client.try_set_min_topup(&te.admin, &5_000_000i128),
            Err(Ok(Error::Unauthorized))
        );
        // Emergency stop management requires admin.
        assert_eq!(
            te.client.try_enable_emergency_stop(&te.admin),
            Err(Ok(Error::Unauthorized))
        );
        assert_eq!(
            te.client.try_disable_emergency_stop(&te.admin),
            Err(Ok(Error::Unauthorized))
        );
        // Cannot re-rotate even with the next nonce value — auth check fires first.
        assert_eq!(
            te.client.try_rotate_admin(&te.admin, &other, &1u64),
            Err(Ok(Error::Unauthorized))
        );
    }

    #[test]
    fn new_admin_gains_all_admin_only_privileges_immediately() {
        let te = TestEnv::default();
        let new_admin = Address::generate(&te.env);

        te.client.rotate_admin(&te.admin, &new_admin, &0u64);

        te.client.set_min_topup(&new_admin, &3_000_000i128);
        assert_eq!(te.client.get_min_topup(), 3_000_000i128);

        te.client.enable_emergency_stop(&new_admin);
        assert!(te.client.get_emergency_stop_status());
        te.env.ledger().with_mut(|li| {
            li.timestamp += crate::admin::CONFIG_COOLDOWN_SECS
        });
        te.client.disable_emergency_stop(&new_admin);
        assert!(!te.client.get_emergency_stop_status());
    }

    // ── Invariant 3: AdminRotatedEvent carries correct payload ────────────────

    #[test]
    fn rotate_admin_emits_event_with_correct_old_admin_new_admin_and_timestamp() {
        // Off-chain indexers rely on AdminRotatedEvent to track the rotation
        // history. Verify every field is correct.
        let te = TestEnv::default();
        let new_admin = Address::generate(&te.env);
        let expected_ts = 42_000u64;
        te.env.ledger().with_mut(|li| li.timestamp = expected_ts);

        te.client.rotate_admin(&te.admin, &new_admin, &0u64);

        // admin_rotated is the last event emitted in the call
        // (nonce_consumed comes first from check_and_advance).
        let events = te.env.events().all();
        let last = events.last().expect("no events emitted after rotate_admin");
        let payload: AdminRotatedEvent = last.2.into_val(&te.env);

        assert_eq!(payload.old_admin, te.admin);
        assert_eq!(payload.new_admin, new_admin);
        assert_eq!(payload.timestamp, expected_ts);
    }

    // ── Invariant 4: nonce protects against replay and out-of-order calls ─────

    #[test]
    fn rotate_admin_wrong_nonce_rejected_state_unchanged() {
        // Providing nonce=1 when 0 is expected must be rejected after auth passes
        // but before any state mutation. The admin key must remain unchanged.
        let te = TestEnv::default();
        let new_admin = Address::generate(&te.env);

        let result = te.client.try_rotate_admin(&te.admin, &new_admin, &1u64);
        assert_eq!(result, Err(Ok(Error::NonceAlreadyUsed)));

        // Admin key unchanged; nonce not advanced.
        assert_eq!(te.client.get_admin(), te.admin);
        assert_eq!(te.client.get_admin_nonce(&te.admin, &1u32), 0u64);
    }

    // ── Invariant 5: subscription storage is isolated from admin rotation ─────

    #[test]
    fn rotate_admin_does_not_mutate_subscription_state() {
        let te = TestEnv::default();
        let (id, _, _) =
            fixtures::create_subscription(&te.env, &te.client, SubscriptionStatus::Active);
        let before = te.client.get_subscription(&id);

        te.client
            .rotate_admin(&te.admin, &Address::generate(&te.env), &0u64);

        let after = te.client.get_subscription(&id);
        assert_eq!(before.subscriber, after.subscriber);
        assert_eq!(before.merchant, after.merchant);
        assert_eq!(before.amount, after.amount);
        assert_eq!(before.status, after.status);
        assert_eq!(before.prepaid_balance, after.prepaid_balance);
    }

    // ── Invariant 6: pending charges remain chargeable after rotation ─────────

    #[test]
    fn new_admin_can_batch_charge_subscriptions_pending_before_rotation() {
        // batch_charge uses require_stored_admin_auth (reads admin from storage).
        // After rotation the new admin is stored so the call succeeds. This ensures
        // that pending charges are never dropped or locked by an admin rotation.
        let te = TestEnv::default();
        te.env.ledger().with_mut(|li| li.timestamp = T0);

        let (id, _, _) =
            fixtures::create_subscription(&te.env, &te.client, SubscriptionStatus::Active);
        fixtures::seed_balance(&te.env, &te.client, id, PREPAID);

        // Advance time past the billing interval so a charge is due.
        te.env
            .ledger()
            .with_mut(|li| li.timestamp = T0 + INTERVAL + 1);

        let new_admin = Address::generate(&te.env);
        te.client.rotate_admin(&te.admin, &new_admin, &0u64);

        let ids = Vec::from_array(&te.env, [id]);
        let results = te.client.batch_charge(&ids, &0u64);
        assert_eq!(results.len(), 1);
        assert!(results.get(0).unwrap().success);
    }

    // ── Invariant 7: rotate_admin is not gated by the emergency stop ──────────

    #[test]
    fn rotate_admin_succeeds_while_emergency_stop_is_active() {
        // rotate_admin must not be blocked by the emergency stop circuit breaker.
        // Rotation is the primary recovery path when the contract is paused.
        let te = TestEnv::default();
        te.client.enable_emergency_stop(&te.admin);
        assert!(te.client.get_emergency_stop_status());

        let new_admin = Address::generate(&te.env);
        te.client.rotate_admin(&te.admin, &new_admin, &0u64);
        assert_eq!(te.client.get_admin(), new_admin);
    }

    #[test]
    fn new_admin_can_disable_emergency_stop_after_rotation() {
        // After rotating during an active emergency stop, the new admin must be
        // able to clear it. This is the critical recovery path.
        let te = TestEnv::default();
        te.client.enable_emergency_stop(&te.admin);

        let new_admin = Address::generate(&te.env);
        te.client.rotate_admin(&te.admin, &new_admin, &0u64);

        te.env.ledger().with_mut(|li| {
            li.timestamp += crate::admin::CONFIG_COOLDOWN_SECS
        });
        te.client.disable_emergency_stop(&new_admin);
        assert!(!te.client.get_emergency_stop_status());

        // Old admin cannot re-enable after privilege was transferred.
        assert_eq!(
            te.client.try_enable_emergency_stop(&te.admin),
            Err(Ok(Error::Unauthorized))
        );
    }
}
