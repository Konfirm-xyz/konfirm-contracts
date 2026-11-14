// Issue #251/#254: channel had zero compliance wiring at all until this
// change, and the new ComplianceBlocked path has zero coverage without this
// file -- the same-Env integration test issue #254 requires for any
// contract that calls into another one.
use super::*;
use konfirm_compliance::ComplianceContract;
use soroban_sdk::testutils::Address as _;

fn setup_token(env: &Env, admin: &Address) -> Address {
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    sac.address()
}

// Registers ChannelContract + ComplianceContract in one Env, initializes
// both, points channel at compliance, and allowlists a funded token. Returns
// everything a test needs to call open_channel.
fn setup(env: &Env) -> (ChannelContractClient<'_>, Address, Address, Address) {
    let admin = Address::generate(env);
    let token_admin = Address::generate(env);
    let token_id = setup_token(env, &token_admin);

    let contract_id = env.register(ChannelContract, ());
    let client = ChannelContractClient::new(env, &contract_id);
    client.initialize(&admin, &86_400u64);
    client.set_token_allowed(&token_id, &true);

    let compliance_admin = Address::generate(env);
    let compliance_id = env.register(ComplianceContract, ());
    let compliance_client =
        konfirm_compliance::ComplianceContractClient::new(env, &compliance_id);
    compliance_client.initialize(&compliance_admin);
    client.set_compliance(&compliance_id);

    (client, token_id, token_admin, compliance_id)
}

#[test]
fn open_channel_rejects_a_blocked_payer_via_real_cross_contract_call() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, token_id, token_admin, compliance_id) = setup(&env);
    let compliance_client = konfirm_compliance::ComplianceContractClient::new(&env, &compliance_id);

    let payer = Address::generate(&env);
    let payee = Address::generate(&env);
    token::StellarAssetClient::new(&env, &token_id).mint(&payer, &1_000_0000i128);
    let _ = token_admin;

    compliance_client.block_address(&payer);

    let payer_pubkey = BytesN::from_array(&env, &[9u8; 32]);
    let res = client.try_open_channel(&payer, &payee, &token_id, &payer_pubkey, &500_0000i128, &None);
    assert_eq!(res, Err(Ok(ChannelError::ComplianceBlocked)));

    // No deposit should have moved.
    let token_client = token::TokenClient::new(&env, &token_id);
    assert_eq!(token_client.balance(&payer), 1_000_0000i128);
    assert_eq!(token_client.balance(&client.address), 0i128);
}

#[test]
fn open_channel_rejects_a_blocked_payee_via_real_cross_contract_call() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, token_id, token_admin, compliance_id) = setup(&env);
    let compliance_client = konfirm_compliance::ComplianceContractClient::new(&env, &compliance_id);

    let payer = Address::generate(&env);
    let payee = Address::generate(&env);
    token::StellarAssetClient::new(&env, &token_id).mint(&payer, &1_000_0000i128);
    let _ = token_admin;

    compliance_client.block_address(&payee);

    let payer_pubkey = BytesN::from_array(&env, &[9u8; 32]);
    let res = client.try_open_channel(&payer, &payee, &token_id, &payer_pubkey, &500_0000i128, &None);
    assert_eq!(res, Err(Ok(ChannelError::ComplianceBlocked)));
}

#[test]
fn top_up_rejects_once_the_payee_is_blocked_after_open() {
    // Compliance status can change after a channel is already open. top_up
    // adds new deposit capacity -- new exposure -- and must re-check both
    // parties even though open_channel already checked them once.
    let env = Env::default();
    env.mock_all_auths();
    let (client, token_id, token_admin, compliance_id) = setup(&env);
    let compliance_client = konfirm_compliance::ComplianceContractClient::new(&env, &compliance_id);

    let payer = Address::generate(&env);
    let payee = Address::generate(&env);
    token::StellarAssetClient::new(&env, &token_id).mint(&payer, &1_000_0000i128);
    let _ = token_admin;

    let payer_pubkey = BytesN::from_array(&env, &[9u8; 32]);
    let id = client.open_channel(&payer, &payee, &token_id, &payer_pubkey, &500_0000i128, &None);

    compliance_client.block_address(&payee);

    let res = client.try_top_up(&payer, &id, &100_0000i128);
    assert_eq!(res, Err(Ok(ChannelError::ComplianceBlocked)));
    assert_eq!(client.get_channel_info(&id).deposited, 500_0000i128);
}

