use crate::{Error, SubscriptionVault, SubscriptionVaultClient, SubscriptionStatus, SplitPayees};
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{token::StellarAssetClient as TokenAdminClient, Address, Env, Vec};

fn setup() -> (
    Env,
    Address,
    SubscriptionVaultClient<'static>,
    TokenAdminClient<'static>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = TokenAdminClient::new(&env, &token);

    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);

    client.init(
        &token,
        &7u32,
        &admin,
        &10i128, // min topup: 10 base units
        &(7 * 24 * 60 * 60u64), // grace period: 7 days
    );

    (env, contract_id, client, token_admin, token)
}

#[test]
fn test_create_subscription_with_split_validation() {
    let (env, _, client, _, _) = setup();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    use soroban_sdk::String;
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &0x1Fi32,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    let payee1 = Address::generate(&env);
    let payee2 = Address::generate(&env);

    // Weights sum != 10_000 (9_999)
    let mut bad_entries1 = Vec::new(&env);
    bad_entries1.push_back((payee1.clone(), 5_000u32));
    bad_entries1.push_back((payee2.clone(), 4_999u32));
    
    let res = client.try_create_subscription_with_split(
        &subscriber,
        &merchant,
        &100i128,
        &3600u64,
        &false,
        &None::<i128>,
        &None::<u64>,
        &bad_entries1,
    );
    assert_eq!(res, Err(Ok(Error::InvalidInput)));

    // Weight == 0
    let mut bad_entries2 = Vec::new(&env);
    bad_entries2.push_back((payee1.clone(), 10_000u32));
    bad_entries2.push_back((payee2.clone(), 0u32));
    
    let res2 = client.try_create_subscription_with_split(
        &subscriber,
        &merchant,
        &100i128,
        &3600u64,
        &false,
        &None::<i128>,
        &None::<u64>,
        &bad_entries2,
    );
    assert_eq!(res2, Err(Ok(Error::InvalidInput)));

    // Empty list
    let bad_entries3 = Vec::new(&env);
    let res3 = client.try_create_subscription_with_split(
        &subscriber,
        &merchant,
        &100i128,
        &3600u64,
        &false,
        &None::<i128>,
        &None::<u64>,
        &bad_entries3,
    );
    assert_eq!(res3, Err(Ok(Error::InvalidInput)));
}

#[test]
fn test_split_charge_distribution_and_dust() {
    let (env, _, client, token_admin, _) = setup();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    use soroban_sdk::String;
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &0x1Fi32,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    let payee0 = Address::generate(&env);
    let payee1 = Address::generate(&env);
    let payee2 = Address::generate(&env);

    // Initialize configs for split payees too so they can earn
    for payee in [&payee0, &payee1, &payee2].iter() {
        client.initialize_merchant_config(
            payee,
            payee,
            &0i32,
            &0x1Fi32,
            &None,
            &String::from_str(&env, "https://example.com"),
        );
    }

    let mut entries = Vec::new(&env);
    entries.push_back((payee0.clone(), 3333u32));
    entries.push_back((payee1.clone(), 3333u32));
    entries.push_back((payee2.clone(), 3334u32));

    let sub_id = client.create_subscription_with_split(
        &subscriber,
        &merchant,
        &100i128, // amount: 100 base units
        &3600u64, // interval: 1 hour
        &false,
        &None::<i128>,
        &None::<u64>,
        &entries,
    );

    // Deposit funds
    token_admin.mint(&subscriber, &10_000i128);
    client.deposit_funds(&sub_id, &1_000i128, &None);

    // Advance time and charge
    env.ledger().set_timestamp(env.ledger().timestamp() + 3601);
    client.charge_subscription(&sub_id, &None);

    // Verify balances
    // Payee 1: 100 * 3333 / 10000 = 33
    // Payee 2: 100 * 3334 / 10000 = 33
    // Payee 0 (first payee): gets remainder = 100 - (33 + 33) = 34.
    let earnings0 = client.get_merchant_token_earnings(&payee0, &client.get_subscription(&sub_id).token);
    let earnings1 = client.get_merchant_token_earnings(&payee1, &client.get_subscription(&sub_id).token);
    let earnings2 = client.get_merchant_token_earnings(&payee2, &client.get_subscription(&sub_id).token);

    assert_eq!(earnings0.accruals.interval, 34);
    assert_eq!(earnings1.accruals.interval, 33);
    assert_eq!(earnings2.accruals.interval, 33);

    // Verify that get_split_payees returns the config correctly
    let saved_split = client.get_split_payees(&sub_id).unwrap();
    assert_eq!(saved_split.entries.len(), 3);
    assert_eq!(saved_split.entries.get(0).unwrap().0, payee0);
}

