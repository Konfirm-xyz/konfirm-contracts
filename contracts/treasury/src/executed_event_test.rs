use super::*;
use soroban_sdk::{
    testutils::{Address as _, Events as _},
    token::{StellarAssetClient, TokenClient},
    Address, Env, IntoVal, Symbol, Val, Vec,
};

fn setup() -> (Env, TreasuryContractClient<'static>, Address, Address, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let signer_a = Address::generate(&env);
    let signer_b = Address::generate(&env);
    let merchant = Address::generate(&env);

    let token_admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_addr = sac.address();

    let contract_id = env.register(TreasuryContract, ());
    let client = TreasuryContractClient::new(&env, &contract_id);

    let mut signers = Vec::new(&env);
    signers.push_back(signer_a.clone());
    signers.push_back(signer_b.clone());
    client.initialize(&admin, &token_addr, &signers, &2u32);

    // Fund the treasury so it can pay out.
    let asset_admin = StellarAssetClient::new(&env, &token_addr);
    asset_admin.mint(&contract_id, &1_000i128);

    (env, client, signer_a, signer_b, merchant, token_addr)
}

#[test]
fn executed_event_carries_amount_and_merchant() {
    let (env, client, signer_a, signer_b, merchant, token_addr) = setup();
    let amount: i128 = 250;

    let id = client.propose_settlement(&signer_a, &merchant, &amount);
    client.approve_settlement(&signer_b, &id);
    client.execute_settlement(&signer_a, &id);

    // Check events right after execute_settlement: a Soroban test env scopes
    // recorded events to the most recent top-level invocation, so the token
    // balance read below (a cross-contract call of its own) would otherwise
    // clear them before this assertion ran.
    //
    // The `executed` event's topic carries the merchant and its data carries
    // the settlement id and the paid amount.
    let topics: Vec<Val> = (Symbol::new(&env, "executed"), merchant.clone()).into_val(&env);
    let data: Val = (id, amount).into_val(&env);
    assert_eq!(
        env.events().all().filter_by_contract(&client.address),
        soroban_sdk::vec![&env, (client.address.clone(), topics, data)]
    );

    // The merchant actually received the funds.
    let token = TokenClient::new(&env, &token_addr);
    assert_eq!(token.balance(&merchant), amount);
}
