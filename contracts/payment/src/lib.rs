#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, symbol_short, Address, BytesN, Env};

// This contract is an attestation layer, not an escrow. Funds for a checkout
// payment move directly payer -> merchant muxed address via a classic
// Stellar payment operation; this contract never holds them. record_payment
// is called by the reconciler only *after* it has independently observed and
// verified that transfer on Horizon — the on-chain record exists because the
// payment already happened, never before.

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct PaymentRecord {
    pub merchant: Address,
    pub payer: Address,
    pub link_id: u64,
    pub amount: i128,
    pub tx_hash: BytesN<32>,
    pub refunded: bool,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Paused,
    NextId,
    Payment(u64),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum PaymentError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    Paused = 4,
    NotFound = 5,
    ZeroAmount = 6,
    AlreadyRefunded = 7,
}

#[contract]
pub struct PaymentContract;

#[contractimpl]
impl PaymentContract {
    pub fn initialize(env: Env, admin: Address) -> Result<(), PaymentError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(PaymentError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().set(&DataKey::NextId, &1u64);
        Ok(())
    }

    fn require_admin(env: &Env) -> Result<(), PaymentError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(PaymentError::NotInitialized)?;
        admin.require_auth();
        Ok(())
    }

    fn require_not_paused(env: &Env) -> Result<(), PaymentError> {
        let paused: bool = env.storage().instance().get(&DataKey::Paused).unwrap_or(false);
        if paused {
            return Err(PaymentError::Paused);
        }
        Ok(())
    }

    pub fn record_payment(
        env: Env,
        merchant: Address,
        payer: Address,
        link_id: u64,
        amount: i128,
        tx_hash: BytesN<32>,
    ) -> Result<u64, PaymentError> {
        Self::require_admin(&env)?;
        Self::require_not_paused(&env)?;
        if amount <= 0 {
            return Err(PaymentError::ZeroAmount);
        }

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        let record = PaymentRecord {
            merchant: merchant.clone(),
            payer,
            link_id,
            amount,
            tx_hash,
            refunded: false,
        };
        env.storage().persistent().set(&DataKey::Payment(id), &record);
        env.events().publish((symbol_short!("recorded"), merchant), (id, link_id, amount));
        Ok(id)
    }

    pub fn mark_refunded(env: Env, payment_id: u64) -> Result<(), PaymentError> {
        Self::require_admin(&env)?;
        Self::require_not_paused(&env)?;
        let mut record: PaymentRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .ok_or(PaymentError::NotFound)?;
        if record.refunded {
            return Err(PaymentError::AlreadyRefunded);
        }
        record.refunded = true;
        env.storage().persistent().set(&DataKey::Payment(payment_id), &record);
        env.events().publish((symbol_short!("refunded"),), payment_id);
        Ok(())
    }

    pub fn get_payment(env: Env, payment_id: u64) -> Result<PaymentRecord, PaymentError> {
        env.storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .ok_or(PaymentError::NotFound)
    }

    pub fn pause(env: Env) -> Result<(), PaymentError> {
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        Ok(())
    }

    pub fn unpause(env: Env) -> Result<(), PaymentError> {
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    fn setup(env: &Env) -> (PaymentContractClient<'_>, Address) {
        let admin = Address::generate(env);
        let contract_id = env.register(PaymentContract, ());
        let client = PaymentContractClient::new(env, &contract_id);
        client.initialize(&admin);
        (client, admin)
    }

    #[test]
    fn record_and_fetch_payment() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);
        let merchant = Address::generate(&env);
        let payer = Address::generate(&env);
        let tx_hash = BytesN::from_array(&env, &[7u8; 32]);

        let id = client.record_payment(&merchant, &payer, &42u64, &1_000_0000i128, &tx_hash);
        let rec = client.get_payment(&id);
        assert_eq!(rec.merchant, merchant);
        assert_eq!(rec.link_id, 42u64);
        assert_eq!(rec.amount, 1_000_0000i128);
        assert!(!rec.refunded);
    }

    #[test]
    fn zero_amount_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);
        let merchant = Address::generate(&env);
        let payer = Address::generate(&env);
        let tx_hash = BytesN::from_array(&env, &[0u8; 32]);

        let res = client.try_record_payment(&merchant, &payer, &1u64, &0i128, &tx_hash);
        assert_eq!(res, Err(Ok(PaymentError::ZeroAmount)));
    }

    #[test]
    fn double_refund_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);
        let merchant = Address::generate(&env);
        let payer = Address::generate(&env);
        let tx_hash = BytesN::from_array(&env, &[1u8; 32]);

        let id = client.record_payment(&merchant, &payer, &1u64, &500_0000i128, &tx_hash);
        client.mark_refunded(&id);
        let res = client.try_mark_refunded(&id);
        assert_eq!(res, Err(Ok(PaymentError::AlreadyRefunded)));
    }

    #[test]
    fn unknown_payment_not_found() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);
        let res = client.try_get_payment(&999u64);
        assert_eq!(res, Err(Ok(PaymentError::NotFound)));
    }

    #[test]
    fn paused_blocks_recording() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);
        let merchant = Address::generate(&env);
        let payer = Address::generate(&env);
        let tx_hash = BytesN::from_array(&env, &[2u8; 32]);

        client.pause();
        let res = client.try_record_payment(&merchant, &payer, &1u64, &100_0000i128, &tx_hash);
        assert_eq!(res, Err(Ok(PaymentError::Paused)));
    }
}