#[test]
fn test_split_payees_paused_or_blocklisted() {
    let (env, _, client, token_admin, _) = setup();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    use soroban_sdk::String;
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &0x1Fi32,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    let payee0 = Address::generate(&env);
    let payee1 = Address::generate(&env);

    for payee in [&payee0, &payee1].iter() {
        client.initialize_merchant_config(
            payee,
            payee,
            &0i32,
            &0x1Fi32,
            &None,
            &String::from_str(&env, "https://example.com"),
        );
    }

    let mut entries = Vec::new(&env);
    entries.push_back((payee0.clone(), 5000u32));
    entries.push_back((payee1.clone(), 5000u32));

    let sub_id = client.create_subscription_with_split(
        &subscriber,
        &merchant,
        &100i128,
        &3600u64,
        &false,
        &None::<i128>,
        &None::<u64>,
        &entries,
    );

    token_admin.mint(&subscriber, &10_000i128);
    client.deposit_funds(&sub_id, &1_000i128, &None);

    // 1. Blocklist one payee
    let admin = client.get_admin();
    client.add_to_blocklist(&admin, &payee1, &Some(String::from_str(&env, "Test blocklist")));

    env.ledger().set_timestamp(env.ledger().timestamp() + 3601);
    let res = client.try_charge_subscription(&sub_id, &None);
    assert_eq!(res, Err(Ok(Error::SubscriberBlocklisted)));

    // Unblocklist payee1
    client.remove_from_blocklist(&admin, &payee1);

    // 2. Pause one payee
    client.update_merchant_config(
        &payee1,
        &Some(payee1.clone()),
        &None,
        &None,
        &None,
        &None,
        &None,
        &Some(true), // Pause
    );

    let res_paused = client.try_charge_subscription(&sub_id, &None);
    assert_eq!(res_paused, Err(Ok(Error::MerchantPaused)));
}

#[test]
fn test_split_payees_removal_mid_billing() {
    let (env, _, client, token_admin, _) = setup();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    use soroban_sdk::String;
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &0x1Fi32,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    let payee0 = Address::generate(&env);
    let payee1 = Address::generate(&env);

    for payee in [&payee0, &payee1].iter() {
        client.initialize_merchant_config(
            payee,
            payee,
            &0i32,
            &0x1Fi32,
            &None,
            &String::from_str(&env, "https://example.com"),
        );
    }

    let mut entries = Vec::new(&env);
    entries.push_back((payee0.clone(), 5000u32));
    entries.push_back((payee1.clone(), 5000u32));

    let sub_id = client.create_subscription_with_split(
        &subscriber,
        &merchant,
        &100i128,
        &3600u64,
        &false,
        &None::<i128>,
        &None::<u64>,
        &entries,
    );

    token_admin.mint(&subscriber, &10_000i128);
    client.deposit_funds(&sub_id, &1_000i128, &None);

    // Update/remove split payees (set to None)
    client.update_split_payees(&subscriber, &sub_id, &None);

    // Verify it is removed
    assert!(client.get_split_payees(&sub_id).is_none());

    // Charge
    env.ledger().set_timestamp(env.ledger().timestamp() + 3601);
    client.charge_subscription(&sub_id, &None);

    // Primary merchant should get 100% of the funds
    let merchant_earnings = client.get_merchant_token_earnings(&merchant, &client.get_subscription(&sub_id).token);
    assert_eq!(merchant_earnings.accruals.interval, 100);

    // Split payees should get 0
    let payee0_earnings = client.get_merchant_token_earnings(&payee0, &client.get_subscription(&sub_id).token);
    let payee1_earnings = client.get_merchant_token_earnings(&payee1, &client.get_subscription(&sub_id).token);
    assert_eq!(payee0_earnings.accruals.interval, 0);
    assert_eq!(payee1_earnings.accruals.interval, 0);
}

