#![no_std]

use konfirm_common::{require_admin, require_not_paused, set_paused, CommonError};
use soroban_sdk::{contract, contractclient, contracterror, contractevent, contractimpl, contracttype, token, vec, Address, Env, Vec};

const TTL_THRESHOLD: u32 = 100;
const TTL_EXTEND_TO: u32 = 5_000_000;

/// Maximum lifetime (in ledgers) a settlement may stay `Pending` before it
/// becomes eligible for permissionless cleanup via `sweep_expired_settlement`.
const MAX_PENDING_LIFETIME: u64 = 17_280; // ~1 day at 5s/ledger

// Minimal cross-contract client for the compliance contract's `is_allowed`.
// The compliance contract address is supplied by the admin at
// `set_compliance` time so this contract can consult the blocklist before
// a settlement proposes or pays out to a blocked merchant.
#[contractclient(name = "ComplianceClient")]
pub trait ComplianceInterface {
    fn is_allowed(env: Env, address: Address) -> bool;
}

#[contractevent]
#[derive(Clone, Debug, PartialEq)]
pub struct InitEvent {
    #[topic]
    pub admin: Address,
}

#[contractevent]
#[derive(Clone, Debug, PartialEq)]
pub struct ComplianceSetEvent {
    pub compliance: Address,
}

#[contractevent]
#[derive(Clone, Debug, PartialEq)]
pub struct SettlementProposedEvent {
    #[topic]
    pub merchant: Address,
    pub settlement_id: u64,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, PartialEq)]
pub struct SettlementApprovedEvent {
    #[topic]
    pub signer: Address,
    pub settlement_id: u64,
}

#[contractevent]
#[derive(Clone, Debug, PartialEq)]
pub struct SettlementExecutedEvent {
    #[topic]
    pub merchant: Address,
    pub settlement_id: u64,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, PartialEq)]
pub struct SettlementExpiredEvent {
    pub settlement_id: u64,
}

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
    // Parallel to `approvals`: approved_at[i] is the ledger timestamp at which approvals[i] signed.
    pub approved_at: Vec<u64>,
    pub status: SettlementStatus,
    // Ledger timestamp at which the settlement was proposed; used for expiry.
    pub proposed_at: u64,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    Admin,
    Token,
    Paused,
    Signers,
    Threshold,
    NextId,
    Settlement(u64),
    Compliance,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum TreasuryError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    /// Never returned: admin/signer gates use `require_auth()`, which traps at the host before this can be produced. Code 3 stays reserved for ABI stability.
    Unauthorized = 3,
    Paused = 4,
    NotFound = 5,
    ZeroAmount = 6,
    NotSigner = 7,
    AlreadyApproved = 8,
    ThresholdNotMet = 9,
    NotPending = 10,
    InvalidThreshold = 11,
    DuplicateSigner = 12,
    NotExpired = 13,
    ComplianceBlocked = 14,
}

impl From<CommonError> for TreasuryError {
    fn from(err: CommonError) -> Self {
        match err {
            CommonError::NotInitialized => TreasuryError::NotInitialized,
            CommonError::Paused => TreasuryError::Paused,
        }
    }
}

#[contract]
pub struct TreasuryContract;

