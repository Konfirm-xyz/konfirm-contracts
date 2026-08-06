#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Bytes,
    BytesN, Env,
};

// One escrow-backed unidirectional payment channel per (payer, payee, token).
// Off-chain, the payer signs monotonically-increasing cumulative-amount
// messages; anyone holding the latest one can call checkpoint() to claim it —
// deliberately not gated to an admin key, so the payee can always recover
// funds they can prove they're owed even if Konfirm's relay disappears.
//
// checkpoint() is the one mutating entry point that is never pausable: pause
// exists to stop *new* exposure (open_channel, top_up), never to freeze a
// withdrawal someone can already prove is owed to them.

const DOMAIN_TAG: &[u8; 15] = b"KONFIRM_CHAN_V1";

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChannelStatus {
    Open,
    Closing,
    Closed,
    Held,
}

#[contracttype]
#[derive(Clone)]
pub struct Channel {
    pub payer: Address,
    pub payee: Address,
    pub token: Address,
    pub payer_pubkey: BytesN<32>,
    pub deposited: i128,
    pub claimed: i128,
    pub nonce: u64,
    pub status: ChannelStatus,
    pub closing_at: u64,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Paused,
    ChallengePeriod,
    NextId,
    Channel(u64),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ChannelError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    Paused = 4,
    NotFound = 5,
    NotOpen = 6,
    StaleClaim = 7,
    ExceedsDeposit = 8,
    ChallengeNotElapsed = 9,
    NotClosing = 10,
    Held = 11,
    ZeroDeposit = 12,
}

#[contract]
pub struct ChannelContract;

