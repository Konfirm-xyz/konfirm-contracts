// Issue #254: treasury's `require_allowed` has called into a real
// ComplianceClient from both propose_settlement and execute_settlement
// since treasury was wired up, but no test anywhere ever registered
// ComplianceContract and TreasuryContract in the same Env -- so the actual
// on-chain-rejection path (the whole point of the integration) had zero
// coverage. This file is that same-Env integration test, for treasury.
use super::*;
use konfirm_compliance::ComplianceContract;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::token;

fn setup(env: &Env) -> (TreasuryContractClient<'_>, Address, Vec<Address>, Address) {
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let sac = env.register_stellar_asset_contract_v2(token_admin);
    let token_id = sac.address();
    let signers = vec![env, Address::generate(env), Address::generate(env)];
    let contract_id = env.register(TreasuryContract, ());
    let client = TreasuryContractClient::new(env, &contract_id);
    client.initialize(&admin, &token_id, &signers, &2u32);
    token::StellarAssetClient::new(env, &token_id).mint(&contract_id, &100_000_0000i128);

    let compliance_admin = Address::generate(env);
    let compliance_id = env.register(ComplianceContract, ());
    let compliance_client =
        konfirm_compliance::ComplianceContractClient::new(env, &compliance_id);
    compliance_client.initialize(&compliance_admin);
    client.set_compliance(&compliance_id);

    (client, admin, signers, compliance_id)
}

#[test]
fn propose_settlement_rejects_a_blocked_merchant_via_real_cross_contract_call() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, signers, compliance_id) = setup(&env);
    let compliance_client = konfirm_compliance::ComplianceContractClient::new(&env, &compliance_id);

    let merchant = Address::generate(&env);
    compliance_client.block_address(&merchant);

    let res = client.try_propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);
    assert_eq!(res, Err(Ok(TreasuryError::ComplianceBlocked)));
}

#[test]
fn execute_settlement_rejects_a_merchant_blocked_after_proposal_via_real_cross_contract_call() {
    // The merchant was clean at propose time and is blocked afterwards --
    // proving execute_settlement re-checks compliance independently right
    // before the real transfer, rather than only trusting the state at
    // proposal time.
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, signers, compliance_id) = setup(&env);
    let compliance_client = konfirm_compliance::ComplianceContractClient::new(&env, &compliance_id);
    let signer_a = signers.get(0).unwrap();
    let signer_b = signers.get(1).unwrap();

    let merchant = Address::generate(&env);
    let id = client.propose_settlement(&signer_a, &merchant, &1_000_0000i128);
    client.approve_settlement(&signer_b, &id);

    compliance_client.block_address(&merchant);

    let res = client.try_execute_settlement(&signer_a, &id);
    assert_eq!(res, Err(Ok(TreasuryError::ComplianceBlocked)));

    // The settlement must stay Pending -- a reverted execute is not a paid one.
    assert_eq!(client.get_settlement(&id).status, SettlementStatus::Pending);
}

#[test]
fn execute_settlement_succeeds_once_cleared() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, signers, compliance_id) = setup(&env);
    let compliance_client = konfirm_compliance::ComplianceContractClient::new(&env, &compliance_id);
    let signer_a = signers.get(0).unwrap();
    let signer_b = signers.get(1).unwrap();

    let merchant = Address::generate(&env);
    compliance_client.block_address(&merchant);
    let res = client.try_propose_settlement(&signer_a, &merchant, &1_000_0000i128);
    assert_eq!(res, Err(Ok(TreasuryError::ComplianceBlocked)));

    compliance_client.clear_address(&merchant);
    let id = client.propose_settlement(&signer_a, &merchant, &1_000_0000i128);
    client.approve_settlement(&signer_b, &id);
    client.execute_settlement(&signer_a, &id);
    assert_eq!(client.get_settlement(&id).status, SettlementStatus::Executed);
}

#[test]
fn propose_settlement_is_unaffected_when_compliance_is_never_configured() {
    // No set_compliance call at all -- advisory mode, no-op. This is the
    // inverse case #254's acceptance criteria also asks for.
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(token_admin);
    let token_id = sac.address();
    let signers = vec![&env, Address::generate(&env), Address::generate(&env)];
    let contract_id = env.register(TreasuryContract, ());
    let client = TreasuryContractClient::new(&env, &contract_id);
    client.initialize(&admin, &token_id, &signers, &2u32);

    let merchant = Address::generate(&env);
    let id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);
    assert_eq!(client.get_settlement(&id).status, SettlementStatus::Pending);
}