// ══════════════════════════════════════════════════════════════════════════════
// Split Billing with Zero-Share Recipients
//
// When an amount is split among many recipients, integer division can produce
// a zero share for a recipient with a very small percentage. This test verifies
// that:
// - Zero-share recipients receive nothing (their earnings are not incremented)
// - No transfer is attempted for zero amounts
// - Dust/remainder is still allocated to the first payee as documented
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_split_billing_with_zero_share_recipient() {
    let (env, _, client, token_admin, _) = setup();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    use soroban_sdk::String;
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &0x1Fi32,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    let payee0 = Address::generate(&env); // Will get majority share
    let payee1 = Address::generate(&env); // Will get very small share → rounds to 0

    // Initialize configs for both payees
    for payee in [&payee0, &payee1].iter() {
        client.initialize_merchant_config(
            payee,
            payee,
            &0i32,
            &0x1Fi32,
            &None,
            &String::from_str(&env, "https://example.com"),
        );
    }

    // Split: payee0 gets 9999 bps (99.99%), payee1 gets 1 bp (0.01%)
    // With a charge amount of 100, payee1's share = 100 * 1 / 10000 = 0 (integer division)
    let mut entries = Vec::new(&env);
    entries.push_back((payee0.clone(), 9999u32));
    entries.push_back((payee1.clone(), 1u32));

    let sub_id = client.create_subscription_with_split(
        &subscriber,
        &merchant,
        &100i128, // amount: 100 base units
        &3600u64, // interval: 1 hour
        &false,
        &None::<i128>,
        &None::<u64>,
        &entries,
    );

    // Deposit funds
    token_admin.mint(&subscriber, &10_000i128);
    client.deposit_funds(&sub_id, &1_000i128, &None);

    // Advance time and charge
    env.ledger().set_timestamp(env.ledger().timestamp() + 3601);
    client.charge_subscription(&sub_id, &None);

    // Verify balances
    // Payee1 share: 100 * 1 / 10000 = 0 (rounds down)
    // Payee0 share: 100 * 9999 / 10000 = 99
    // Remainder: 100 - 99 = 1 → goes to payee0 (first payee gets dust)
    // So payee0 should get 99 + 1 = 100, payee1 should get 0
    let token = client.get_subscription(&sub_id).token;
    let earnings0 = client.get_merchant_token_earnings(&payee0, &token);
    let earnings1 = client.get_merchant_token_earnings(&payee1, &token);

    assert_eq!(
        earnings0.accruals.interval, 100,
        "payee0 should receive full amount (99 + 1 remainder)"
    );
    assert_eq!(
        earnings1.accruals.interval, 0,
        "payee1 should receive nothing (share rounds to zero)"
    );

    // Verify total is conserved
    assert_eq!(
        earnings0.accruals.interval + earnings1.accruals.interval,
        100,
        "total earnings must equal charged amount"
    );
}

