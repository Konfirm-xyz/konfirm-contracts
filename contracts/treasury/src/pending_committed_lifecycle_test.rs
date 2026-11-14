// Issue #259: `pending_committed` (behind `available_balance`,
// `committed_balance`, and `is_settlement_payable`) used to be an O(n) scan
// over every settlement id ever issued. It is now an O(1) read of a running
// total maintained incrementally by `add_pending_committed` /
// `remove_pending_committed`. A fast-but-wrong replacement would be strictly
// worse than the slow-but-correct original, so this file tracks the
// expected total independently in the test itself and cross-checks it
// against `committed_balance`/`available_balance` after every lifecycle
// transition: proposed, approved (must NOT change the total), executed, and
// expired-and-swept.
use super::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};

fn setup(env: &Env) -> (TreasuryContractClient<'_>, Vec<Address>, Address) {
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let sac = env.register_stellar_asset_contract_v2(token_admin);
    let token_id = sac.address();
    let signers = vec![env, Address::generate(env), Address::generate(env), Address::generate(env)];
    let contract_id = env.register(TreasuryContract, ());
    let client = TreasuryContractClient::new(env, &contract_id);
    client.initialize(&admin, &token_id, &signers, &2u32);
    token::StellarAssetClient::new(env, &token_id).mint(&contract_id, &1_000_000_0000i128);
    (client, signers, token_id)
}

#[test]
fn committed_and_available_balance_track_every_lifecycle_transition() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, signers, token_id) = setup(&env);
    let token_client = token::TokenClient::new(&env, &token_id);

    let signer_a = signers.get(0).unwrap();
    let signer_b = signers.get(1).unwrap();
    let merchant_a = Address::generate(&env);
    let merchant_b = Address::generate(&env);
    let merchant_c = Address::generate(&env);

    let mut expected_pending: i128 = 0;
    // Re-reads the treasury's real token balance fresh each time, rather
    // than capturing it once -- execute_settlement actually moves tokens
    // out of the treasury, so the real balance shrinks over the lifecycle
    // and available_balance must be checked against the current balance,
    // not a stale snapshot from before any execution.
    macro_rules! assert_totals {
        () => {
            assert_eq!(client.committed_balance(), expected_pending, "committed_balance drifted");
            assert_eq!(
                client.available_balance(),
                token_client.balance(&client.address) - expected_pending,
                "available_balance drifted"
            );
        };
    }

    // --- proposed: each new Pending settlement adds its amount. ---
    let amount_a: i128 = 1_000_0000;
    let id_a = client.propose_settlement(&signer_a, &merchant_a, &amount_a);
    expected_pending += amount_a;
    assert_totals!();

    let amount_b: i128 = 2_500_0000;
    let id_b = client.propose_settlement(&signer_a, &merchant_b, &amount_b);
    expected_pending += amount_b;
    assert_totals!();

    // --- approved: must NOT change the total (status/amount unchanged). ---
    client.approve_settlement(&signer_b, &id_a);
    assert_totals!();

    // --- executed: leaves Pending, removes its claim. ---
    client.execute_settlement(&signer_a, &id_a);
    expected_pending -= amount_a;
    assert_totals!();
    assert_eq!(client.get_settlement(&id_a).status, SettlementStatus::Executed);

    // A third proposal while B is still outstanding.
    let amount_c: i128 = 750_0000;
    let id_c = client.propose_settlement(&signer_a, &merchant_c, &amount_c);
    expected_pending += amount_c;
    assert_totals!();

    // --- expired-and-swept: leaves Pending via a different path, also
    // removes its claim. B is swept; C is left Pending to prove the sweep
    // only ever touches the one settlement it targets. ---
    env.ledger().with_mut(|l| l.timestamp += MAX_PENDING_LIFETIME + 1);
    client.sweep_expired_settlement(&id_b);
    expected_pending -= amount_b;
    assert_totals!();
    assert_eq!(client.get_settlement(&id_b).status, SettlementStatus::Cancelled);

    // C is also past its lifetime now (same ledger-time advance), but
    // sweeping is permissionless-per-id, not automatic -- it must still be
    // Pending and still counted until someone actually sweeps it.
    assert_eq!(client.get_settlement(&id_c).status, SettlementStatus::Pending);
    assert_totals!();

    client.sweep_expired_settlement(&id_c);
    expected_pending -= amount_c;
    assert_totals!();
    assert_eq!(expected_pending, 0, "every settlement left Pending by the end of this test");
}

