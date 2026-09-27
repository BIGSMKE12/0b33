//! Tests for #600: statement compaction must preserve aggregate totals.
//!
//! After `compact_subscription_statements`, the compacted aggregate's totals
//! must equal the sum of the pruned statements, and the retained head must
//! still be independently queryable — nothing lost, nothing double-counted.

use crate::statements::{compact_subscription_statements, get_compacted_aggregate};
use crate::types::BillingChargeKind;
use crate::{SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{testutils::Address as _, Address, Env};

fn setup() -> (Env, Address) {
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    (env, contract_id)
}

/// Appends `count` statements (amounts 100, 200, 300, ...) for `subscription_id`,
/// alternating charge kinds so the per-kind aggregate breakdown is exercised too.
fn append_known_sequence(env: &Env, contract_id: &Address, subscription_id: u32, count: u32) -> i128 {
    let mut total = 0i128;
    env.as_contract(contract_id, || {
        let merchant = Address::generate(env);
        let token = Address::generate(env);
        for i in 0..count {
            let amount = 100i128 * (i as i128 + 1);
            let kind = match i % 3 {
                0 => BillingChargeKind::Interval,
                1 => BillingChargeKind::Usage,
                _ => BillingChargeKind::OneOff,
            };
            crate::statements::append_statement(
                env,
                subscription_id,
                amount,
                merchant.clone(),
                token.clone(),
                kind,
                i as u64,
                i as u64 + 1000,
            )
            .unwrap();
            total += amount;
        }
    });
    total
}

/// Compacting with `keep_recent=5` prunes the oldest statements and the
/// compacted aggregate's total equals exactly the sum of what was pruned.
#[test]
fn compaction_preserves_totals_keep_recent_five() {
    let (env, contract_id) = setup();
    let sub_id = 1u32;
    let appended_total = append_known_sequence(&env, &contract_id, sub_id, 12);

    let summary = env.as_contract(&contract_id, || {
        compact_subscription_statements(&env, sub_id, Some(5)).unwrap()
    });

    // 12 statements, keep the 5 most recent -> 7 pruned.
    assert_eq!(summary.pruned_count, 7);
    assert_eq!(summary.kept_count, 5);

    let pruned_sum: i128 = (1..=7i128).map(|i| 100 * i).sum();
    let retained_sum: i128 = (8..=12i128).map(|i| 100 * i).sum();
    assert_eq!(summary.total_pruned_amount, pruned_sum);
    assert_eq!(pruned_sum + retained_sum, appended_total);

    let aggregate = env.as_contract(&contract_id, || get_compacted_aggregate(&env, sub_id));
    assert_eq!(aggregate.pruned_count, 7);
    assert_eq!(aggregate.total_amount, pruned_sum);

    // Aggregate's total plus what's still retrievable via the paginated
    // (non-pruned) list must reconstruct the original grand total exactly.
    let page = env.as_contract(&contract_id, || {
        crate::statements::get_statements_by_subscription_offset(&env, sub_id, 0, 100, false).unwrap()
    });
    assert_eq!(page.statements.len(), 5);
    let retained_from_page: i128 = page.statements.iter().map(|s| s.amount).sum();
    assert_eq!(retained_from_page, retained_sum);
    assert_eq!(aggregate.total_amount + retained_from_page, appended_total);

    // Per-kind breakdown also sums correctly (i=0..6 pruned => kinds 0,1,2,0,1,2,0 => Interval amounts at i=0,3,6).
    let expected_interval: i128 = [1, 4, 7].iter().map(|&i| 100 * i as i128).sum();
    let expected_usage: i128 = [2, 5].iter().map(|&i| 100 * i as i128).sum();
    let expected_one_off: i128 = [3, 6].iter().map(|&i| 100 * i as i128).sum();
    assert_eq!(aggregate.totals.interval, expected_interval);
    assert_eq!(aggregate.totals.usage, expected_usage);
    assert_eq!(aggregate.totals.one_off, expected_one_off);
}

/// Running compaction twice accumulates into the same aggregate rather than
/// overwriting it.
#[test]
fn repeated_compaction_accumulates_the_aggregate() {
    let (env, contract_id) = setup();
    let sub_id = 2u32;
    append_known_sequence(&env, &contract_id, sub_id, 10);

    let first = env.as_contract(&contract_id, || {
        compact_subscription_statements(&env, sub_id, Some(6)).unwrap()
    });
    assert_eq!(first.pruned_count, 4);

    // Append more, then compact again with a tighter retention.
    let more_total = append_known_sequence(&env, &contract_id, sub_id, 3);
    let second = env.as_contract(&contract_id, || {
        compact_subscription_statements(&env, sub_id, Some(2)).unwrap()
    });
    // Before: 6 kept. After appending 3: 9 total. keep_recent=2 -> prune 7.
    assert_eq!(second.pruned_count, 7);

    let aggregate = env.as_contract(&contract_id, || get_compacted_aggregate(&env, sub_id));
    assert_eq!(aggregate.pruned_count, first.pruned_count + second.pruned_count);
    assert_eq!(aggregate.total_amount, first.total_pruned_amount + second.total_pruned_amount);

    let _ = more_total;
}

/// `keep_recent` greater than the total statement count is a no-op.
#[test]
fn keep_recent_greater_than_total_is_a_noop() {
    let (env, contract_id) = setup();
    let sub_id = 3u32;
    append_known_sequence(&env, &contract_id, sub_id, 4);

    let summary = env.as_contract(&contract_id, || {
        compact_subscription_statements(&env, sub_id, Some(100)).unwrap()
    });
    assert_eq!(summary.pruned_count, 0);
    assert_eq!(summary.kept_count, 4);
    assert_eq!(summary.total_pruned_amount, 0);

    let aggregate = env.as_contract(&contract_id, || get_compacted_aggregate(&env, sub_id));
    assert_eq!(aggregate.pruned_count, 0);
    assert_eq!(aggregate.total_amount, 0);
}

/// `keep_recent = 0` prunes every statement.
#[test]
fn keep_recent_zero_prunes_everything() {
    let (env, contract_id) = setup();
    let sub_id = 4u32;
    let appended_total = append_known_sequence(&env, &contract_id, sub_id, 5);

    let summary = env.as_contract(&contract_id, || {
        compact_subscription_statements(&env, sub_id, Some(0)).unwrap()
    });
    assert_eq!(summary.pruned_count, 5);
    assert_eq!(summary.kept_count, 0);
    assert_eq!(summary.total_pruned_amount, appended_total);

    let page = env.as_contract(&contract_id, || {
        crate::statements::get_statements_by_subscription_offset(&env, sub_id, 0, 100, false).unwrap()
    });
    assert_eq!(page.statements.len(), 0);
    assert_eq!(page.total, 0);
}

/// Compacting a subscription with no statement history at all is a clean no-op.
#[test]
fn empty_statement_history_is_a_noop() {
    let (env, contract_id) = setup();
    let sub_id = 5u32;

    let summary = env.as_contract(&contract_id, || {
        compact_subscription_statements(&env, sub_id, Some(5)).unwrap()
    });
    assert_eq!(summary.pruned_count, 0);
    assert_eq!(summary.kept_count, 0);
    assert_eq!(summary.total_pruned_amount, 0);

    let aggregate = env.as_contract(&contract_id, || get_compacted_aggregate(&env, sub_id));
    assert_eq!(aggregate.pruned_count, 0);
    assert_eq!(aggregate.oldest_period_start, None);
    assert_eq!(aggregate.newest_period_end, None);
}

/// Falls back to the global retention config when no per-call override is given.
#[test]
fn falls_back_to_global_retention_config() {
    let (env, contract_id) = setup();
    let sub_id = 6u32;
    append_known_sequence(&env, &contract_id, sub_id, 8);

    env.as_contract(&contract_id, || {
        crate::statements::set_retention_config(&env, 3);
    });

    let summary = env.as_contract(&contract_id, || {
        compact_subscription_statements(&env, sub_id, None).unwrap()
    });
    assert_eq!(summary.kept_count, 3);
    assert_eq!(summary.pruned_count, 5);
}

/// INVARIANT: After compaction, sum(retained statements) + compacted_aggregate.total_amount
/// must equal the original total_charged amount recorded elsewhere in the subscription.
///
/// This test verifies that statement compaction preserves the total charged amount invariant,
/// ensuring no billing history is lost or double-counted during compaction.
#[test]
fn compaction_preserves_total_charged_amount_invariant() {
    let (env, contract_id) = setup();
    let sub_id = 7u32;
    
    // Append statements with known amounts
    let appended_total = append_known_sequence(&env, &contract_id, sub_id, 15);
    
    // Record the expected total before compaction
    let expected_total = appended_total;
    
    // Compact, keeping recent 5 statements
    let summary = env.as_contract(&contract_id, || {
        compact_subscription_statements(&env, sub_id, Some(5)).unwrap()
    });
    
    // Verify pruned + kept = original total
    assert_eq!(summary.pruned_count + summary.kept_count, 15);
    
    // Get the compacted aggregate
    let aggregate = env.as_contract(&contract_id, || get_compacted_aggregate(&env, sub_id));
    
    // Get remaining statements
    let page = env.as_contract(&contract_id, || {
        crate::statements::get_statements_by_subscription_offset(&env, sub_id, 0, 100, false).unwrap()
    });
    
    // Sum of retained statements
    let retained_sum: i128 = page.statements.iter().map(|s| s.amount).sum();
    
    // INVARIANT CHECK: compacted_aggregate.total_amount + sum(retained) == original total
    let reconstructed_total = aggregate.total_amount + retained_sum;
    assert_eq!(
        reconstructed_total, 
        expected_total,
        "Compaction must preserve total charged amount: aggregate({}) + retained({}) != original({})",
        aggregate.total_amount,
        retained_sum,
        expected_total
    );
    
    // Verify the sum also matches what we know was pruned vs kept
    assert_eq!(summary.total_pruned_amount, aggregate.total_amount);
}

/// INVARIANT DOCUMENTATION: Verify that the total charged amount invariant is documented
/// in docs/billing_statements.md (this test ensures we don't forget the documentation).
///
/// This is a documentation test — it verifies that the billing_statements.md file exists
/// and contains the invariant documentation. If this test is added before the docs exist,
/// it should fail to remind us to document the invariant.
#[test]
fn verify_compaction_invariant_is_documented() {
    // This test reads the documentation file to ensure the invariant is documented.
    // If the file doesn't exist or doesn't contain the invariant, the test fails.
    
    let docs_path = std::path::Path::new("../../docs/billing_statements.md");
    
    if !docs_path.exists() {
        panic!(
            "Documentation file missing: docs/billing_statements.md must document the \
             'total charged amount invariant' for statement compaction"
        );
    }
    
    let contents = std::fs::read_to_string(docs_path)
        .expect("Failed to read docs/billing_statements.md");
    
    // Check that the invariant is mentioned
    let has_invariant_doc = contents.contains("total charged amount") 
        || contents.contains("compaction preserves")
        || contents.contains("invariant");
    
    assert!(
        has_invariant_doc,
        "docs/billing_statements.md must document the total charged amount invariant for compaction"
    );
}