#[test]
fn test_split_billing_multiple_zero_share_recipients() {
    let (env, _, client, token_admin, _) = setup();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    use soroban_sdk::String;
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &0x1Fi32,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    let payee0 = Address::generate(&env); // Large share
    let payee1 = Address::generate(&env); // Zero share (1 bp)
    let payee2 = Address::generate(&env); // Zero share (1 bp)
    let payee3 = Address::generate(&env); // Zero share (1 bp)

    // Initialize configs for all payees
    for payee in [&payee0, &payee1, &payee2, &payee3].iter() {
        client.initialize_merchant_config(
            payee,
            payee,
            &0i32,
            &0x1Fi32,
            &None,
            &String::from_str(&env, "https://example.com"),
        );
    }

    // Split: payee0 gets 9997 bps, others get 1 bp each
    // With charge amount of 100:
    // payee0: 100 * 9997 / 10000 = 99
    // payee1: 100 * 1 / 10000 = 0
    // payee2: 100 * 1 / 10000 = 0
    // payee3: 100 * 1 / 10000 = 0
    // Remainder: 100 - 99 = 1 → goes to payee0
    let mut entries = Vec::new(&env);
    entries.push_back((payee0.clone(), 9997u32));
    entries.push_back((payee1.clone(), 1u32));
    entries.push_back((payee2.clone(), 1u32));
    entries.push_back((payee3.clone(), 1u32));

    let sub_id = client.create_subscription_with_split(
        &subscriber,
        &merchant,
        &100i128,
        &3600u64,
        &false,
        &None::<i128>,
        &None::<u64>,
        &entries,
    );

    token_admin.mint(&subscriber, &10_000i128);
    client.deposit_funds(&sub_id, &1_000i128, &None);

    env.ledger().set_timestamp(env.ledger().timestamp() + 3601);
    client.charge_subscription(&sub_id, &None);

    let token = client.get_subscription(&sub_id).token;
    let earnings0 = client.get_merchant_token_earnings(&payee0, &token);
    let earnings1 = client.get_merchant_token_earnings(&payee1, &token);
    let earnings2 = client.get_merchant_token_earnings(&payee2, &token);
    let earnings3 = client.get_merchant_token_earnings(&payee3, &token);

    assert_eq!(
        earnings0.accruals.interval, 100,
        "payee0 should receive 99 + 1 remainder = 100"
    );
    assert_eq!(
        earnings1.accruals.interval, 0,
        "payee1 should receive nothing"
    );
    assert_eq!(
        earnings2.accruals.interval, 0,
        "payee2 should receive nothing"
    );
    assert_eq!(
        earnings3.accruals.interval, 0,
        "payee3 should receive nothing"
    );

    // Verify total
    assert_eq!(
        earnings0.accruals.interval
            + earnings1.accruals.interval
            + earnings2.accruals.interval
            + earnings3.accruals.interval,
        100
    );
}

#[test]
fn test_split_billing_all_zero_shares_gives_all_to_first() {
    let (env, _, client, token_admin, _) = setup();

    let subscriber = Address::generate(&env);
    let merchant = Address::generate(&env);

    use soroban_sdk::String;
    client.initialize_merchant_config(
        &merchant,
        &merchant,
        &0i32,
        &0x1Fi32,
        &None,
        &String::from_str(&env, "https://example.com"),
    );

    let payee0 = Address::generate(&env);
    let payee1 = Address::generate(&env);
    let payee2 = Address::generate(&env);

    for payee in [&payee0, &payee1, &payee2].iter() {
        client.initialize_merchant_config(
            payee,
            payee,
            &0i32,
            &0x1Fi32,
            &None,
            &String::from_str(&env, "https://example.com"),
        );
    }

    // Extreme case: charge amount is 2, each payee gets 1/3
    // With integer division:
    // payee0: 2 * 3333 / 10000 = 0
    // payee1: 2 * 3333 / 10000 = 0
    // payee2: 2 * 3334 / 10000 = 0
    // Remainder: 2 - 0 = 2 → all goes to first payee
    let mut entries = Vec::new(&env);
    entries.push_back((payee0.clone(), 3333u32));
    entries.push_back((payee1.clone(), 3333u32));
    entries.push_back((payee2.clone(), 3334u32));

    let sub_id = client.create_subscription_with_split(
        &subscriber,
        &merchant,
        &2i128, // Very small charge
        &3600u64,
        &false,
        &None::<i128>,
        &None::<u64>,
        &entries,
    );

    token_admin.mint(&subscriber, &10_000i128);
    client.deposit_funds(&sub_id, &1_000i128, &None);

    env.ledger().set_timestamp(env.ledger().timestamp() + 3601);
    client.charge_subscription(&sub_id, &None);

    let token = client.get_subscription(&sub_id).token;
    let earnings0 = client.get_merchant_token_earnings(&payee0, &token);
    let earnings1 = client.get_merchant_token_earnings(&payee1, &token);
    let earnings2 = client.get_merchant_token_earnings(&payee2, &token);

    assert_eq!(
        earnings0.accruals.interval, 2,
        "payee0 gets all funds when all shares round to zero"
    );
    assert_eq!(earnings1.accruals.interval, 0);
    assert_eq!(earnings2.accruals.interval, 0);
}