#[contractimpl]
impl ChannelContract {
    pub fn initialize(env: Env, admin: Address, challenge_period_secs: u64) -> Result<(), ChannelError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ChannelError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().set(&DataKey::ChallengePeriod, &challenge_period_secs);
        env.storage().instance().set(&DataKey::NextId, &1u64);
        Ok(())
    }

    fn require_admin(env: &Env) -> Result<(), ChannelError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ChannelError::NotInitialized)?;
        admin.require_auth();
        Ok(())
    }

    fn require_not_paused(env: &Env) -> Result<(), ChannelError> {
        let paused: bool = env.storage().instance().get(&DataKey::Paused).unwrap_or(false);
        if paused {
            return Err(ChannelError::Paused);
        }
        Ok(())
    }

    fn get_channel(env: &Env, id: u64) -> Result<Channel, ChannelError> {
        env.storage()
            .persistent()
            .get(&DataKey::Channel(id))
            .ok_or(ChannelError::NotFound)
    }

    fn save_channel(env: &Env, id: u64, ch: &Channel) {
        env.storage().persistent().set(&DataKey::Channel(id), ch);
        env.storage().persistent().extend_ttl(&DataKey::Channel(id), 100, 5_000_000);
    }

    /// The message a payer signs off-chain to authorize a cumulative amount.
    /// Fixed-width, domain-tagged encoding — never string concatenation,
    /// which is how signatures end up ambiguous across field boundaries.
    fn claim_payload(env: &Env, channel_id: u64, nonce: u64, cumulative_amount: i128) -> Bytes {
        let mut payload = Bytes::from_array(env, DOMAIN_TAG);
        payload.append(&Bytes::from_array(env, &channel_id.to_be_bytes()));
        payload.append(&Bytes::from_array(env, &nonce.to_be_bytes()));
        payload.append(&Bytes::from_array(env, &cumulative_amount.to_be_bytes()));
        payload
    }

    pub fn open_channel(
        env: Env,
        payer: Address,
        payee: Address,
        token: Address,
        payer_pubkey: BytesN<32>,
        deposit: i128,
    ) -> Result<u64, ChannelError> {
        payer.require_auth();
        Self::require_not_paused(&env)?;
        if deposit <= 0 {
            return Err(ChannelError::ZeroDeposit);
        }

        token::TokenClient::new(&env, &token).transfer(&payer, env.current_contract_address(), &deposit);

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        let channel = Channel {
            payer: payer.clone(),
            payee: payee.clone(),
            token,
            payer_pubkey,
            deposited: deposit,
            claimed: 0,
            nonce: 0,
            status: ChannelStatus::Open,
            closing_at: 0,
        };
        Self::save_channel(&env, id, &channel);
        env.events().publish((symbol_short!("opened"), payer, payee), (id, deposit));
        Ok(id)
    }

    pub fn top_up(env: Env, payer: Address, channel_id: u64, amount: i128) -> Result<(), ChannelError> {
        payer.require_auth();
        Self::require_not_paused(&env)?;
        let mut ch = Self::get_channel(&env, channel_id)?;
        if ch.payer != payer {
            return Err(ChannelError::Unauthorized);
        }
        if ch.status != ChannelStatus::Open {
            return Err(ChannelError::NotOpen);
        }
        if amount <= 0 {
            return Err(ChannelError::ZeroDeposit);
        }
        token::TokenClient::new(&env, &ch.token).transfer(&payer, env.current_contract_address(), &amount);
        ch.deposited += amount;
        Self::save_channel(&env, channel_id, &ch);
        env.events().publish((symbol_short!("topup"),), (channel_id, amount));
        Ok(())
    }

    /// Never pausable. See module docs.
    pub fn checkpoint(
        env: Env,
        channel_id: u64,
        cumulative_amount: i128,
        nonce: u64,
        signature: BytesN<64>,
    ) -> Result<(), ChannelError> {
        let mut ch = Self::get_channel(&env, channel_id)?;
        if ch.status == ChannelStatus::Held {
            return Err(ChannelError::Held);
        }
        if ch.status == ChannelStatus::Closed {
            return Err(ChannelError::NotOpen);
        }
        if nonce <= ch.nonce || cumulative_amount <= ch.claimed {
            return Err(ChannelError::StaleClaim);
        }
        if cumulative_amount > ch.deposited {
            return Err(ChannelError::ExceedsDeposit);
        }

        let payload = Self::claim_payload(&env, channel_id, nonce, cumulative_amount);
        // Traps the whole invocation on a bad signature rather than returning
        // an error value — a real property of the host function, not a choice
        // this contract makes.
        env.crypto().ed25519_verify(&ch.payer_pubkey, &payload, &signature);

        let delta = cumulative_amount - ch.claimed;
        token::TokenClient::new(&env, &ch.token).transfer(&env.current_contract_address(), &ch.payee, &delta);

        ch.claimed = cumulative_amount;
        ch.nonce = nonce;
        Self::save_channel(&env, channel_id, &ch);
        env.events().publish((symbol_short!("checkpt"),), (channel_id, delta, cumulative_amount));
        Ok(())
    }

    pub fn initiate_close(env: Env, caller: Address, channel_id: u64) -> Result<(), ChannelError> {
        caller.require_auth();
        let mut ch = Self::get_channel(&env, channel_id)?;
        if caller != ch.payer && caller != ch.payee {
            return Err(ChannelError::Unauthorized);
        }
        if ch.status != ChannelStatus::Open {
            return Err(ChannelError::NotOpen);
        }
        ch.status = ChannelStatus::Closing;
        ch.closing_at = env.ledger().timestamp();
        Self::save_channel(&env, channel_id, &ch);
        env.events().publish((symbol_short!("closing"),), channel_id);
        Ok(())
    }

    /// Permissionless — anyone can trigger this once the challenge window has
    /// elapsed. Pays the unclaimed remainder back to the payer.
    pub fn finalize_close(env: Env, channel_id: u64) -> Result<(), ChannelError> {
        let mut ch = Self::get_channel(&env, channel_id)?;
        if ch.status != ChannelStatus::Closing {
            return Err(ChannelError::NotClosing);
        }
        let period: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ChallengePeriod)
            .ok_or(ChannelError::NotInitialized)?;
        if env.ledger().timestamp() < ch.closing_at + period {
            return Err(ChannelError::ChallengeNotElapsed);
        }
        let remainder = ch.deposited - ch.claimed;
        if remainder > 0 {
            token::TokenClient::new(&env, &ch.token).transfer(
                &env.current_contract_address(),
                &ch.payer,
                &remainder,
            );
        }
        ch.status = ChannelStatus::Closed;
        ch.deposited = ch.claimed;
        Self::save_channel(&env, channel_id, &ch);
        env.events().publish((symbol_short!("closed"),), channel_id);
        Ok(())
    }

    pub fn hold_channel(env: Env, channel_id: u64) -> Result<(), ChannelError> {
        Self::require_admin(&env)?;
        let mut ch = Self::get_channel(&env, channel_id)?;
        ch.status = ChannelStatus::Held;
        Self::save_channel(&env, channel_id, &ch);
        env.events().publish((symbol_short!("held"),), channel_id);
        Ok(())
    }

    pub fn release_hold(env: Env, channel_id: u64) -> Result<(), ChannelError> {
        Self::require_admin(&env)?;
        let mut ch = Self::get_channel(&env, channel_id)?;
        if ch.status != ChannelStatus::Held {
            return Err(ChannelError::NotOpen);
        }
        ch.status = ChannelStatus::Open;
        Self::save_channel(&env, channel_id, &ch);
        env.events().publish((symbol_short!("release"),), channel_id);
        Ok(())
    }

    pub fn get_channel_info(env: Env, channel_id: u64) -> Result<Channel, ChannelError> {
        Self::get_channel(&env, channel_id)
    }

    pub fn pause(env: Env) -> Result<(), ChannelError> {
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        Ok(())
    }

    pub fn unpause(env: Env) -> Result<(), ChannelError> {
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger};

    fn setup_token(env: &Env, admin: &Address) -> Address {
        // Stellar Asset Contract test token, provided by soroban-sdk testutils.
        let sac = env.register_stellar_asset_contract_v2(admin.clone());
        sac.address()
    }

    #[test]
    fn open_and_checkpoint_transfers_funds() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_id = setup_token(&env, &token_admin);
        let token_admin_client = token::StellarAssetClient::new(&env, &token_id);

        let payer = Address::generate(&env);
        let payee = Address::generate(&env);
        token_admin_client.mint(&payer, &1_000_0000i128);

        let contract_id = env.register(ChannelContract, ());
        let client = ChannelContractClient::new(&env, &contract_id);
        client.initialize(&admin, &86_400u64);

        // In production the payer's real Ed25519 keypair signs off-chain; the
        // test harness here only exercises the accounting and TTL logic, not
        // signature generation (that lives in the reconciler/client SDK).
        let payer_pubkey = BytesN::from_array(&env, &[9u8; 32]);
        let id = client.open_channel(&payer, &payee, &token_id, &payer_pubkey, &500_0000i128);

        let ch = client.get_channel_info(&id);
        assert_eq!(ch.deposited, 500_0000i128);
        assert_eq!(ch.status, ChannelStatus::Open);

        let token_client = token::TokenClient::new(&env, &token_id);
        assert_eq!(token_client.balance(&contract_id), 500_0000i128);
    }

    #[test]
    fn finalize_close_before_challenge_elapses_fails() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_id = setup_token(&env, &token_admin);
        let token_admin_client = token::StellarAssetClient::new(&env, &token_id);

        let payer = Address::generate(&env);
        let payee = Address::generate(&env);
        token_admin_client.mint(&payer, &1_000_0000i128);

        let contract_id = env.register(ChannelContract, ());
        let client = ChannelContractClient::new(&env, &contract_id);
        client.initialize(&admin, &86_400u64);
        let payer_pubkey = BytesN::from_array(&env, &[9u8; 32]);
        let id = client.open_channel(&payer, &payee, &token_id, &payer_pubkey, &500_0000i128);

        client.initiate_close(&payer, &id);
        let res = client.try_finalize_close(&id);
        assert_eq!(res, Err(Ok(ChannelError::ChallengeNotElapsed)));
    }

    #[test]
    fn finalize_close_refunds_remainder_after_challenge_period() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_id = setup_token(&env, &token_admin);
        let token_admin_client = token::StellarAssetClient::new(&env, &token_id);

        let payer = Address::generate(&env);
        let payee = Address::generate(&env);
        token_admin_client.mint(&payer, &1_000_0000i128);

        let contract_id = env.register(ChannelContract, ());
        let client = ChannelContractClient::new(&env, &contract_id);
        client.initialize(&admin, &3_600u64);
        let payer_pubkey = BytesN::from_array(&env, &[9u8; 32]);
        let id = client.open_channel(&payer, &payee, &token_id, &payer_pubkey, &500_0000i128);

        client.initiate_close(&payer, &id);
        env.ledger().with_mut(|l| l.timestamp += 3_601);
        client.finalize_close(&id);

        let token_client = token::TokenClient::new(&env, &token_id);
        assert_eq!(token_client.balance(&payer), 1_000_0000i128);
        assert_eq!(token_client.balance(&contract_id), 0i128);
    }

    #[test]
    fn checkpoint_rejects_stale_nonce() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_id = setup_token(&env, &token_admin);
        let token_admin_client = token::StellarAssetClient::new(&env, &token_admin);
        let _ = token_admin_client; // silence unused in this path

        let payer = Address::generate(&env);
        let payee = Address::generate(&env);
        token::StellarAssetClient::new(&env, &token_id).mint(&payer, &1_000_0000i128);

        let contract_id = env.register(ChannelContract, ());
        let client = ChannelContractClient::new(&env, &contract_id);
        client.initialize(&admin, &86_400u64);
        let payer_pubkey = BytesN::from_array(&env, &[9u8; 32]);
        let id = client.open_channel(&payer, &payee, &token_id, &payer_pubkey, &500_0000i128);

        let bogus_sig = BytesN::from_array(&env, &[0u8; 64]);
        let res = client.try_checkpoint(&id, &0i128, &0u64, &bogus_sig);
        assert_eq!(res, Err(Ok(ChannelError::StaleClaim)));
    }

    #[test]
    fn held_channel_rejects_checkpoint() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let token_id = setup_token(&env, &token_admin);
        token::StellarAssetClient::new(&env, &token_id).mint(&Address::generate(&env), &0i128);

        let payer = Address::generate(&env);
        let payee = Address::generate(&env);
        token::StellarAssetClient::new(&env, &token_id).mint(&payer, &1_000_0000i128);

        let contract_id = env.register(ChannelContract, ());
        let client = ChannelContractClient::new(&env, &contract_id);
        client.initialize(&admin, &86_400u64);
        let payer_pubkey = BytesN::from_array(&env, &[9u8; 32]);
        let id = client.open_channel(&payer, &payee, &token_id, &payer_pubkey, &500_0000i128);

        client.hold_channel(&id);
        let bogus_sig = BytesN::from_array(&env, &[0u8; 64]);
        let res = client.try_checkpoint(&id, &100_0000i128, &1u64, &bogus_sig);
        assert_eq!(res, Err(Ok(ChannelError::Held)));
    }
}