#[test]
fn is_settlement_payable_reflects_the_running_total_not_a_stale_scan() {
    let env = Env::default();
    env.mock_all_auths();
    // A fresh instance funded with exactly enough for one of two
    // settlements, rather than reusing setup()'s generous mint and trying
    // to claw funds back out (the test SAC's balance isn't clawbackable by
    // default).
    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(token_admin);
    let token_id = sac.address();
    let signers = vec![&env, Address::generate(&env), Address::generate(&env)];
    let contract_id = env.register(TreasuryContract, ());
    let client = TreasuryContractClient::new(&env, &contract_id);
    client.initialize(&admin, &token_id, &signers, &2u32);
    token::StellarAssetClient::new(&env, &token_id).mint(&contract_id, &1_000_0000i128);

    let signer_a = signers.get(0).unwrap();
    let merchant_a = Address::generate(&env);
    let merchant_b = Address::generate(&env);

    let id_a = client.propose_settlement(&signer_a, &merchant_a, &1_000_0000i128);
    assert!(client.is_settlement_payable(&id_a), "the only settlement should be payable alone");

    // A second settlement now competes for the same (exhausted) balance.
    let id_b = client.propose_settlement(&signer_a, &merchant_b, &1_000_0000i128);
    assert!(!client.is_settlement_payable(&id_a), "A is no longer payable once B also claims the balance");
    assert!(!client.is_settlement_payable(&id_b), "B is no longer payable once A also claims the balance");

    // Cancel B (via expiry) and confirm A becomes payable again -- proving
    // is_settlement_payable (and the running total behind it) reacts to B
    // leaving Pending, not just to funds moving.
    let signer_b = signers.get(1).unwrap();
    client.approve_settlement(&signer_b, &id_a);
    env.ledger().with_mut(|l| l.timestamp += MAX_PENDING_LIFETIME + 1);
    client.sweep_expired_settlement(&id_b);
    assert!(client.is_settlement_payable(&id_a), "A is payable again once B's claim is released");
}

#[test]
fn running_total_matches_a_brute_force_scan_at_every_step() {
    // Belt-and-suspenders: recompute the old O(n) definition independently
    // in the test (by reading back every settlement through the public
    // get_settlement, not the contract's internal helper) and assert it
    // agrees with committed_balance() at each step. This is the same
    // property the old implementation guaranteed by construction; the new
    // one must preserve it exactly, not just "look right" on a few cases.
    let env = Env::default();
    env.mock_all_auths();
    let (client, signers, _token_id) = setup(&env);
    let signer_a = signers.get(0).unwrap();

    let mut ids = soroban_sdk::vec![&env];
    let amounts = [100_0000i128, 200_0000i128, 300_0000i128, 400_0000i128];

    let brute_force_total = |client: &TreasuryContractClient, ids: &soroban_sdk::Vec<u64>| -> i128 {
        let mut total: i128 = 0;
        for id in ids.iter() {
            let s = client.get_settlement(&id);
            if s.status == SettlementStatus::Pending {
                total += s.amount;
            }
        }
        total
    };

    for amount in amounts {
        let merchant = Address::generate(&env);
        let id = client.propose_settlement(&signer_a, &merchant, &amount);
        ids.push_back(id);
        assert_eq!(client.committed_balance(), brute_force_total(&client, &ids));
    }

    let signer_b = signers.get(1).unwrap();
    client.approve_settlement(&signer_b, &ids.get(0).unwrap());
    client.execute_settlement(&signer_a, &ids.get(0).unwrap());
    assert_eq!(client.committed_balance(), brute_force_total(&client, &ids));

    env.ledger().with_mut(|l| l.timestamp += MAX_PENDING_LIFETIME + 1);
    client.sweep_expired_settlement(&ids.get(1).unwrap());
    assert_eq!(client.committed_balance(), brute_force_total(&client, &ids));
}
