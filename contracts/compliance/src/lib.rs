#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env};

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Paused,
    Blocked(Address),
    Allowed(Address),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ComplianceError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    Paused = 4,
}

#[contract]
pub struct ComplianceContract;

// Default-allow, deny-list model: an address is screened out only if it has
// been explicitly blocked. This matches how sanctions screening actually
// works in practice (absence from a list means cleared) rather than
// requiring every legitimate address to be pre-enrolled.
#[contractimpl]
impl ComplianceContract {
    /// One-time setup: stores `admin` and starts unpaused. `admin` must authorize
    /// the call. Returns `AlreadyInitialized` if an admin is already set.
    pub fn initialize(env: Env, admin: Address) -> Result<(), ComplianceError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(ComplianceError::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.events().publish((symbol_short!("init"),), admin);
        Ok(())
    }

    /// Loads the stored admin and requires its authorization. Returns
    /// `NotInitialized` if `initialize` has not been called.
    fn require_admin(env: &Env) -> Result<Address, ComplianceError> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(ComplianceError::NotInitialized)?;
        admin.require_auth();
        Ok(admin)
    }

    /// Returns `Paused` while the contract is paused; otherwise `Ok(())`.
    fn require_not_paused(env: &Env) -> Result<(), ComplianceError> {
        let paused: bool = env.storage().instance().get(&DataKey::Paused).unwrap_or(false);
        if paused {
            return Err(ComplianceError::Paused);
        }
        Ok(())
    }

    /// Read-only, and deliberately callable even while paused — pausing the
    /// contract must never be able to brick checkout by blocking the one
    /// query the payment-preparation path depends on.
    pub fn is_allowed(env: Env, addr: Address) -> bool {
        !env.storage().persistent().has(&DataKey::Blocked(addr))
    }

    /// Admin-only; fails with `Paused` while paused. Marks `addr` as blocked, so
    /// `is_allowed` returns `false` for it. Also writes `Allowed(addr) = false`
    /// as an explicit "not allowed" record, the counterpart of the `true` record
    /// `allow_address` writes; `is_allowed` only looks at the blocked marker.
    pub fn block_address(env: Env, addr: Address) -> Result<(), ComplianceError> {
        Self::require_admin(&env)?;
        Self::require_not_paused(&env)?;
        env.storage().persistent().set(&DataKey::Blocked(addr.clone()), &true);
        env.storage().persistent().set(&DataKey::Allowed(addr.clone()), &false);
        env.events().publish((symbol_short!("blocked"),), addr);
        Ok(())
    }

    /// Explicitly clears a block (e.g. after manual review) and leaves an
    /// audit record distinct from an address that was simply never flagged.
    pub fn allow_address(env: Env, addr: Address) -> Result<(), ComplianceError> {
        Self::require_admin(&env)?;
        Self::require_not_paused(&env)?;
        env.storage().persistent().remove(&DataKey::Blocked(addr.clone()));
        env.storage().persistent().set(&DataKey::Allowed(addr.clone()), &true);
        env.events().publish((symbol_short!("allowed"),), addr);
        Ok(())
    }

    /// Admin-only; fails with `Paused` while paused. Removes both the blocked and
    /// allowed records for `addr`, returning it to the default-allow state as if
    /// it had never been flagged (unlike `allow_address`, which leaves an
    /// explicit allowed record).
    pub fn clear_address(env: Env, addr: Address) -> Result<(), ComplianceError> {
        Self::require_admin(&env)?;
        Self::require_not_paused(&env)?;
        env.storage().persistent().remove(&DataKey::Blocked(addr.clone()));
        env.storage().persistent().remove(&DataKey::Allowed(addr.clone()));
        env.events().publish((symbol_short!("cleared"),), addr);
        Ok(())
    }

    /// Admin-only. Pauses the contract: `block_address`, `allow_address` and
    /// `clear_address` fail with `Paused`. `is_allowed` keeps working.
    pub fn pause(env: Env) -> Result<(), ComplianceError> {
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        Ok(())
    }

    /// Admin-only. Resumes the contract after `pause`, re-enabling
    /// `block_address`, `allow_address` and `clear_address`.
    pub fn unpause(env: Env) -> Result<(), ComplianceError> {
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    fn setup(env: &Env) -> (ComplianceContractClient<'_>, Address) {
        let admin = Address::generate(env);
        let contract_id = env.register(ComplianceContract, ());
        let client = ComplianceContractClient::new(env, &contract_id);
        client.initialize(&admin);
        (client, admin)
    }

    #[test]
    fn default_allow_until_blocked() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);
        let addr = Address::generate(&env);

        assert!(client.is_allowed(&addr));
        client.block_address(&addr);
        assert!(!client.is_allowed(&addr));
    }

    #[test]
    fn clear_restores_default_allow() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);
        let addr = Address::generate(&env);

        client.block_address(&addr);
        assert!(!client.is_allowed(&addr));
        client.allow_address(&addr);
        assert!(client.is_allowed(&addr));
        client.clear_address(&addr);
        assert!(client.is_allowed(&addr));
    }

    #[test]
    fn is_allowed_works_while_paused() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);
        let addr = Address::generate(&env);

        client.block_address(&addr);
        client.pause();
        // must not panic / must not require unpausing to read
        assert!(!client.is_allowed(&addr));
    }

    #[test]
    fn mutations_rejected_while_paused() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin) = setup(&env);
        let addr = Address::generate(&env);

        client.pause();
        let res = client.try_block_address(&addr);
        assert_eq!(res, Err(Ok(ComplianceError::Paused)));
    }

    #[test]
    fn double_initialize_rejected() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, admin) = setup(&env);
        let res = client.try_initialize(&admin);
        assert_eq!(res, Err(Ok(ComplianceError::AlreadyInitialized)));
    }
}
