#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, symbol_short, vec, Address, Env, Vec};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettlementStatus {
    Pending,
    Executed,
    Cancelled,
}

#[contracttype]
#[derive(Clone)]
pub struct Settlement {
    pub merchant: Address,
    pub amount: i128,
    pub approvals: Vec<Address>,
    pub status: SettlementStatus,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Paused,
    Signers,
    Threshold,
    NextId,
    Settlement(u64),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum TreasuryError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    Paused = 4,
    NotFound = 5,
    ZeroAmount = 6,
    NotSigner = 7,
    AlreadyApproved = 8,
    ThresholdNotMet = 9,
    NotPending = 10,
    InvalidThreshold = 11,
}

#[contract]
pub struct TreasuryContract;

#[contractimpl]
impl TreasuryContract {
    pub fn initialize(
        env: Env,
        admin: Address,
        signers: Vec<Address>,
        threshold: u32,
    ) -> Result<(), TreasuryError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(TreasuryError::AlreadyInitialized);
        }
        if threshold == 0 || threshold > signers.len() {
            return Err(TreasuryError::InvalidThreshold);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().set(&DataKey::Signers, &signers);
        env.storage().instance().set(&DataKey::Threshold, &threshold);
        env.storage().instance().set(&DataKey::NextId, &1u64);
        Ok(())
    }

    fn require_not_paused(env: &Env) -> Result<(), TreasuryError> {
        let paused: bool = env.storage().instance().get(&DataKey::Paused).unwrap_or(false);
        if paused {
            return Err(TreasuryError::Paused);
        }
        Ok(())
    }

    fn require_signer(env: &Env, who: &Address) -> Result<(), TreasuryError> {
        who.require_auth();
        let signers: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Signers)
            .ok_or(TreasuryError::NotInitialized)?;
        if !signers.contains(who) {
            return Err(TreasuryError::NotSigner);
        }
        Ok(())
    }

    fn require_admin(env: &Env) -> Result<(), TreasuryError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(TreasuryError::NotInitialized)?;
        admin.require_auth();
        Ok(())
    }

    pub fn propose_settlement(
        env: Env,
        proposer: Address,
        merchant: Address,
        amount: i128,
    ) -> Result<u64, TreasuryError> {
        Self::require_signer(&env, &proposer)?;
        Self::require_not_paused(&env)?;
        if amount <= 0 {
            return Err(TreasuryError::ZeroAmount);
        }

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        let settlement = Settlement {
            merchant: merchant.clone(),
            amount,
            approvals: vec![&env, proposer],
            status: SettlementStatus::Pending,
        };
        env.storage().persistent().set(&DataKey::Settlement(id), &settlement);
        env.events().publish((symbol_short!("proposed"), merchant), (id, amount));
        Ok(id)
    }

    pub fn approve_settlement(env: Env, signer: Address, settlement_id: u64) -> Result<(), TreasuryError> {
        Self::require_signer(&env, &signer)?;
        Self::require_not_paused(&env)?;

        let mut settlement: Settlement = env
            .storage()
            .persistent()
            .get(&DataKey::Settlement(settlement_id))
            .ok_or(TreasuryError::NotFound)?;
        if settlement.status != SettlementStatus::Pending {
            return Err(TreasuryError::NotPending);
        }
        if settlement.approvals.contains(&signer) {
            return Err(TreasuryError::AlreadyApproved);
        }
        settlement.approvals.push_back(signer);
        env.storage().persistent().set(&DataKey::Settlement(settlement_id), &settlement);
        env.events().publish((symbol_short!("approved"),), settlement_id);
        Ok(())
    }

    pub fn execute_settlement(env: Env, settlement_id: u64) -> Result<(), TreasuryError> {
        Self::require_not_paused(&env)?;
        let threshold: u32 = env
            .storage()
            .instance()
            .get(&DataKey::Threshold)
            .ok_or(TreasuryError::NotInitialized)?;

        let mut settlement: Settlement = env
            .storage()
            .persistent()
            .get(&DataKey::Settlement(settlement_id))
            .ok_or(TreasuryError::NotFound)?;
        if settlement.status != SettlementStatus::Pending {
            return Err(TreasuryError::NotPending);
        }
        if settlement.approvals.len() < threshold {
            return Err(TreasuryError::ThresholdNotMet);
        }
        settlement.status = SettlementStatus::Executed;
        env.storage().persistent().set(&DataKey::Settlement(settlement_id), &settlement);
        env.events().publish((symbol_short!("executed"),), settlement_id);
        Ok(())
        // NOTE: honest gap — this does not yet move a real token balance via
        // token::TokenClient. Recording approval/threshold state is the part
        // worth getting right first; the disbursement transfer is a follow-up,
        // same shape as the transfer already implemented in the channel contract.
    }

    pub fn get_settlement(env: Env, settlement_id: u64) -> Result<Settlement, TreasuryError> {
        env.storage()
            .persistent()
            .get(&DataKey::Settlement(settlement_id))
            .ok_or(TreasuryError::NotFound)
    }

    pub fn pause(env: Env) -> Result<(), TreasuryError> {
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        Ok(())
    }

    pub fn unpause(env: Env) -> Result<(), TreasuryError> {
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    fn setup(env: &Env) -> (TreasuryContractClient<'_>, Address, Vec<Address>) {
        let admin = Address::generate(env);
        let signers = vec![
            env,
            Address::generate(env),
            Address::generate(env),
            Address::generate(env),
        ];
        let contract_id = env.register(TreasuryContract, ());
        let client = TreasuryContractClient::new(env, &contract_id);
        client.initialize(&admin, &signers, &2u32);
        (client, admin, signers)
    }

    #[test]
    fn threshold_not_met_below_two_signatures() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, signers) = setup(&env);
        let merchant = Address::generate(&env);

        let id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);
        let res = client.try_execute_settlement(&id);
        assert_eq!(res, Err(Ok(TreasuryError::ThresholdNotMet)));
    }

    #[test]
    fn executes_once_threshold_met() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, signers) = setup(&env);
        let merchant = Address::generate(&env);

        let id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);
        client.approve_settlement(&signers.get(1).unwrap(), &id);
        client.execute_settlement(&id);
        let settlement = client.get_settlement(&id);
        assert_eq!(settlement.status, SettlementStatus::Executed);
    }

    #[test]
    fn non_signer_cannot_approve() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, signers) = setup(&env);
        let merchant = Address::generate(&env);
        let outsider = Address::generate(&env);

        let id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);
        let res = client.try_approve_settlement(&outsider, &id);
        assert_eq!(res, Err(Ok(TreasuryError::NotSigner)));
    }

    #[test]
    fn double_approval_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, signers) = setup(&env);
        let merchant = Address::generate(&env);

        let id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);
        let res = client.try_approve_settlement(&signers.get(0).unwrap(), &id);
        assert_eq!(res, Err(Ok(TreasuryError::AlreadyApproved)));
    }

    #[test]
    fn double_execution_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, signers) = setup(&env);
        let merchant = Address::generate(&env);

        let id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);
        client.approve_settlement(&signers.get(1).unwrap(), &id);
        client.execute_settlement(&id);
        let res = client.try_execute_settlement(&id);
        assert_eq!(res, Err(Ok(TreasuryError::NotPending)));
    }

    #[test]
    fn invalid_threshold_rejected_at_init() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let signers = vec![&env, Address::generate(&env)];
        let contract_id = env.register(TreasuryContract, ());
        let client = TreasuryContractClient::new(&env, &contract_id);
        let res = client.try_initialize(&admin, &signers, &5u32);
        assert_eq!(res, Err(Ok(TreasuryError::InvalidThreshold)));
    }
}
