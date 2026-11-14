// Issue #254: payment's `require_allowed` has called into a real
// ComplianceClient since payment/treasury were wired up, but no test
// anywhere ever registered ComplianceContract and PaymentContract in the
// same Env -- so the actual on-chain-rejection path (the whole point of the
// integration) had zero coverage. This file is that same-Env integration
// test, for payment.
use super::*;
use konfirm_compliance::ComplianceContract;
use soroban_sdk::testutils::Address as _;

fn setup(env: &Env) -> (PaymentContractClient<'_>, Address, Address) {
    let admin = Address::generate(env);
    let contract_id = env.register(PaymentContract, ());
    let client = PaymentContractClient::new(env, &contract_id);
    client.initialize(&admin);

    let compliance_admin = Address::generate(env);
    let compliance_id = env.register(ComplianceContract, ());
    let compliance_client =
        konfirm_compliance::ComplianceContractClient::new(env, &compliance_id);
    compliance_client.initialize(&compliance_admin);
    client.set_compliance(&compliance_id);

    (client, admin, compliance_id)
}

#[test]
fn record_payment_rejects_a_blocked_merchant_via_real_cross_contract_call() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, compliance_id) = setup(&env);
    let compliance_client = konfirm_compliance::ComplianceContractClient::new(&env, &compliance_id);

    let merchant = Address::generate(&env);
    let payer = Address::generate(&env);
    let tx_hash = BytesN::from_array(&env, &[1u8; 32]);

    compliance_client.block_address(&merchant);

    let res = client.try_record_payment(&merchant, &payer, &1u64, &100_0000i128, &tx_hash);
    assert_eq!(res, Err(Ok(PaymentError::ComplianceBlocked)));
}

#[test]
fn record_payment_rejects_a_blocked_payer_via_real_cross_contract_call() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, compliance_id) = setup(&env);
    let compliance_client = konfirm_compliance::ComplianceContractClient::new(&env, &compliance_id);

    let merchant = Address::generate(&env);
    let payer = Address::generate(&env);
    let tx_hash = BytesN::from_array(&env, &[2u8; 32]);

    compliance_client.block_address(&payer);

    let res = client.try_record_payment(&merchant, &payer, &1u64, &100_0000i128, &tx_hash);
    assert_eq!(res, Err(Ok(PaymentError::ComplianceBlocked)));
}

#[test]
fn record_payment_succeeds_once_cleared() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, _admin, compliance_id) = setup(&env);
    let compliance_client = konfirm_compliance::ComplianceContractClient::new(&env, &compliance_id);

    let merchant = Address::generate(&env);
    let payer = Address::generate(&env);
    let tx_hash = BytesN::from_array(&env, &[3u8; 32]);

    compliance_client.block_address(&merchant);
    let res = client.try_record_payment(&merchant, &payer, &1u64, &100_0000i128, &tx_hash);
    assert_eq!(res, Err(Ok(PaymentError::ComplianceBlocked)));

    compliance_client.clear_address(&merchant);
    let id = client.record_payment(&merchant, &payer, &1u64, &100_0000i128, &tx_hash);
    let rec = client.get_payment(&id);
    assert_eq!(rec.merchant, merchant);
}

#[test]
fn record_payment_is_unaffected_when_compliance_is_never_configured() {
    // No set_compliance call at all -- advisory mode, no-op. This is the
    // inverse case #254's acceptance criteria also asks for.
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let contract_id = env.register(PaymentContract, ());
    let client = PaymentContractClient::new(&env, &contract_id);
    client.initialize(&admin);

    let merchant = Address::generate(&env);
    let payer = Address::generate(&env);
    let tx_hash = BytesN::from_array(&env, &[4u8; 32]);

    let id = client.record_payment(&merchant, &payer, &1u64, &100_0000i128, &tx_hash);
    assert_eq!(client.get_payment(&id).merchant, merchant);
}