#[test]
fn open_channel_succeeds_once_cleared() {
    let env = Env::default();
    env.mock_all_auths();
    let (client, token_id, token_admin, compliance_id) = setup(&env);
    let compliance_client = konfirm_compliance::ComplianceContractClient::new(&env, &compliance_id);

    let payer = Address::generate(&env);
    let payee = Address::generate(&env);
    token::StellarAssetClient::new(&env, &token_id).mint(&payer, &1_000_0000i128);
    let _ = token_admin;

    compliance_client.block_address(&payer);
    let payer_pubkey = BytesN::from_array(&env, &[9u8; 32]);
    let res = client.try_open_channel(&payer, &payee, &token_id, &payer_pubkey, &500_0000i128, &None);
    assert_eq!(res, Err(Ok(ChannelError::ComplianceBlocked)));

    compliance_client.clear_address(&payer);
    let id = client.open_channel(&payer, &payee, &token_id, &payer_pubkey, &500_0000i128, &None);
    assert_eq!(client.get_channel_info(&id).status, ChannelStatus::Open);
}

#[test]
fn open_channel_is_unaffected_when_compliance_is_never_configured() {
    // No set_compliance call at all -- advisory mode, no-op. This is the
    // inverse case #254's acceptance criteria also asks for.
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_id = setup_token(&env, &token_admin);
    token::StellarAssetClient::new(&env, &token_id).mint(&Address::generate(&env), &0i128);

    let contract_id = env.register(ChannelContract, ());
    let client = ChannelContractClient::new(&env, &contract_id);
    client.initialize(&admin, &86_400u64);
    client.set_token_allowed(&token_id, &true);

    let payer = Address::generate(&env);
    let payee = Address::generate(&env);
    token::StellarAssetClient::new(&env, &token_id).mint(&payer, &1_000_0000i128);
    let payer_pubkey = BytesN::from_array(&env, &[9u8; 32]);
    let id = client.open_channel(&payer, &payee, &token_id, &payer_pubkey, &500_0000i128, &None);
    assert_eq!(client.get_channel_info(&id).status, ChannelStatus::Open);
}

#[test]
fn checkpoint_still_pays_out_to_an_already_blocked_payee() {
    // By design (see require_allowed's doc comment): checkpoint is never
    // compliance-gated, mirroring how it is never pause-gated. A payee who
    // was allowed when the channel opened, and later gets blocked, must
    // still be able to recover funds they can prove are owed -- compliance
    // only prevents *new* exposure (open_channel/top_up), it never freezes
    // an already-signed claim.
    use ed25519_dalek::{Signer, SigningKey};

    let env = Env::default();
    env.mock_all_auths();
    let (client, token_id, token_admin, compliance_id) = setup(&env);
    let compliance_client = konfirm_compliance::ComplianceContractClient::new(&env, &compliance_id);

    let payer = Address::generate(&env);
    let payee = Address::generate(&env);
    token::StellarAssetClient::new(&env, &token_id).mint(&payer, &1_000_0000i128);
    let _ = token_admin;

    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let payer_pubkey = BytesN::from_array(&env, &signing_key.verifying_key().to_bytes());
    let id = client.open_channel(&payer, &payee, &token_id, &payer_pubkey, &500_0000i128, &None);

    compliance_client.block_address(&payee);

    let cumulative_amount: i128 = 100_0000;
    let nonce: u64 = 1;
    let payload = ChannelContract::claim_payload(&env, &client.address, id, nonce, cumulative_amount);
    let mut buf = [0u8; 103];
    payload.copy_into_slice(&mut buf);
    let signature = BytesN::from_array(&env, &signing_key.sign(&buf).to_bytes());

    client.checkpoint(&id, &cumulative_amount, &nonce, &signature);

    let token_client = token::TokenClient::new(&env, &token_id);
    assert_eq!(token_client.balance(&payee), cumulative_amount);
}