#[contractimpl]
impl TreasuryContract {
    /// One-time setup. Requires the admin's auth; not pause-gated. Stores the
    /// admin, the payout token, the signer set and the approval threshold —
    /// the number of distinct signer approvals a settlement needs before it
    /// can execute. Fails with `AlreadyInitialized` on a second call,
    /// `InvalidThreshold` unless `1 <= threshold <= signers.len()`, and
    /// `DuplicateSigner` if an address appears twice in `signers`. The
    /// contract holds no funds until it is funded separately;
    /// `execute_settlement` pays out of its balance.
    pub fn initialize(
        env: Env,
        admin: Address,
        token: Address,
        signers: Vec<Address>,
        threshold: u32,
    ) -> Result<(), TreasuryError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(TreasuryError::AlreadyInitialized);
        }
        if threshold == 0 || threshold > signers.len() {
            return Err(TreasuryError::InvalidThreshold);
        }
        for (i, signer) in signers.iter().enumerate() {
            if signers.iter().skip(i + 1).any(|other| other == signer) {
                return Err(TreasuryError::DuplicateSigner);
            }
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().set(&DataKey::Signers, &signers);
        env.storage().instance().set(&DataKey::Threshold, &threshold);
        env.storage().instance().set(&DataKey::NextId, &1u64);
        InitEvent { admin }.publish(&env);
        Ok(())
    }

    /// Admin-only. Sets the compliance contract consulted by
    /// `propose_settlement` and `execute_settlement` to reject blocked
    /// addresses before funds move. Fails with `NotInitialized` before
    /// `initialize`.
    pub fn set_compliance(env: Env, compliance: Address) -> Result<(), TreasuryError> {
        env.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Compliance, &compliance);
        ComplianceSetEvent { compliance }.publish(&env);
        Ok(())
    }

    // Requires `who`'s auth and that `who` is in the signer set (`NotSigner`).
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

    // Requires the stored admin's auth; `NotInitialized` before `initialize`.
    // Delegates to the shared `konfirm_common::require_admin` helper.
    fn require_admin(env: &Env) -> Result<(), TreasuryError> {
        require_admin(env, &DataKey::Admin)?;
        Ok(())
    }

    // Rejects `who` if the configured compliance contract reports it blocked.
    // No-op when no compliance contract has been configured.
    fn require_allowed(env: &Env, who: &Address) -> Result<(), TreasuryError> {
        if let Some(compliance) = env.storage().instance().get::<DataKey, Address>(&DataKey::Compliance) {
            let client = ComplianceClient::new(env, &compliance);
            if !client.is_allowed(who) {
                return Err(TreasuryError::ComplianceBlocked);
            }
        }
        Ok(())
    }

    // Persists a Settlement and bumps its persistent-entry TTL, mirroring
    // `channel::save_channel`. Every Settlement write must go through here so
    // a settlement that sits `Pending` through a slow multisig review is never
    // archived out from under `approve_settlement`/`execute_settlement`.
    //
    // TTL horizon: `TTL_EXTEND_TO` (5_000_000 ledgers) is the same horizon the
    // channel contract uses. At ~5s per ledger that is roughly 9 months, far
    // longer than any realistic approval cycle (days to weeks), so a Pending
    // settlement stays live across the whole review window without an
    // off-chain restore op.
    fn save_settlement(env: &Env, id: u64, settlement: &Settlement) {
        env.storage().persistent().set(&DataKey::Settlement(id), settlement);
        env.storage()
            .persistent()
            .extend_ttl(&DataKey::Settlement(id), TTL_THRESHOLD, TTL_EXTEND_TO);
    }

    // Current token balance held by this contract.
    fn token_balance(env: &Env) -> i128 {
        let token: Address = match env.storage().instance().get(&DataKey::Token) {
            Some(t) => t,
            None => return 0,
        };
        token::Client::new(env, &token).balance(&env.current_contract_address())
    }

    // Sum of `amount` across every still-`Pending` settlement. These have
    // first claim on the treasury balance ahead of any newly-approved
    // settlement, so signers should treat this as committed funds.
    fn pending_committed(env: &Env) -> i128 {
        let next_id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        let mut total: i128 = 0;
        let mut id: u64 = 1;
        while id < next_id {
            if let Some(settlement) = env
                .storage()
                .persistent()
                .get::<DataKey, Settlement>(&DataKey::Settlement(id))
            {
                if settlement.status == SettlementStatus::Pending {
                    total += settlement.amount;
                }
            }
            id += 1;
        }
        total
    }

    /// Signer-only, pause-gated. Opens a new `Pending` settlement paying
    /// `amount` to `merchant` and returns its id. The proposer's approval
    /// counts as the first of the `threshold` required. Does not move funds.
    /// Fails with `NotSigner`, `ZeroAmount` if `amount <= 0`, and `Paused`
    /// while paused.
    ///
    /// Deliberate choice: this does NOT reject a proposal that would
    /// over-commit the treasury. Over-commitment is allowed and surfaces at
    /// `execute_settlement` time (which traps rather than falsely succeeding).
    /// Signers should consult `available_balance` / `is_settlement_payable`
    /// before approving to avoid committing to a settlement that cannot be
    /// paid in full.
    pub fn propose_settlement(
        env: Env,
        proposer: Address,
        merchant: Address,
        amount: i128,
    ) -> Result<u64, TreasuryError> {
        env.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
        Self::require_signer(&env, &proposer)?;
        require_not_paused(&env, &DataKey::Paused)?;
        if amount <= 0 {
            return Err(TreasuryError::ZeroAmount);
        }
        Self::require_allowed(&env, &merchant)?;

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(1);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        let settlement = Settlement {
            merchant: merchant.clone(),
            amount,
            approvals: vec![&env, proposer],
            approved_at: vec![&env, env.ledger().timestamp()],
            status: SettlementStatus::Pending,
            proposed_at: env.ledger().timestamp(),
        };
        Self::save_settlement(&env, id, &settlement);
        SettlementProposedEvent { merchant, settlement_id: id, amount }.publish(&env);
        Ok(id)
    }

    /// Signer-only, pause-gated. Adds `signer`'s approval to a `Pending`
    /// settlement. Each signer may approve a given settlement once
    /// (`AlreadyApproved`), so the approval count is the number of distinct
    /// signers. Reaching the threshold does not execute the settlement; that
    /// takes a separate `execute_settlement` call. Fails with `NotSigner`,
    /// `NotFound`, `NotPending`, and `Paused` while paused.
    pub fn approve_settlement(env: Env, signer: Address, settlement_id: u64) -> Result<(), TreasuryError> {
        env.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
        Self::require_signer(&env, &signer)?;
        require_not_paused(&env, &DataKey::Paused)?;

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
        settlement.approvals.push_back(signer.clone());
        settlement.approved_at.push_back(env.ledger().timestamp());
        env.storage().persistent().set(&DataKey::Settlement(settlement_id), &settlement);
        env.storage().persistent().extend_ttl(&DataKey::Settlement(settlement_id), TTL_THRESHOLD, TTL_EXTEND_TO);
        SettlementApprovedEvent { signer, settlement_id }.publish(&env);
        Ok(())
    }

    /// Permissionless. Read-only view. Returns the treasury's current token
    /// balance minus the sum of `amount` across every still-`Pending` settlement. This is the
    /// balance available to a newly-approved settlement after already-pending
    /// settlements' first claim. A negative result means the treasury is
    /// over-committed: some pending settlements cannot be paid in full even
    /// if all of them execute. Signers/tooling should check this before
    /// approving a settlement.
    pub fn available_balance(env: Env) -> i128 {
        Self::token_balance(&env) - Self::pending_committed(&env)
    }

    /// Permissionless. Read-only view. Returns the total amount committed to
    /// still-`Pending` settlements (their first claim on the treasury balance).
    pub fn committed_balance(env: Env) -> i128 {
        Self::pending_committed(&env)
    }

    /// Permissionless. Read-only view. Reports whether `settlement_id` is
    /// currently payable in full given the treasury balance and the first
    /// claim of all *other* still-`Pending` settlements. Returns `false` for
    /// a missing or non-`Pending` settlement. This is a proactive signal:
    /// `execute_settlement` traps rather than falsely succeeding when a
    /// settlement is underfunded, so callers should consult this before
    /// relying on a settlement being payable.
    pub fn is_settlement_payable(env: Env, settlement_id: u64) -> bool {
        let settlement: Settlement = match env
            .storage()
            .persistent()
            .get(&DataKey::Settlement(settlement_id))
        {
            Some(s) => s,
            None => return false,
        };
        if settlement.status != SettlementStatus::Pending {
            return false;
        }
        let others = Self::pending_committed(&env) - settlement.amount;
        Self::token_balance(&env) - others >= settlement.amount
    }

    /// Signer-only, pause-gated. Any one signer (`caller`) can execute once the
    /// settlement's approval count reaches the threshold; fails with
    /// `ThresholdNotMet` before then. Executing does not count as an approval.
    /// Transfers `amount` of the payout token from the treasury to the
    /// merchant, then marks the settlement `Executed`. If the transfer traps,
    /// the whole call reverts and the settlement stays `Pending`. Also fails
    /// with `NotSigner`, `NotFound`, `NotPending`, `ComplianceBlocked` if the
    /// merchant is blocked by the configured compliance contract, and `Paused`
    /// while paused.
    pub fn execute_settlement(env: Env, caller: Address, settlement_id: u64) -> Result<(), TreasuryError> {
        env.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
        Self::require_signer(&env, &caller)?;
        require_not_paused(&env, &DataKey::Paused)?;

        let mut settlement: Settlement = env
            .storage()
            .persistent()
            .get(&DataKey::Settlement(settlement_id))
            .ok_or(TreasuryError::NotFound)?;
        if settlement.status != SettlementStatus::Pending {
            return Err(TreasuryError::NotPending);
        }
        let threshold: u32 = env
            .storage()
            .instance()
            .get(&DataKey::Threshold)
            .ok_or(TreasuryError::NotInitialized)?;
        if settlement.approvals.len() < threshold {
            return Err(TreasuryError::ThresholdNotMet);
        }
        Self::require_allowed(&env, &settlement.merchant)?;

        // Transfer before marking Executed, same order-of-operations
        // channel::checkpoint already uses — if the transfer traps (e.g.
        // this contract's own balance is somehow short), the whole call
        // reverts and the settlement stays Pending rather than being
        // marked paid out when it wasn't.
        let token_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::Token)
            .ok_or(TreasuryError::NotInitialized)?;
        token::Client::new(&env, &token_addr).transfer(
            &env.current_contract_address(),
            &settlement.merchant,
            &settlement.amount,
        );

        settlement.status = SettlementStatus::Executed;
        let merchant = settlement.merchant.clone();
        let amount = settlement.amount;
        Self::save_settlement(&env, settlement_id, &settlement);
        // Carries the merchant and amount, not just the id (issue #76): a
        // listener otherwise has to look the settlement back up to know what
        // was actually paid out.
        SettlementExecutedEvent { merchant, settlement_id, amount }.publish(&env);
        Ok(())
    }

    /// Permissionless. Read-only and not pause-gated. Returns the settlement,
    /// or `NotFound` for an unknown id.
    pub fn get_settlement(env: Env, settlement_id: u64) -> Result<Settlement, TreasuryError> {
        env.storage()
            .persistent()
            .get(&DataKey::Settlement(settlement_id))
            .ok_or(TreasuryError::NotFound)
    }

    /// Permissionless. Transitions a `Pending` settlement that has outlived
    /// `MAX_PENDING_LIFETIME` to `Cancelled`, mirroring `channel`'s
    /// `finalize_close` cleanup pattern. Anyone may call this; no auth is
    /// required. Fails with `NotFound`, `NotPending` if the settlement is not
    /// `Pending`, and `NotExpired` if it is still within its lifetime.
    pub fn sweep_expired_settlement(env: Env, settlement_id: u64) -> Result<(), TreasuryError> {
        env.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);

        let mut settlement: Settlement = env
            .storage()
            .persistent()
            .get(&DataKey::Settlement(settlement_id))
            .ok_or(TreasuryError::NotFound)?;
        if settlement.status != SettlementStatus::Pending {
            return Err(TreasuryError::NotPending);
        }
        let now = env.ledger().timestamp();
        if now < settlement.proposed_at.saturating_add(MAX_PENDING_LIFETIME) {
            return Err(TreasuryError::NotExpired);
        }

        settlement.status = SettlementStatus::Cancelled;
        Self::save_settlement(&env, settlement_id, &settlement);
        SettlementExpiredEvent { settlement_id }.publish(&env);
        Ok(())
    }

    /// Admin-only. Blocks `propose_settlement`, `approve_settlement` and
    /// `execute_settlement` until unpaused. Reads keep working while paused.
    pub fn pause(env: Env) -> Result<(), TreasuryError> {
        env.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &true);
        Ok(())
    }

    /// Admin-only. Lifts a pause set by `pause`. Callable while paused.
    pub fn unpause(env: Env) -> Result<(), TreasuryError> {
        env.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_EXTEND_TO);
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Paused, &false);
        Ok(())
    }

    /// Permissionless. Read-only. Returns whether the contract is paused.
    pub fn is_paused(env: Env) -> bool {
        env.storage().instance().get(&DataKey::Paused).unwrap_or(false)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    fn setup(env: &Env) -> (TreasuryContractClient<'_>, Address, Vec<Address>, Address) {
        let admin = Address::generate(env);
        let token_admin = Address::generate(env);
        let sac = env.register_stellar_asset_contract_v2(token_admin);
        let token_id = sac.address();
        let signers = vec![
            env,
            Address::generate(env),
            Address::generate(env),
            Address::generate(env),
        ];
        let contract_id = env.register(TreasuryContract, ());
        let client = TreasuryContractClient::new(env, &contract_id);
        client.initialize(&admin, &token_id, &signers, &2u32);
        // Fund the treasury itself so execute_settlement's real transfer has
        // something to pay out.
        token::StellarAssetClient::new(env, &token_id).mint(&contract_id, &100_000_0000i128);
        (client, admin, signers, token_id)
    }

    #[test]
    fn zero_threshold_rejected_at_init() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let token = Address::generate(&env);
        let signers = vec![&env, Address::generate(&env)];
        let contract_id = env.register(TreasuryContract, ());
        let client = TreasuryContractClient::new(&env, &contract_id);
        let res = client.try_initialize(&admin, &token, &signers, &0u32);
        assert_eq!(res, Err(Ok(TreasuryError::InvalidThreshold)));
    }

    #[test]
    fn execute_settlement_actually_transfers_the_real_token_balance() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, signers, token_id) = setup(&env);
        let merchant = Address::generate(&env);
        let token_client = token::TokenClient::new(&env, &token_id);

        let contract_balance_before = token_client.balance(&client.address);
        let merchant_balance_before = token_client.balance(&merchant);
        assert_eq!(merchant_balance_before, 0);

        let id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);
        client.approve_settlement(&signers.get(1).unwrap(), &id);
        client.execute_settlement(&signers.get(0).unwrap(), &id);

        // The actual gap this whole fix closes: before it, execute_settlement
        // only flipped a status flag and neither balance below would have
        // moved at all.
        assert_eq!(token_client.balance(&merchant), 1_000_0000i128);
        assert_eq!(token_client.balance(&client.address), contract_balance_before - 1_000_0000i128);
    }

    #[test]
    fn execute_settlement_traps_and_settlement_stays_pending_if_treasury_is_short() {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let token_admin = Address::generate(&env);
        let sac = env.register_stellar_asset_contract_v2(token_admin);
        let token_id = sac.address();
        let signers = vec![
            &env,
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        ];
        let contract_id = env.register(TreasuryContract, ());
        let client = TreasuryContractClient::new(&env, &contract_id);
        client.initialize(&admin, &token_id, &signers, &2u32);
        // Deliberately NOT funded — this is the one test in this module
        // that doesn't use setup()'s generous mint, specifically to prove
        // an underfunded treasury can't silently mark a settlement
        // Executed without the money actually moving.
        let merchant = Address::generate(&env);

        let id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);
        client.approve_settlement(&signers.get(1).unwrap(), &id);
        let res = client.try_execute_settlement(&signers.get(0).unwrap(), &id);
        assert!(res.is_err(), "an underfunded transfer should trap, not silently succeed");

        let settlement = client.get_settlement(&id);
        assert_eq!(settlement.status, SettlementStatus::Pending, "a reverted execute must leave the settlement Pending, not Executed");
    }

    #[test]
    fn approve_settlement_event_records_approving_signer() {
        use soroban_sdk::testutils::Events as _;
        use soroban_sdk::Event as _;
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, signers, _token) = setup(&env);
        let merchant = Address::generate(&env);

        let id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);
        let approver = signers.get(1).unwrap();
        client.approve_settlement(&approver, &id);

        let expected = SettlementApprovedEvent { signer: approver.clone(), settlement_id: id };
        assert_eq!(env.events().all(), [expected.to_xdr(&env, &client.address)]);
    }

    // issue #255: available_balance, committed_balance, is_settlement_payable
    // and sweep_expired_settlement had zero test coverage. This exercises
    // the accounting across propose -> execute, including a second
    // concurrently-pending settlement so "committed" genuinely reflects the
    // sum across settlements rather than just the one under test.
    #[test]
    fn available_and_committed_balance_track_pending_settlements() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, signers, token_id) = setup(&env);
        let merchant_a = Address::generate(&env);
        let merchant_b = Address::generate(&env);
        let token_balance = token::TokenClient::new(&env, &token_id).balance(&client.address);

        // Nothing pending yet: the whole balance is available.
        assert_eq!(client.committed_balance(), 0);
        assert_eq!(client.available_balance(), token_balance);

        let id_a = client.propose_settlement(&signers.get(0).unwrap(), &merchant_a, &1_000_0000i128);
        assert_eq!(client.committed_balance(), 1_000_0000i128);
        assert_eq!(client.available_balance(), token_balance - 1_000_0000i128);

        let id_b = client.propose_settlement(&signers.get(0).unwrap(), &merchant_b, &2_000_0000i128);
        assert_eq!(client.committed_balance(), 3_000_0000i128);
        assert_eq!(client.available_balance(), token_balance - 3_000_0000i128);

        // Executing settlement A moves real funds out (shrinking the actual
        // token balance) and removes A from the pending/committed total —
        // both effects must net out so available_balance tracks the same
        // number either way.
        client.approve_settlement(&signers.get(1).unwrap(), &id_a);
        client.execute_settlement(&signers.get(0).unwrap(), &id_a);
        assert_eq!(client.committed_balance(), 2_000_0000i128);
        let new_token_balance = token::TokenClient::new(&env, &token_id).balance(&client.address);
        assert_eq!(new_token_balance, token_balance - 1_000_0000i128);
        assert_eq!(client.available_balance(), new_token_balance - 2_000_0000i128);

        let _ = id_b; // id_b stays Pending throughout; only its committed total is asserted.
    }

    #[test]
    fn is_settlement_payable_reflects_funding_and_settlement_state() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, signers, _token) = setup(&env);
        let merchant = Address::generate(&env);

        // Unknown id: false, not a trap.
        assert!(!client.is_settlement_payable(&999u64));

        let id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);
        // The treasury (funded with 100_000_0000 in setup()) can cover this
        // comfortably on its own.
        assert!(client.is_settlement_payable(&id));

        // Once Executed, it is no longer "payable" in this sense — it has
        // already been paid.
        client.approve_settlement(&signers.get(1).unwrap(), &id);
        client.execute_settlement(&signers.get(0).unwrap(), &id);
        assert!(!client.is_settlement_payable(&id));

        // A settlement that over-commits the treasury relative to the other
        // still-Pending settlement's first claim is reported unpayable.
        let big_id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &90_000_0000i128);
        let other_id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &20_000_0000i128);
        assert!(!client.is_settlement_payable(&big_id));
        assert!(!client.is_settlement_payable(&other_id));
    }

    #[test]
    fn sweep_expired_settlement_cancels_only_after_lifetime_elapses_and_frees_committed_balance() {
        use soroban_sdk::testutils::Ledger;
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, signers, token_id) = setup(&env);
        let merchant = Address::generate(&env);
        let token_balance = token::TokenClient::new(&env, &token_id).balance(&client.address);

        let id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);

        // Not expired yet: rejected one unit before the boundary.
        env.ledger().with_mut(|l| l.timestamp += MAX_PENDING_LIFETIME - 1);
        let res = client.try_sweep_expired_settlement(&id);
        assert_eq!(res, Err(Ok(TreasuryError::NotExpired)));
        assert_eq!(client.get_settlement(&id).status, SettlementStatus::Pending);
        assert_eq!(client.committed_balance(), 1_000_0000i128);

        // Exactly at the boundary, the sweep succeeds.
        env.ledger().with_mut(|l| l.timestamp += 1);
        client.sweep_expired_settlement(&id);
        assert_eq!(client.get_settlement(&id).status, SettlementStatus::Cancelled);

        // Cancelling frees the committed claim: no real funds ever moved
        // (sweep never calls the token contract), so the whole balance is
        // available again.
        assert_eq!(client.committed_balance(), 0);
        assert_eq!(client.available_balance(), token_balance);
        assert_eq!(token::TokenClient::new(&env, &token_id).balance(&client.address), token_balance);
    }

    #[test]
    fn sweep_expired_settlement_rejects_unknown_and_non_pending() {
        use soroban_sdk::testutils::Ledger;
        let env = Env::default();
        env.mock_all_auths();
        let (client, _admin, signers, _token) = setup(&env);
        let merchant = Address::generate(&env);

        let res = client.try_sweep_expired_settlement(&999u64);
        assert_eq!(res, Err(Ok(TreasuryError::NotFound)));

        let id = client.propose_settlement(&signers.get(0).unwrap(), &merchant, &1_000_0000i128);
        client.approve_settlement(&signers.get(1).unwrap(), &id);
        client.execute_settlement(&signers.get(0).unwrap(), &id);

        env.ledger().with_mut(|l| l.timestamp += MAX_PENDING_LIFETIME + 1);
        let res = client.try_sweep_expired_settlement(&id);
        assert_eq!(res, Err(Ok(TreasuryError::NotPending)));
        assert_eq!(client.get_settlement(&id).status, SettlementStatus::Executed);
    }
}

#[cfg(test)]
mod executed_event_test;
