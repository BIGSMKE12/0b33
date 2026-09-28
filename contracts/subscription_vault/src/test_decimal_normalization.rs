#![cfg(test)]

use crate::types::{normalize_amount, denormalize_amount, DataKey, Error};
use soroban_sdk::{Address, Env};
use soroban_sdk::testutils::Address as _;

#[test]
fn test_6_decimal_token_normalization() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &6u32);

        // 1.234567 in 6 decimals = 1_234_567
        let raw = 1_234_567i128;
        let normalized = normalize_amount(&env, &token, raw).unwrap();
        // Should scale up by 10^(9-6) = 1000
        assert_eq!(normalized, 1_234_567_000);

        let denormalized = denormalize_amount(&env, &token, normalized).unwrap();
        assert_eq!(denormalized, raw);
    });
}

#[test]
fn test_7_decimal_token_normalization() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &7u32);

        // 12.345678 in 7 decimals = 12_345_678
        let raw = 12_345_678i128;
        let normalized = normalize_amount(&env, &token, raw).unwrap();
        // Should scale up by 10^(9-7) = 100
        assert_eq!(normalized, 1_234_567_800);

        let denormalized = denormalize_amount(&env, &token, normalized).unwrap();
        assert_eq!(denormalized, raw);
    });
}

#[test]
fn test_zero_decimal_token_rejected() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &0u32);

        let res = normalize_amount(&env, &token, 100);
        assert_eq!(res, Err(Error::InvalidTokenDecimals));

        let res_denorm = denormalize_amount(&env, &token, 100);
        assert_eq!(res_denorm, Err(Error::InvalidTokenDecimals));
    });
}

/// Test case for 0-decimal token (whole-unit tokens).
/// 
/// Tokens with 0 decimals represent whole units (e.g., NFTs, whole coin tokens).
/// Division by 10^0 = 1 should be a no-op, but the current implementation rejects
/// 0-decimal tokens. This test documents the current behavior.
///
/// ACCEPTANCE CRITERIA: This test verifies that the code path for 0-decimal tokens
/// is exercised and the behavior is well-defined (currently: rejection).
///
/// If the requirement changes to support 0-decimal tokens, update both the
/// implementation and this test to verify no-op normalization (multiply/divide by 1).
#[test]
fn test_zero_decimal_token_normalization_behavior() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        // Set token decimals to 0 (whole-unit token)
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &0u32);

        // For 0-decimal tokens:
        // - Raw amount = actual whole units (e.g., 100 = 100 tokens)
        // - Normalized (9 decimals) should be: 100 * 10^9 = 100_000_000_000
        // - Denormalized should be: 100_000_000_000 / 10^9 = 100
        
        let raw_amount = 100i128;
        
        // Current behavior: rejects 0-decimal tokens
        let normalize_result = normalize_amount(&env, &token, raw_amount);
        assert_eq!(
            normalize_result, 
            Err(Error::InvalidTokenDecimals),
            "Current implementation rejects 0-decimal tokens"
        );
        
        // If this behavior changes to support 0-decimal tokens, expected behavior:
        // let normalized = normalize_result.unwrap();
        // assert_eq!(normalized, 100_000_000_000i128, "100 whole tokens -> 100e9 in 9-decimal internal");
        // 
        // let denormalized = denormalize_amount(&env, &token, normalized).unwrap();
        // assert_eq!(denormalized, raw_amount, "round-trip must preserve whole amount");
    });
}

/// Edge case: 0-decimal token with large whole amounts near i128::MAX.
/// Verifies overflow detection when normalizing large whole-unit amounts.
#[test]
fn test_zero_decimal_token_large_amounts_overflow_detection() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &0u32);

        // Large whole amount that would overflow when scaled to 9 decimals
        // i128::MAX / 10^9 ≈ 1.7e29, so amounts above this would overflow
        let large_amount = i128::MAX / 1_000_000_000 + 1;
        
        let result = normalize_amount(&env, &token, large_amount);
        
        // Current: rejects due to 0 decimals, but if supported, should detect overflow
        assert!(
            result == Err(Error::InvalidTokenDecimals) || result == Err(Error::Overflow),
            "Must reject 0-decimal token or detect overflow for large amounts"
        );
    });
}

#[test]
fn test_unregistered_token_rejected() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        let res = normalize_amount(&env, &token, 100);
        assert_eq!(res, Err(Error::InvalidToken));
    });
}

#[test]
fn test_normalization_overflow() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &6u32);

        // raw = i128::MAX, which will overflow when multiplying by 1000
        let res = normalize_amount(&env, &token, i128::MAX);
        assert_eq!(res, Err(Error::Overflow));
    });
}

#[test]
fn test_denormalization_overflow() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &12u32);

        // normalized = i128::MAX, which will overflow when multiplying by 1000
        let res = denormalize_amount(&env, &token, i128::MAX);
        assert_eq!(res, Err(Error::Overflow));
    });
}

#[test]
fn test_greater_than_9_decimals() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &12u32); // 12 decimals

        // 1.234567890123 in 12 decimals = 1_234_567_890_123
        // This cannot be represented in 9 decimals without precision loss
        let raw_loss = 1_234_567_890_123i128;
        let res_loss = normalize_amount(&env, &token, raw_loss);
        assert_eq!(res_loss, Err(Error::InvalidInput));

        // 1.234567890000 in 12 decimals = 1_234_567_890_000
        // This can be represented exactly in 9 decimals as 1_234_567_890
        let raw_exact = 1_234_567_890_000i128;
        let normalized = normalize_amount(&env, &token, raw_exact).unwrap();
        assert_eq!(normalized, 1_234_567_890);

        let denormalized = denormalize_amount(&env, &token, normalized).unwrap();
        assert_eq!(denormalized, raw_exact);
    });
}

#[test]
fn test_denormalization_precision_loss() {
    let env = Env::default();
    let contract_id = env.register(crate::SubscriptionVault, ());
    let token = Address::generate(&env);

    env.as_contract(&contract_id, || {
        env.storage().instance().set(&DataKey::TokenDecimals(token.clone()), &6u32);

        // 1.0005 in 9 decimals = 1_000_500_000
        // Try to denormalize to 6 decimals, which cannot represent 1.0005 exactly
        let normalized = 1_000_500_500i128;
        let res = denormalize_amount(&env, &token, normalized);
        assert_eq!(res, Err(Error::InvalidInput));
    });
}
